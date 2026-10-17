package industries.ducktape.mobile

import android.app.Activity
import android.view.Surface

/**
 * The Rust side (`crates/phone/src/os/android/jni.rs`): every call only hands on, to gm's host path or to the
 * host's inbound signals; `surfaceGone` alone waits, until gm's render thread lets the surface go.
 */
object Native {
    init {
        System.loadLibrary("ducktape_phone")
    }

    /** gm learns [activity], under [host]; the host starts at the first surface, over [files] and [cache]. */
    @JvmStatic external fun create(activity: Activity, host: Long, files: String, cache: String)

    @JvmStatic external fun surface(host: Long, surface: Surface, scale: Float)

    @JvmStatic external fun surfaceGone(host: Long)

    @JvmStatic external fun resumed(host: Long)

    @JvmStatic external fun paused(host: Long)

    @JvmStatic external fun destroyed(host: Long)

    @JvmStatic external fun configuration(uiMode: Int)

    /** A whole `MotionEvent`: `[id, x, y]` per pointer in index order; false when it is not gpui's. */
    @JvmStatic external fun touch(action: Int, index: Int, pointers: FloatArray): Boolean

    @JvmStatic external fun key(code: Int, action: Int, meta: Int)

    /** The system bars and cutout, and the keyboard over the bottom edge, in pixels. */
    @JvmStatic external fun insets(top: Int, right: Int, bottom: Int, left: Int, ime: Int)

    @JvmStatic external fun back()

    /**
     * One update from the [MainActivity] `InputConnection` for gm's text input: [kind] 0 composing, 1 commit,
     * 2 delete around, 3 delete around in code points, 4 done, 5 dismissed by BACK.
     */
    @JvmStatic external fun ime(session: Long, kind: Int, text: String, start: Int, end: Int)

    /** The OS asked the app to give memory back (`onTrimMemory`). */
    @JvmStatic external fun memoryWarning()

    /** A `duck://` or `ducktape://` link an intent carried. */
    @JvmStatic external fun link(url: String)

    /**
     * A program's notice shown ([Ring]): its payload as JSON, when it was shown (Unix seconds); [front] when it came
     * while the app was in front (nothing shown), [opened] when the person opened it.
     */
    @JvmStatic external fun notice(payload: String, at: Long, front: Boolean, opened: Boolean)

    /** This device's push token of [kind] (`fcm`), for the device registry; empty when withdrawn. */
    @JvmStatic external fun pushToken(kind: String, token: String)
}
