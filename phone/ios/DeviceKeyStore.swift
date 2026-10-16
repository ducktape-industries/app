import CryptoKit
import Foundation
import Security

/// This device's Ed25519 keys, one per keyring alias. The Rust core calls these through the C ABI (the two
/// `@_cdecl` functions at the end, `key/ios.rs` in the core); no key ever reaches JS.
///
/// The Secure Enclave holds P-256 keys only, so the key is a software Ed25519 seed. Where this device has a Secure
/// Enclave, the seed is sealed by an Enclave P-256 key (ECIES, bound to its alias) and the sealed bytes are a
/// Keychain item, this device only; the seed is opened into this process's memory only to sign, and zeroed after.
/// With no Secure Enclave (the simulator: see `enclave`) the seed itself is the Keychain item, this device only.
///
/// A key lives as long as the app's container, as Android's does: Keychain items outlive the app, the keyring marks
/// that name a key's network (the app's Documents) do not. See `claim`.
enum DeviceKeyStore {
  /// The Enclave key that seals every seed: Android's seal alias.
  private static let sealTag = Data("ducktape:device-key-seal".utf8)
  /// A sealed seed's Keychain service, and a bare seed's (no Secure Enclave).
  private static let sealedService = "industries.ducktape.mobile.device-key.sealed"
  private static let seedService = "industries.ducktape.mobile.device-key.seed"
  private static let ecies = SecKeyAlgorithm.eciesEncryptionCofactorVariableIVX963SHA256AESGCM
  /// One ask at a time: a first ask makes the key, and a second must not make another.
  private static let lock = NSLock()
  /// The app container's mark (UserDefaults, which the container holds): this container's keys are the ones there.
  private static let containerMark = "industries.ducktape.mobile.device-key.container"
  /// `claim` ran in this process.
  private static var claimed = false

  /// Whether new seeds are sealed by the Secure Enclave: where this device has one. The simulator says it has one (an
  /// emulation on the Mac), which is no hardware this app can vouch for, so there seeds stay bare and say so.
  private static var enclave: Bool {
    #if targetEnvironment(simulator)
      return false
    #else
      return SecureEnclave.isAvailable
    #endif
  }

  /// The 32-byte Ed25519 public key under `alias`, made on first ask.
  static func publicKey(_ alias: String) throws -> Data {
    try lock.withLock {
      try claim()
      if let key = try opened(alias, { try Curve25519.Signing.PrivateKey(rawRepresentation: $0).publicKey }) {
        return key.rawRepresentation
      }
      return try make(alias)
    }
  }

  /// PureEdDSA over `message` (the caller has already applied the namespace). 64 bytes.
  static func sign(_ alias: String, _ message: Data) throws -> Data {
    try lock.withLock {
      try claim()
      guard let signature = try opened(alias, { try Curve25519.Signing.PrivateKey(rawRepresentation: $0).signature(for: message) })
      else { throw KeyError("There is no device key under \(alias).") }
      return signature
    }
  }

  /// Where the key under `alias` is kept, never a byte of it, as `enclave` decides for every key here: `sealed` by an
  /// Enclave key (`level` `secure-enclave`: the seal key the Keychain finds under the Secure Enclave's token), or a
  /// bare seed (`keychain`).
  static func storage(_ alias: String) throws -> [String: Any] {
    try lock.withLock {
      try claim()
      guard try item(service, alias) != nil else { throw KeyError("There is no device key under \(alias).") }
      guard enclave else { return ["sealed": false, "level": "keychain"] }
      guard try sealKey() != nil else { throw KeyError("The Secure Enclave key that seals \(alias) is gone.") }
      return ["sealed": true, "level": "secure-enclave"]
    }
  }

  /// The Keychain service of this device's keys: sealed ones where there is an Enclave, bare seeds where not.
  private static var service: String { enclave ? sealedService : seedService }

  /// Once a process, before any key is read: a container without the mark is a new install (or a restore without
  /// this device's Keychain), so the keys an earlier install left in the Keychain, and their seal key, are not this
  /// container's: they go, then the mark is written (in that order, so a stop between the two only deletes again).
  /// Only this store's items go (the seal key by `sealKey`'s own query); nothing else in the app keeps anything in the
  /// Keychain.
  private static func claim() throws {
    if claimed { return }
    if !UserDefaults.standard.bool(forKey: containerMark) {
      for name in [sealedService, seedService] {
        try delete([kSecClass as String: kSecClassGenericPassword, kSecAttrService as String: name])
      }
      try delete([
        kSecClass as String: kSecClassKey,
        kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
        kSecAttrApplicationTag as String: sealTag,
        kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
      ])
      UserDefaults.standard.set(true, forKey: containerMark)
    }
    claimed = true
  }

  private static func delete(_ query: [String: Any]) throws {
    let status = SecItemDelete(query as CFDictionary)
    guard status == errSecSuccess || status == errSecItemNotFound else { throw KeyError(status, "clear an earlier install's keys") }
  }

  /// `use` of the seed under `alias`, the seed zeroed after; nil when there is none yet.
  private static func opened<T>(_ alias: String, _ use: ([UInt8]) throws -> T) throws -> T? {
    guard let kept = try item(service, alias) else { return nil }
    var seed = try enclave ? unseal(alias, kept) : [UInt8](kept)
    defer { wipe(&seed) }
    return try use(seed)
  }

  private static func make(_ alias: String) throws -> Data {
    var seed = [UInt8](repeating: 0, count: 32)
    defer { wipe(&seed) }
    let status = SecRandomCopyBytes(kSecRandomDefault, seed.count, &seed)
    guard status == errSecSuccess else { throw KeyError(status, "make this device's key") }
    let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
    if enclave {
      try keep(service, alias, try seal(alias, seed))
    } else {
      var bare = Data(seed)
      defer { bare.resetBytes(in: 0..<bare.count) }
      try keep(service, alias, bare)
    }
    return key.publicKey.rawRepresentation
  }

  /// The alias, a zero byte and the seed, sealed to the Enclave key: a sealed seed opens only under its own alias.
  private static func seal(_ alias: String, _ seed: [UInt8]) throws -> Data {
    let key = try sealKey() ?? makeSealKey()
    guard let publicHalf = SecKeyCopyPublicKey(key) else { throw KeyError("The Secure Enclave key has no public half.") }
    var plain = Array(alias.utf8) + [0] + seed
    defer { wipe(&plain) }
    var error: Unmanaged<CFError>?
    guard let sealed = SecKeyCreateEncryptedData(publicHalf, ecies, Data(plain) as CFData, &error)
    else { throw KeyError(error, "seal this device's key") }
    return sealed as Data
  }

  private static func unseal(_ alias: String, _ sealed: Data) throws -> [UInt8] {
    guard let key = try sealKey() else { throw KeyError("The Secure Enclave key that seals \(alias) is gone.") }
    var error: Unmanaged<CFError>?
    guard let opened = SecKeyCreateDecryptedData(key, ecies, sealed as CFData, &error)
    else { throw KeyError(error, "open this device's key") }
    var plain = [UInt8](opened as Data)
    defer { wipe(&plain) }
    let bound = Array(alias.utf8) + [0]
    guard plain.count == bound.count + 32, plain.starts(with: bound)
    else { throw KeyError("The sealed key under \(alias) is not that alias's.") }
    return Array(plain[bound.count...])
  }

  /// The Enclave key that seals the seeds, or nil before the first is made.
  private static func sealKey() throws -> SecKey? {
    let query: [String: Any] = [
      kSecClass as String: kSecClassKey,
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrApplicationTag as String: sealTag,
      kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
      kSecReturnRef as String: true,
    ]
    var found: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &found)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let found, CFGetTypeID(found) == SecKeyGetTypeID()
    else { throw KeyError(status, "find the Secure Enclave key") }
    return (found as! SecKey)
  }

  private static func makeSealKey() throws -> SecKey {
    var error: Unmanaged<CFError>?
    guard let access = SecAccessControlCreateWithFlags(nil, kSecAttrAccessibleWhenUnlockedThisDeviceOnly, .privateKeyUsage, &error)
    else { throw KeyError(error, "make the Secure Enclave key") }
    let attributes: [String: Any] = [
      kSecAttrKeyType as String: kSecAttrKeyTypeECSECPrimeRandom,
      kSecAttrKeySizeInBits as String: 256,
      kSecAttrTokenID as String: kSecAttrTokenIDSecureEnclave,
      kSecPrivateKeyAttrs as String: [
        kSecAttrIsPermanent as String: true,
        kSecAttrApplicationTag as String: sealTag,
        kSecAttrAccessControl as String: access,
      ] as [String: Any],
    ]
    guard let key = SecKeyCreateRandomKey(attributes as CFDictionary, &error)
    else { throw KeyError(error, "make the Secure Enclave key") }
    return key
  }

  /// The Keychain item `service`/`alias`'s bytes, or nil when there is none.
  private static func item(_ service: String, _ alias: String) throws -> Data? {
    let query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: alias,
      kSecReturnData as String: true,
    ]
    var found: CFTypeRef?
    let status = SecItemCopyMatching(query as CFDictionary, &found)
    if status == errSecItemNotFound { return nil }
    guard status == errSecSuccess, let bytes = found as? Data else { throw KeyError(status, "read this device's key") }
    return bytes
  }

  /// A new Keychain item, this device only (never in a backup, never synced): whole or not at all.
  private static func keep(_ service: String, _ alias: String, _ bytes: Data) throws {
    let item: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: service,
      kSecAttrAccount as String: alias,
      kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
      kSecValueData as String: bytes,
    ]
    let status = SecItemAdd(item as CFDictionary, nil)
    guard status == errSecSuccess else { throw KeyError(status, "keep this device's key") }
  }
}

private func wipe(_ bytes: inout [UInt8]) {
  bytes.withUnsafeMutableBytes { buffer in
    if let base = buffer.baseAddress { _ = memset_s(base, buffer.count, 0, buffer.count) }
  }
}

/// What went wrong in the key store, as a sentence (what JS and the core show).
struct KeyError: LocalizedError {
  let sentence: String
  var errorDescription: String? { sentence }

  init(_ sentence: String) {
    self.sentence = sentence
  }

  init(_ status: OSStatus, _ doing: String) {
    let reason = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
    sentence = "Could not \(doing): \(reason)"
  }

  init(_ error: Unmanaged<CFError>?, _ doing: String) {
    let reason = error.map { CFErrorCopyDescription($0.takeRetainedValue()) as String } ?? "no reason given"
    sentence = "Could not \(doing): \(reason)"
  }
}

/// Writes the 32-byte public key under `alias` (made on first ask) to `out`. Null when it did; else what went wrong,
/// a C string from `strdup` the caller frees.
@_cdecl("ducktape_device_key_public_key")
public func ducktapeDeviceKeyPublicKey(_ alias: UnsafePointer<CChar>, _ out: UnsafeMutablePointer<UInt8>) -> UnsafeMutablePointer<CChar>? {
  answered {
    let key = try DeviceKeyStore.publicKey(String(cString: alias))
    guard key.count == 32 else { throw KeyError("The key store answered a \(key.count)-byte public key.") }
    key.copyBytes(to: out, count: 32)
  }
}

/// Writes PureEdDSA over `message` (`length` bytes) with the key under `alias` to `out` (64 bytes). Answers as
/// `ducktape_device_key_public_key` does.
@_cdecl("ducktape_device_key_sign")
public func ducktapeDeviceKeySign(
  _ alias: UnsafePointer<CChar>, _ message: UnsafePointer<UInt8>, _ length: Int, _ out: UnsafeMutablePointer<UInt8>
) -> UnsafeMutablePointer<CChar>? {
  answered {
    let signature = try DeviceKeyStore.sign(String(cString: alias), Data(bytes: message, count: length))
    guard signature.count == 64 else { throw KeyError("The key store answered a \(signature.count)-byte signature.") }
    signature.copyBytes(to: out, count: 64)
  }
}

private func answered(_ body: () throws -> Void) -> UnsafeMutablePointer<CChar>? {
  do {
    try body()
    return nil
  } catch {
    return strdup(error.localizedDescription)
  }
}
