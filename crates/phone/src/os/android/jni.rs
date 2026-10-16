//! The JNI entries of `phone/android`'s Activity (`industries.ducktape.mobile.Native`,
//! a Kotlin object whose `@JvmStatic external` functions these are), and the one
//! call back into it ([`move_task_to_back`]).
//!
//! Every entry runs on the Java UI thread and only hands on: to gm's host path
//! (its threading contract, gm `android/host.rs`: commands and atomics, never
//! gpui state) or to the host's `Inbound` ([`crate::os::send`]). None blocks
//! but `surfaceGone`, which gm makes wait until its render thread lets the
//! surface go.
//!
//! Foreign functions defined here (`Native.<name>`):
//! - `create(activity, host, files, cache)`: the Activity, gm's id for it, and
//!   its `filesDir` / `cacheDir` paths; references borrowed for the call.
//! - `surface(host, surface, scale)`, `surfaceGone(host)`: a `Surface` borrowed
//!   for the call (gm takes its own `ANativeWindow` reference).
//! - `resumed(host)`, `paused(host)`, `destroyed(host)`, `configuration(uiMode)`.
//! - `touch(action, index, pointers)`: `pointers` is `[id, x, y]` per pointer,
//!   in index order, physical pixels; copied out for the call.
//! - `key(code, action, meta)`, `insets(top, right, bottom, left, ime)` (pixels),
//!   `back()`.
//!
//! - `android_main(app)`: the entry gm's NativeActivity glue (android-activity,
//!   which gm links whatever path a host takes) names; never called, since
//!   no NativeActivity is declared (see its own doc).
//!
//! Called from here: `Activity.moveTaskToBack(boolean)` on gm's render thread,
//! through gm's own attached `Env` and the Activity reference gm keeps.
#![allow(unsafe_code, reason = "the JNI entries the Android Activity calls")]

use gpui_mobile::android::host::{self, Pointer};
use jni::objects::{JClass, JFloatArray, JObject, JString};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::{Env, EnvUnowned};
use std::path::PathBuf;

/// What an entry's failure becomes for Java: a `RuntimeException` (there is
/// none of ours; a JNI call that failed is the only way in).
type Policy = jni::errors::ThrowRuntimeExAndDefault;

/// gm's id for one Activity instance.
fn id(host: jlong) -> host::HostId {
    host as host::HostId
}

/// `Native.create`: gm learns the Activity, then starts its render thread
/// (once per process), where the host starts at the first surface and each
/// new Activity gets the phone's window.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_create<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    activity: JObject<'caller>,
    host: jlong,
    files: JString<'caller>,
    cache: JString<'caller>,
) {
    env.with_env(|env: &mut Env| -> jni::errors::Result<()> {
        gpui_mobile::android::init_logger();
        let (files, cache) = (
            PathBuf::from(files.to_string()),
            PathBuf::from(cache.to_string()),
        );
        if let Err(error) = host::set_activity(env, id(host), &activity) {
            tracing::error!(target: "ducktape::phone", %error, "gm did not take the Activity");
        }
        host::on_open_window(|_, cx| crate::shell::open_window(cx));
        host::start_with_assets(gpui_kit::assets::AllAssets, move |cx| {
            crate::os::start(files, cache, cx);
        });
        Ok(())
    })
    .resolve::<Policy>();
}

/// `Native.surface`: from `surfaceCreated` and `surfaceChanged` both.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_surface<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    host: jlong,
    surface: JObject<'caller>,
    scale: jfloat,
) {
    env.with_env(|env: &mut Env| -> jni::errors::Result<()> {
        // SAFETY: `env` is this thread's attached JNIEnv and `surface` a live
        // `android.view.Surface` reference for the call (NDK
        // `ANativeWindow_fromSurface`); the window it answers holds its own
        // reference.
        let window = unsafe {
            ndk::native_window::NativeWindow::from_surface(
                env.get_raw().cast(),
                surface.as_raw().cast(),
            )
        };
        match window {
            Some(window) => host::surface_created(id(host), window, scale),
            None => tracing::error!(target: "ducktape::phone", "the Surface gave no native window"),
        }
        Ok(())
    })
    .resolve::<Policy>();
}

/// `Native.surfaceGone`: blocks until gm's render thread has let go.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_surfaceGone(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    host: jlong,
) {
    host::surface_destroyed(id(host));
}

/// `Native.resumed`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_resumed(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    host: jlong,
) {
    host::resumed(id(host));
}

/// `Native.paused`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_paused(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    host: jlong,
) {
    host::paused(id(host));
}

/// `Native.destroyed`: the Activity is finishing for good.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_destroyed(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    host: jlong,
) {
    host::host_destroyed(id(host));
}

/// `Native.configuration`: the night mode the Activity was told of.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_configuration(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    ui_mode: jint,
) {
    host::configuration_changed(ui_mode);
}

/// `Native.touch`: a whole `MotionEvent`; `false` when it lands on a
/// platform view (none yet), for Java to hand on.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_touch<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    action: jint,
    index: jint,
    pointers: JFloatArray<'caller>,
) -> jboolean {
    env.with_env(|env: &mut Env| -> jni::errors::Result<jboolean> {
        let mut flat = vec![0.; pointers.len(env)?];
        pointers.get_region(env, 0, &mut flat)?;
        let pointers: Vec<Pointer> = flat
            .chunks_exact(3)
            .map(|pointer| Pointer {
                id: pointer[0] as i32,
                x: pointer[1],
                y: pointer[2],
            })
            .collect();
        Ok(host::motion_event(
            action as u32,
            index.max(0) as usize,
            &pointers,
        ))
    })
    .resolve::<Policy>()
}

/// `Native.key`: `action` 0 down, 1 up.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_key(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    code: jint,
    action: jint,
    meta: jint,
) {
    host::key(code, action, meta);
}

/// `Native.insets`: the system bars, the cutout and the keyboard, in pixels.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_insets(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    top: jint,
    right: jint,
    bottom: jint,
    left: jint,
    ime: jint,
) {
    super::set_insets([top, right, bottom, left], ime);
}

/// `Native.back`: the Activity's BACK, for the stack to walk.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_back(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
) {
    crate::os::send(ducktape_host::contract::Signal::Back);
}

/// The entry android-activity's NativeActivity glue calls. gm links that
/// glue on every path (its `android-activity` dependency has no feature to
/// leave it out), so the library must define the symbol to load at all;
/// this app declares no NativeActivity, so nothing calls it. The root fix
/// is gm's: make `android-activity` optional for the host-driven path.
#[unsafe(no_mangle)]
pub fn android_main(_: android_activity::AndroidApp) {
    tracing::error!(target: "ducktape::phone", "a NativeActivity started; this app has none");
}

/// Puts the app behind the others: BACK at Home's root.
pub fn move_task_to_back() {
    let moved = gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        env.call_method(
            &activity,
            jni::jni_str!("moveTaskToBack"),
            jni::jni_sig!("(Z)Z"),
            &[jni::objects::JValue::Bool(true)],
        )
        .map(drop)
        .map_err(|error| error.to_string())
    });
    if let Err(error) = moved {
        tracing::error!(target: "ducktape::phone", %error, "the app did not go behind the others");
    }
}
