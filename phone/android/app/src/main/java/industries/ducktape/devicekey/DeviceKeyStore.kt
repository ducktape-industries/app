package industries.ducktape.devicekey

import android.os.Build
import android.system.Os
import android.system.OsConstants
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import com.google.crypto.tink.subtle.Ed25519Sign
import java.io.File
import java.io.FileOutputStream
import java.security.GeneralSecurityException
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.ProviderException
import java.security.SecureRandom
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec

/**
 * This device's Ed25519 keys, one per keyring alias. The Rust core calls these over JNI; no key ever
 * reaches JS.
 *
 * Where Keystore holds Ed25519 (Android 13, API 33, and up) the key is Keystore's and never leaves
 * it. Below that, or on a Keystore without the curve, the key is a software Ed25519 seed sealed by a
 * Keystore AES-GCM key in app-private storage (bound to its alias), opened into this process's memory
 * only to sign and zeroed after.
 */
object DeviceKeyStore {
  /** Loading the class loads the core's library and hands Rust this class (see `key.rs`). */
  val started: Boolean

  /** Where sealed seeds are kept; set by [DeviceKeyModule] before the core asks for a key. */
  @Volatile var sealedDir: File? = null

  init {
    System.loadLibrary("ducktape_phone")
    attach()
    started = true
  }

  @JvmStatic private external fun attach()

  private const val SEAL_ALIAS = "ducktape:device-key-seal"
  private val keystore: KeyStore by lazy { KeyStore.getInstance("AndroidKeyStore").apply { load(null) } }

  /** The 32-byte Ed25519 public key under [alias], made on first ask. Throws when Keystore refuses. */
  @JvmStatic
  @Synchronized
  fun publicKey(alias: String): ByteArray {
    unseal(alias)?.let { seed -> return seed.wiped { Ed25519Sign.KeyPair.newKeyPairFromSeed(it).publicKey } }
    keystore.getCertificate(alias)?.let { return lastBytes(it.publicKey.encoded) }
    return make(alias)
  }

  /** PureEdDSA over [message] (the caller has already applied the namespace). 64 bytes. */
  @JvmStatic
  fun sign(alias: String, message: ByteArray): ByteArray {
    unseal(alias)?.let { seed -> return seed.wiped { Ed25519Sign(it).sign(message) } }
    val key = keystore.getKey(alias, null) as? PrivateKey ?: error("no device key under $alias")
    return Signature.getInstance("Ed25519").run {
      initSign(key)
      update(message)
      sign()
    }
  }

  @JvmStatic fun exists(alias: String): Boolean = keystore.containsAlias(alias) || sealed(alias).exists()

  /**
   * Where the key under [alias] is kept, never a byte of it: `sealed` (a software seed that a Keystore key seals)
   * or Keystore's own, and `level`, the security level of the Keystore key that holds or seals it: `strongbox`,
   * `tee`, `secure` (secure hardware, which one not said), `software` or `unknown`.
   */
  @JvmStatic
  @Synchronized
  fun storage(alias: String): Map<String, Any> {
    val sealed = sealed(alias).exists()
    val info = if (sealed) {
      val key = keystore.getKey(SEAL_ALIAS, null) as? SecretKey ?: error("no seal key")
      SecretKeyFactory.getInstance(key.algorithm, "AndroidKeyStore").getKeySpec(key, KeyInfo::class.java) as KeyInfo
    } else {
      val key = keystore.getKey(alias, null) as? PrivateKey ?: error("no device key under $alias")
      // the key answers "EdDSA", which AndroidKeyStore has no KeyFactory for; it was made under EC (see [make])
      KeyFactory.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").getKeySpec(key, KeyInfo::class.java)
    }
    return mapOf("sealed" to sealed, "level" to level(info))
  }

  private fun level(info: KeyInfo): String {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) {
      @Suppress("DEPRECATION")
      return if (info.isInsideSecureHardware) "secure" else "software"
    }
    return when (info.securityLevel) {
      KeyProperties.SECURITY_LEVEL_STRONGBOX -> "strongbox"
      KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "tee"
      KeyProperties.SECURITY_LEVEL_UNKNOWN_SECURE -> "secure"
      KeyProperties.SECURITY_LEVEL_SOFTWARE -> "software"
      else -> "unknown"
    }
  }

  private fun make(alias: String): ByteArray {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      try {
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
          .setAlgorithmParameterSpec(ECGenParameterSpec("ed25519"))
          .setDigests(KeyProperties.DIGEST_NONE)
          .setUserAuthenticationRequired(false)
          .build()
        val pair = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
          initialize(spec)
          generateKeyPair()
        }
        return lastBytes(pair.public.encoded)
      } catch (refused: GeneralSecurityException) {
        // this device's Keystore has no Ed25519: seal a software key instead
      } catch (refused: ProviderException) {
      }
    }
    return ByteArray(32).also { SecureRandom().nextBytes(it) }.wiped { seed ->
      val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply {
        init(Cipher.ENCRYPT_MODE, sealKey())
        updateAAD(alias.toByteArray(Charsets.UTF_8))
      }
      keep(sealed(alias), cipher.iv + cipher.doFinal(seed))
      Ed25519Sign.KeyPair.newKeyPairFromSeed(seed).publicKey
    }
  }

  /**
   * Writes [bytes] as [file] whole or not at all: a temp file, synced, renamed over it, and the
   * folder synced, so a power cut never leaves a short file that [unseal] would refuse for good.
   */
  private fun keep(file: File, bytes: ByteArray) {
    val dir = file.parentFile!!.apply { mkdirs() }
    val temp = File(dir, file.name + ".tmp")
    FileOutputStream(temp).use { out ->
      out.write(bytes)
      out.fd.sync()
    }
    check(temp.renameTo(file)) { "could not keep the device key" }
    val folder = Os.open(dir.path, OsConstants.O_RDONLY, 0)
    try {
      Os.fsync(folder)
    } finally {
      Os.close(folder)
    }
  }

  /** The seed sealed under [alias], or null when that key is Keystore's (or not made yet). */
  private fun unseal(alias: String): ByteArray? {
    val file = sealed(alias)
    if (!file.exists()) return null
    val bytes = file.readBytes()
    val cipher = Cipher.getInstance("AES/GCM/NoPadding")
    cipher.init(Cipher.DECRYPT_MODE, sealKey(), GCMParameterSpec(128, bytes, 0, 12))
    cipher.updateAAD(alias.toByteArray(Charsets.UTF_8))
    return cipher.doFinal(bytes, 12, bytes.size - 12)
  }

  /** [use] of this seed, then the seed zeroed, whatever [use] did. */
  private inline fun <T> ByteArray.wiped(use: (ByteArray) -> T): T =
    try {
      use(this)
    } finally {
      fill(0)
    }

  private fun sealed(alias: String): File {
    val dir = checkNotNull(sealedDir) { "the device key store has no folder yet" }
    return File(dir, alias.replace(':', '_') + ".sealed")
  }

  private fun sealKey(): SecretKey {
    (keystore.getKey(SEAL_ALIAS, null) as? SecretKey)?.let { return it }
    val spec = KeyGenParameterSpec.Builder(SEAL_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
      .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
      .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
      .setKeySize(256)
      .build()
    return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
      init(spec)
      generateKey()
    }
  }

  /** An X.509 SubjectPublicKeyInfo's Ed25519 key: its last 32 bytes. */
  private fun lastBytes(encoded: ByteArray): ByteArray = encoded.copyOfRange(encoded.size - 32, encoded.size)
}
