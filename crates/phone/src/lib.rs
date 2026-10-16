//! The phone shell: the blind host core in one full-screen window.
//!
//! - [`os`]: what the phone gives the host (`contract::OsServices`): the
//!   app container's directories, this device's key, the safe area and the
//!   software keyboard, the facts and limits of a phone. ONE file names the
//!   OS, with an arm per phone (`os/ios`, `os/android`); each arm is also
//!   where its launcher enters (the C ABI `main` on iOS, the Activity's JNI
//!   entries on Android).
//! - [`key`]: this device's key in the OS key store, the Expo build's own
//!   store (`phone/ios/DeviceKeyStore.swift`, `phone/android/.../DeviceKeyStore.kt`)
//!   under the Expo build's aliases, so a phone that ran the Expo app keeps
//!   its key and its account.
//! - [`assets`]: gpui-kit's catalog with its Lucide icons at the Expo
//!   build's stroke.
//! - [`shell`]: the window: the stack (Home and Open under a tab bar; a
//!   program on top, its view seated full screen under the capsule), the
//!   sheets over it (the account menu, the capsule's menu, the consent
//!   sheet), Back, the links the OS hands over, memory pressure.
//!
//! Nothing here names a user program; the host decides what runs.

pub mod assets;
pub mod key;
pub mod os;
pub mod shell;
