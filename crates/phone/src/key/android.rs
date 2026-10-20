//! The key store on Android: Android Keystore through the Kotlin
//! `DeviceKeyStore` (`phone/android`, the Expo build's file with only its
//! library's name changed), asked over JNI for a public key, a signature or
//! where a key is kept. Ported from mobile
//! `crates/mobile-core/src/key/android.rs` (dev 6f23151) onto jni 0.22, gm's.
//!
//! Foreign functions:
//! - defined here: `DeviceKeyStore.attach()`, which Kotlin calls once from
//!   the class's `init`, on whichever thread first touches the class; it
//!   keeps the VM and a global reference to the class (a thread Rust started
//!   sees only the system class loader).
//! - called from here, on any thread (attached for good on first call; the
//!   store takes its own lock): the static `publicKey(String): byte[]`,
//!   `sign(String, byte[]): byte[]` and `storage(String): Map`. Every local
//!   reference lives in the attach call's own frame.
#![allow(unsafe_code, reason = "the JNI entry the Kotlin key store calls")]

use super::KeyResult;
use jni::objects::{JByteArray, JClass, JObject, JString, JValue};
use jni::refs::Global;
use jni::{Env, EnvUnowned, JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;

struct Store {
    vm: JavaVM,
    class: Global<JClass<'static>>,
}

static STORE: OnceLock<Store> = OnceLock::new();

/// `DeviceKeyStore.attach()`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_devicekey_DeviceKeyStore_attach<'caller>(
    mut env: EnvUnowned<'caller>,
    class: JClass<'caller>,
) {
    env.with_env(|env: &mut Env| -> jni::errors::Result<()> {
        let store = Store {
            vm: env.get_java_vm()?,
            class: env.new_global_ref(&class)?,
        };
        STORE.get_or_init(|| store);
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

pub(super) fn public_key(alias: &str) -> KeyResult<[u8; 32]> {
    let bytes = ask(|env, class| {
        let alias = env.new_string(alias)?;
        let answer = env
            .call_static_method(
                class,
                jni_str!("publicKey"),
                jni_sig!("(Ljava/lang/String;)[B"),
                &[JValue::Object(&alias)],
            )?
            .l()?;
        bytes_of(env, answer)
    })?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("Keystore answered a {}-byte public key", bytes.len()))
}

pub(super) fn sign(alias: &str, message: &[u8]) -> KeyResult<[u8; 64]> {
    let bytes = ask(|env, class| {
        let alias = env.new_string(alias)?;
        let message = env.byte_array_from_slice(message)?;
        let answer = env
            .call_static_method(
                class,
                jni_str!("sign"),
                jni_sig!("(Ljava/lang/String;[B)[B"),
                &[JValue::Object(&alias), JValue::Object(&message)],
            )?
            .l()?;
        bytes_of(env, answer)
    })?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| format!("Keystore answered a {}-byte signature", bytes.len()))
}

pub(super) fn storage(alias: &str) -> KeyResult<(bool, String)> {
    ask(|env, class| {
        let alias = env.new_string(alias)?;
        let map = env
            .call_static_method(
                class,
                jni_str!("storage"),
                jni_sig!("(Ljava/lang/String;)Ljava/util/Map;"),
                &[JValue::Object(&alias)],
            )?
            .l()?;
        let sealed = field(env, &map, "sealed")?;
        let sealed = env
            .call_method(&sealed, jni_str!("booleanValue"), jni_sig!("()Z"), &[])?
            .z()?;
        let level = field(env, &map, "level")?;
        let level = env.cast_local::<JString>(level)?.to_string();
        Ok((sealed, level))
    })
}

fn field<'local>(
    env: &mut Env<'local>,
    map: &JObject<'_>,
    name: &str,
) -> jni::errors::Result<JObject<'local>> {
    let name = env.new_string(name)?;
    env.call_method(
        map,
        jni_str!("get"),
        jni_sig!("(Ljava/lang/Object;)Ljava/lang/Object;"),
        &[JValue::Object(&name)],
    )?
    .l()
}

fn bytes_of(env: &mut Env<'_>, answer: JObject<'_>) -> jni::errors::Result<Vec<u8>> {
    let array = env.cast_local::<JByteArray>(answer)?;
    env.convert_byte_array(&array)
}

fn ask<T>(
    call: impl FnOnce(&mut Env<'_>, &JClass<'static>) -> jni::errors::Result<T>,
) -> KeyResult<T> {
    let store = STORE
        .get()
        .ok_or("This device's key store has not started yet.")?;
    store
        .vm
        .attach_current_thread(|env| call(env, &store.class))
        .map_err(|error| match error {
            jni::errors::Error::CaughtJavaException { msg, .. } => msg,
            other => other.to_string(),
        })
}
