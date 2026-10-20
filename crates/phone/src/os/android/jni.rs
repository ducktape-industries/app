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
//! - `ime(session, kind, text, start, end)`: the Activity's `InputConnection`
//!   (gm's `GpuiInputActivity` contract, gm `android/host.rs` `ime_event`).
//! - `memoryWarning()`: `onTrimMemory` at a level that is memory pressure.
//! - `link(url)`: a `duck://` or `ducktape://` intent's data.
//! - `pushToken(kind, token)`: this device's push token (`fcm`), for the
//!   device registry; empty when withdrawn.
//! - `notice(payload, at, front, opened) -> shows`: a program's notice
//!   `Ring.kt` showed, opened or handed over in front; from Firebase's own
//!   thread for one that came in front (it only hands on, through a
//!   channel, and answers from what the shell published whether the shade
//!   shows it).
//! - `swipeBegins(x, y, dx, dy, held) -> begins`: whether the Activity's
//!   tabs' swipe starts, read against the last draw; `swipeDrag(x, speed)`,
//!   `swipeLetGo()`, `swipeCancel()`: that swipe under way, let go, ended.
//!
//! - `android_main(app)`: the entry gm's NativeActivity glue (android-activity,
//!   which gm links whatever path a host takes) names; never called, since
//!   no NativeActivity is declared (see its own doc).
//!
//! Called from here, through gm's own attached `Env` and the Activity reference
//! gm keeps: `Activity.moveTaskToBack(boolean)` and
//! `MainActivity.systemBars(dark)` on gm's render thread, and
//! `MainActivity.ask(permission)` (blocking until the person answers) and
//! `MainActivity.openSettings(page)` on a camera's or a microphone's thread,
//! `MainActivity.pickFiles(multiple)` and `MainActivity.keepFile(path)`
//! (each blocking until the person answers) on a thread of their own,
//! `MainActivity.banner(payload)` on the host's banner queue's, and
//! `MainActivity.withdrawNotice(tag)` on any.
#![allow(unsafe_code, reason = "the JNI entries the Android Activity calls")]

use gpui_mobile::android::host::{self, Pointer};
use jni::objects::{JClass, JFloatArray, JObject, JString, JValue};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::{Env, EnvUnowned};
use std::path::PathBuf;
use std::time::Duration;

type Policy = jni::errors::ThrowRuntimeExAndDefault;

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
        if let Err(error) = hand_context(env, &activity) {
            tracing::error!(target: "ducktape::phone", %error, "no Context for the camera and the microphone");
        }
        ducktape_av::asks_through(ask);
        host::on_open_window(|_, cx| crate::shell::open_window(cx));
        host::start_with_assets(crate::assets::PhoneAssets, move |cx| {
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

/// `Native.swipeBegins`: whether the tabs' swipe begins (`MainActivity`, at
/// the touch slop): its finger came down at (`x`, `y`) in the window's
/// points, went (`dx`, `dy`) points to the slop, and came down `held`
/// seconds ago. Android's Back is the system's: no swipe back begins here
/// (`shell::swipe::pages`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_swipeBegins(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    x: jfloat,
    y: jfloat,
    dx: jfloat,
    dy: jfloat,
    held: jfloat,
) -> jboolean {
    use gpui_kit::{point, px};
    let held = Duration::try_from_secs_f32(held).unwrap_or(Duration::MAX);
    crate::shell::swipe::pages(point(px(x), px(y)), point(px(dx), px(dy)), held)
}

/// `Native.swipeDrag`: the tabs' swipe under way, `x` points to the right
/// of where it began (negative to the left), moving `speed` points per
/// second to the right.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_swipeDrag(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
    x: jfloat,
    speed: jfloat,
) {
    crate::os::send(ducktape_host::contract::Signal::Swipe { x, speed });
}

/// `Native.swipeLetGo`: that swipe let go, where and as fast as its last
/// drag said: the shell judges which tab it lands on.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_swipeLetGo(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
) {
    crate::os::send(ducktape_host::contract::Signal::SwipeLetGo);
}

/// `Native.swipeCancel`: that swipe ended without going on (the OS
/// cancelled the touch).
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_swipeCancel(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
) {
    crate::os::send(ducktape_host::contract::Signal::SwipeCancelled);
}

/// `Native.ime`: one update from the Activity's `InputConnection`, for gm's
/// text input. `kind`: 0 composing, 1 commit, 2 delete around, 3 delete
/// around in code points, 4 done, 5 dismissed by BACK. Java clamps the
/// selection to 0 and up; a negative one reads as 0 here too.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_ime<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    session: jlong,
    kind: jint,
    text: JString<'caller>,
    start: jint,
    end: jint,
) {
    env.with_env(|_: &mut Env| -> jni::errors::Result<()> {
        host::ime_event(
            session as u64,
            kind,
            text.to_string(),
            start.max(0) as usize,
            end.max(0) as usize,
        );
        Ok(())
    })
    .resolve::<Policy>();
}

/// `Native.memoryWarning`: the OS asked the app to give memory back.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_memoryWarning(
    _: EnvUnowned<'_>,
    _: JClass<'_>,
) {
    crate::os::send(ducktape_host::contract::Signal::MemoryWarning);
}

/// `Native.link`: a link the OS handed the app (an intent's data).
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_link<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    url: JString<'caller>,
) {
    env.with_env(|_: &mut Env| -> jni::errors::Result<()> {
        crate::os::link(url.to_string());
        Ok(())
    })
    .resolve::<Policy>();
}

/// `Native.pushToken`: this device's push token of `kind` (`Ring.kt`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_pushToken<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    kind: JString<'caller>,
    token: JString<'caller>,
) {
    env.with_env(|_: &mut Env| -> jni::errors::Result<()> {
        crate::os::push_token(&kind.to_string(), token.to_string());
        Ok(())
    })
    .resolve::<Policy>();
}

/// `Native.notice`: a program's notice the OS showed (`Ring.kt`), its
/// payload as JSON, the Unix seconds it was shown at; `front` when it came
/// while the app was in front, `opened` when the person opened it
/// ([`crate::os::os_notice`]). Whether the shade shows one that came in
/// front.
#[unsafe(no_mangle)]
pub extern "system" fn Java_industries_ducktape_mobile_Native_notice<'caller>(
    mut env: EnvUnowned<'caller>,
    _: JClass<'caller>,
    payload: JString<'caller>,
    at: jlong,
    front: jboolean,
    opened: jboolean,
) -> jboolean {
    env.with_env(|_: &mut Env| -> jni::errors::Result<jboolean> {
        Ok(crate::os::os_notice(
            &payload.to_string(),
            at,
            front,
            opened,
        ))
    })
    .resolve::<Policy>()
}

/// This phone's own banner, its payload as JSON (`os::banner_payload`),
/// up through the Activity (`MainActivity.banner`, `Ring.kt`'s notice):
/// whether it was handed to the shade.
pub fn banner(payload: &str) -> bool {
    let shown = gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        let payload = env.new_string(payload).map_err(|error| error.to_string())?;
        env.call_method(
            &activity,
            jni::jni_str!("banner"),
            jni::jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::Object(&payload)],
        )
        .and_then(|shown| shown.z())
        .map_err(|error| error.to_string())
    });
    shown.unwrap_or_else(|error| {
        tracing::error!(target: "ducktape::phone", %error, "a banner did not go up");
        false
    })
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

fn hand_context(env: &mut Env, activity: &JObject) -> jni::errors::Result<()> {
    static HANDED: std::sync::Once = std::sync::Once::new();
    if HANDED.is_completed() {
        return Ok(());
    }
    let vm = env.get_java_vm()?;
    let app = env
        .call_method(
            activity,
            jni::jni_str!("getApplicationContext"),
            jni::jni_sig!("()Landroid/content/Context;"),
            &[],
        )?
        .l()?;
    let app = env.new_global_ref(&app)?;
    HANDED.call_once(|| {
        // SAFETY: the live VM, and a global reference to the application
        // Context kept for the process (never deleted: forgotten below),
        // handed before any camera or microphone thread or cpal call reads
        // them (the host starts after this entry returns)
        unsafe {
            ndk_context::initialize_android_context(vm.get_raw().cast(), app.as_raw().cast());
        }
        std::mem::forget(app);
    });
    Ok(())
}

fn ask(permission: &str) -> bool {
    let asked = gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        let permission = env
            .new_string(permission)
            .map_err(|error| error.to_string())?;
        env.call_method(
            &activity,
            jni::jni_str!("ask"),
            jni::jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::Object(&permission)],
        )
        .and_then(|granted| granted.z())
        .map_err(|error| error.to_string())
    });
    asked.unwrap_or_else(|error| {
        tracing::error!(target: "ducktape::phone", %error, "the permission was not asked");
        false
    })
}

/// The system's open-document dialog, through the Activity
/// (`MainActivity.pickFiles`): the paths of the Activity's copies of the
/// files picked, `None` when the person cancelled. Blocks the calling
/// thread (never the UI one) until the person answers.
pub fn pick_files(multiple: bool) -> Result<Option<Vec<PathBuf>>, String> {
    gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        let picked = env
            .call_method(
                &activity,
                jni::jni_str!("pickFiles"),
                jni::jni_sig!("(Z)[Ljava/lang/String;"),
                &[JValue::Bool(multiple)],
            )
            .and_then(|picked| picked.l())
            .map_err(|error| {
                env.exception_clear();
                error.to_string()
            })?;
        if picked.is_null() {
            return Ok(None);
        }
        // SAFETY: the `String[]` the method returned, a local reference
        // alive for this call
        let picked =
            unsafe { jni::objects::JObjectArray::<JObject>::from_raw(env, picked.as_raw()) };
        let count = picked.len(env).map_err(|error| error.to_string())?;
        let mut paths = Vec::with_capacity(count);
        for index in 0..count {
            let path: JObject = picked
                .get_element(env, index)
                .map_err(|error| error.to_string())?;
            paths.push(PathBuf::from(gpui_mobile::android::jni::get_string(
                env, &path,
            )));
        }
        Ok(Some(paths))
    })
}

/// `path` to the system's create-document dialog, through the Activity
/// (`MainActivity.keepFile`), which copies it where the person chose:
/// whether they did, `false` when they cancelled. Blocks the calling
/// thread (never the UI one) until the person answers.
pub fn keep_file(path: &std::path::Path) -> Result<bool, String> {
    gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        let text = path.to_string_lossy();
        let path = env
            .new_string(text.as_ref())
            .map_err(|error| error.to_string())?;
        env.call_method(
            &activity,
            jni::jni_str!("keepFile"),
            jni::jni_sig!("(Ljava/lang/String;)Z"),
            &[JValue::Object(&path)],
        )
        .and_then(|kept| kept.z())
        .map_err(|error| {
            env.exception_clear();
            error.to_string()
        })
    })
}

/// The system bars' icons over the app's appearance, through the Activity
/// (`MainActivity.systemBars`): light icons over a `dark` app, dark ones
/// over a light one.
pub fn system_bars(dark: bool) {
    let set = gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        env.call_method(
            &activity,
            jni::jni_str!("systemBars"),
            jni::jni_sig!("(Z)V"),
            &[JValue::Bool(dark)],
        )
        .map(drop)
        .map_err(|error| error.to_string())
    });
    if let Err(error) = set {
        tracing::error!(target: "ducktape::phone", %error, "the system bars did not follow the appearance");
    }
}

/// The shade's notices under `tag` (the push server's) taken down, through
/// the Activity (`MainActivity.withdrawNotice`, `Ring.withdraw`).
pub fn withdraw_notice(tag: &str) {
    let withdrawn = gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        let tag = env.new_string(tag).map_err(|error| error.to_string())?;
        env.call_method(
            &activity,
            jni::jni_str!("withdrawNotice"),
            jni::jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&tag)],
        )
        .map(drop)
        .map_err(|error| error.to_string())
    });
    if let Err(error) = withdrawn {
        tracing::error!(target: "ducktape::phone", %error, "a notice stayed in the shade");
    }
}

/// Brings up the app's own page in Settings, `page` being its `package:`
/// URI (`ducktape_av::settings_page`), where a refused camera or microphone
/// is allowed.
pub fn open_settings(page: &str) {
    let opened = gpui_mobile::android::jni::with_env(|env| {
        let activity = gpui_mobile::android::jni::activity(env)?;
        let page = env.new_string(page).map_err(|error| error.to_string())?;
        env.call_method(
            &activity,
            jni::jni_str!("openSettings"),
            jni::jni_sig!("(Ljava/lang/String;)V"),
            &[JValue::Object(&page)],
        )
        .map(drop)
        .map_err(|error| error.to_string())
    });
    if let Err(error) = opened {
        tracing::error!(target: "ducktape::phone", %error, "the app's settings did not open");
    }
}
