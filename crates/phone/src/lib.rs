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
//! - [`shell`]: the window: the stack (Home, Open, Account under a tab bar;
//!   a program pushed over them) and Home, the network's programs.
//!
//! Nothing here names a user program; the host decides what runs.

pub mod assets;
pub mod key;
pub mod os;
pub mod shell;
