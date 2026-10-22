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
    fn ducktape_view_secret_get(
        name: *const c_char,
        value: *mut *mut u8,
        length: *mut usize,
    ) -> *mut c_char;
    fn ducktape_view_secret_set(
        name: *const c_char,
        value: *const u8,
        length: usize,
    ) -> *mut c_char;
    fn free(pointer: *mut c_void);
}

pub(super) fn secret_get(name: &str) -> KeyResult<Option<Vec<u8>>> {
    let name = c_string(name)?;
    let mut value: *mut u8 = std::ptr::null_mut();
    let mut length = 0usize;
    // SAFETY: `name` lives for the call; Swift writes `value` and `length` once.
    said(unsafe { ducktape_view_secret_get(name.as_ptr(), &mut value, &mut length) })?;
    if value.is_null() {
        return Ok(None);
    }
    // SAFETY: non-null, `length` bytes from `malloc`, ours to read and free.
    let kept = unsafe { std::slice::from_raw_parts(value, length) }.to_vec();
    // SAFETY: allocated by `malloc`, freed once, here.
    unsafe { free(value.cast()) };
    Ok(Some(kept))
}

pub(super) fn secret_set(name: &str, value: &[u8]) -> KeyResult<()> {
    let name = c_string(name)?;
    // SAFETY: `name` and `value` live for the call, and `length` is `value`'s.
    said(unsafe { ducktape_view_secret_set(name.as_ptr(), value.as_ptr(), value.len()) })
}

pub(super) fn secret_remove(name: &str) -> KeyResult<()> {
    let name = c_string(name)?;
    // SAFETY: `name` is alive for the call; a null value removes.
    said(unsafe { ducktape_view_secret_set(name.as_ptr(), std::ptr::null(), 0) })
}

pub(super) fn public_key(alias: &str) -> KeyResult<[u8; 32]> {
    let alias = c_string(alias)?;
    let mut out = [0; 32];
    // SAFETY: `alias` is a NUL-terminated string alive for the call, `out`
    // is the 32 bytes the Swift side writes (its `copyBytes(count: 32)`).
    said(unsafe { ducktape_device_key_public_key(alias.as_ptr(), out.as_mut_ptr()) })?;
    Ok(out)
}

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

pub(super) fn storage(alias: &str) -> KeyResult<(bool, String)> {
    let alias = c_string(alias)?;
    let mut sealed = 0u8;
    let mut level: *mut c_char = std::ptr::null_mut();
    // SAFETY: `alias` is alive for the call; `sealed` and `level` are this
    // frame's, written once by the Swift side.
    said(unsafe { ducktape_device_key_storage(alias.as_ptr(), &mut sealed, &mut level) })?;
    Ok((
        sealed != 0,
        copy_and_free_swift_string(level).ok_or("The key store named no level.")?,
    ))
}

fn c_string(alias: &str) -> KeyResult<CString> {
    CString::new(alias).map_err(|_| format!("The key alias `{alias}` holds a NUL byte."))
}

fn copy_and_free_swift_string(text: *mut c_char) -> Option<String> {
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

fn said(error: *mut c_char) -> KeyResult<()> {
    copy_and_free_swift_string(error).map_or(Ok(()), Err)
}
