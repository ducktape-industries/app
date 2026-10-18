// The Activity, the Kotlin key store, and the Rust library: `phone/Makefile`
// builds `libducktape_phone.so` per ABI into `build/rust/jniLibs` and the
// emoji face gm reads into `build/rust/assets`, and the quack every notice
// plays into `build/rust/res`, before Gradle runs.
plugins {
    id("com.android.application")
}

android {
    namespace = "industries.ducktape.mobile"
    compileSdk = 36

    defaultConfig {
        applicationId = "industries.ducktape.mobile"
        // Vulkan 1.0 is mandatory from 26; the Expo build's floor too
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDir("build/rust/jniLibs")
            assets.srcDir("build/rust/assets")
            res.srcDir("build/rust/res")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        // the Rust library is stripped by its own profile, or kept whole
        jniLibs.keepDebugSymbols += "**/libducktape_phone.so"
    }
}

dependencies {
    // Ed25519 in software, for a device whose Keystore has none (below
    // Android 13): the Expo build's key store's one dependency
    implementation("com.google.crypto.tink:tink-android:1.23.0")
    // the ring's messages (Ring.kt): Firebase Cloud Messaging, Apache-2.0
    implementation("com.google.firebase:firebase-messaging:26.0.0")
}

// The owner's Firebase project (`google-services.json`, beside this file,
// from the Firebase console): with it the build has a Firebase app and FCM
// delivers the ring; without it the build is the same app, and the ring
// comes only from its debug entry (Ring.kt).
if (file("google-services.json").exists()) {
    apply(plugin = "com.google.gms.google-services")
}
