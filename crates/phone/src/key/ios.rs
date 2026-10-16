//! The key store on iOS: the Swift `DeviceKeyStore` (`phone/ios`, the Expo
//! build's file as it is), asked over the C ABI for a public key, a
//! signature or where a key is kept. Ported from mobile
//! `crates/mobile-core/src/key/ios.rs` (dev 6f23151), plus the storage
//! call, whose `@_cdecl` sits beside the store in `phone/ios/KeyStorage.swift`
//! (the Expo build answered it to JS instead).
//!
//! Foreign functions, all defined in Swift and linked into the app beside
//! this library; any thread may call them (the store takes its own lock):
//! - `ducktape_device_key_public_key(alias, out)`: `alias` is borrowed for
//!   the call, `out` is 32 bytes the caller owns.
//! - `ducktape_device_key_sign(alias, message, length, out)`: `message` is
//!   borrowed for the call, `out` is 64 bytes the caller owns.
//! - `ducktape_device_key_storage(alias, sealed, level)`: `sealed` is one
//!   byte the caller owns; `*level` is set to a C string from `strdup`
//!   that the caller frees.
//! - each answers null when done, else a sentence from `strdup` that the
//!   caller frees with `free`.
#![allow(unsafe_code, reason = "the C ABI to the Swift key store")]

use std::ffi::{CStr, CString, c_char, c_void};

use super::KeyResult;

unsafe extern "C" {
    fn ducktape_device_key_public_key(alias: *const c_char, out: *mut u8) -> *mut c_char;
    fn ducktape_device_key_sign(
        alias: *const c_char,
        message: *const u8,
        length: usize,
        out: *mut u8,
    ) -> *mut c_char;
    fn ducktape_device_key_storage(
        alias: *const c_char,
        sealed: *mut u8,
        level: *mut *mut c_char,
    ) -> *mut c_char;
    fn free(pointer: *mut c_void);
}

/// The 32-byte public key under `alias`, made on first ask.
pub(super) fn public_key(alias: &str) -> KeyResult<[u8; 32]> {
    let alias = c_string(alias)?;
    let mut out = [0; 32];
    // SAFETY: `alias` is a NUL-terminated string alive for the call, `out`
    // is the 32 bytes the Swift side writes (its `copyBytes(count: 32)`).
    said(unsafe { ducktape_device_key_public_key(alias.as_ptr(), out.as_mut_ptr()) })?;
    Ok(out)
}

/// PureEdDSA over `message` with the key under `alias`.
pub(super) fn sign(alias: &str, message: &[u8]) -> KeyResult<[u8; 64]> {
    let alias = c_string(alias)?;
    let mut out = [0; 64];
    // SAFETY: `alias` and `message` are alive for the call and `length` is
    // `message`'s; `out` is the 64 bytes the Swift side writes.
    said(unsafe {
        ducktape_device_key_sign(
            alias.as_ptr(),
            message.as_ptr(),
            message.len(),
            out.as_mut_ptr(),
        )
    })?;
    Ok(out)
}

/// Whether the key under `alias` is sealed, and the store's word for where
/// it is kept (`secure-enclave`, `keychain`).
pub(super) fn storage(alias: &str) -> KeyResult<(bool, String)> {
    let alias = c_string(alias)?;
    let mut sealed = 0u8;
    let mut level: *mut c_char = std::ptr::null_mut();
    // SAFETY: `alias` is alive for the call; `sealed` and `level` are this
    // frame's, written once by the Swift side.
    said(unsafe { ducktape_device_key_storage(alias.as_ptr(), &mut sealed, &mut level) })?;
    Ok((
        sealed != 0,
        owned(level).ok_or("The key store named no level.")?,
    ))
}

fn c_string(alias: &str) -> KeyResult<CString> {
    CString::new(alias).map_err(|_| format!("The key alias `{alias}` holds a NUL byte."))
}

/// A `strdup` string from Swift, copied out and freed; `None` for null.
fn owned(text: *mut c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: non-null, from `strdup`: NUL-terminated and ours to free.
    let copied = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: allocated by `strdup` (malloc), freed once, here.
    unsafe { free(text.cast()) };
    Some(copied)
}

/// A call's answer: done when null, else its sentence (freed here).
fn said(error: *mut c_char) -> KeyResult<()> {
    owned(error).map_or(Ok(()), Err)
}
