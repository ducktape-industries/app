package industries.ducktape.mobile

import android.app.Activity
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowManager
import android.window.OnBackInvokedDispatcher
import industries.ducktape.devicekey.DeviceKeyStore
import java.io.File

/**
 * The phone's one Activity: a `SurfaceView` gpui draws into (gm's host-driven path), and everything the OS tells
 * an Activity handed to Rust ([Native]). It draws nothing and decides nothing itself.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {
    /** gm's id for this instance: a new instance gets a new window, this one keeps its own. */
    private val host = next++

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        // the key store first: loading its class loads the library and hands Rust the class
        DeviceKeyStore.sealedDir = File(noBackupFilesDir, "device-keys")
        check(DeviceKeyStore.started)
        Native.create(this, host, filesDir.path, cacheDir.path)
        edgeToEdge()
        val surface = SurfaceView(this)
        surface.holder.addCallback(this)
        surface.setOnApplyWindowInsetsListener { _, insets -> handOn(insets) }
        setContentView(surface)
        if (Build.VERSION.SDK_INT >= 33) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT) {
                Native.back()
            }
        }
    }

    /** The app draws under the system bars and the cutout; the insets say where they are. */
    @Suppress("DEPRECATION")
    private fun edgeToEdge() {
        if (Build.VERSION.SDK_INT >= 30) {
            window.setDecorFitsSystemWindows(false)
        } else {
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
        }
        window.statusBarColor = Color.TRANSPARENT
        window.navigationBarColor = Color.TRANSPARENT
        if (Build.VERSION.SDK_INT >= 28) {
            window.attributes.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
    }

    @Suppress("DEPRECATION")
    private fun handOn(insets: WindowInsets): WindowInsets {
        if (Build.VERSION.SDK_INT >= 30) {
            val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
            val ime = insets.getInsets(WindowInsets.Type.ime()).bottom
            Native.insets(bars.top, bars.right, bars.bottom, bars.left, ime)
        } else {
            // below 30 the system window insets already hold the keyboard (adjustNothing leaves them to us)
            Native.insets(insets.systemWindowInsetTop, insets.systemWindowInsetRight, insets.systemWindowInsetBottom,
                insets.systemWindowInsetLeft, 0)
        }
        return insets
    }

    override fun surfaceCreated(holder: SurfaceHolder) = Native.surface(host, holder.surface, density())

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) =
        Native.surface(host, holder.surface, density())

    override fun surfaceDestroyed(holder: SurfaceHolder) = Native.surfaceGone(host)

    private fun density() = resources.displayMetrics.density

    override fun onResume() {
        super.onResume()
        Native.resumed(host)
    }

    override fun onPause() {
        Native.paused(host)
        super.onPause()
    }

    override fun onDestroy() {
        if (isFinishing) Native.destroyed(host)
        super.onDestroy()
    }

    override fun onConfigurationChanged(config: Configuration) {
        super.onConfigurationChanged(config)
        Native.configuration(config.uiMode)
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        val pointers = FloatArray(event.pointerCount * 3)
        for (i in 0 until event.pointerCount) {
            pointers[i * 3] = event.getPointerId(i).toFloat()
            pointers[i * 3 + 1] = event.getX(i)
            pointers[i * 3 + 2] = event.getY(i)
        }
        return Native.touch(event.actionMasked, event.actionIndex, pointers) || super.dispatchTouchEvent(event)
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val handed = event.keyCode != KeyEvent.KEYCODE_BACK &&
            (event.action == KeyEvent.ACTION_DOWN || event.action == KeyEvent.ACTION_UP)
        if (!handed) return super.dispatchKeyEvent(event)
        Native.key(event.keyCode, event.action, event.metaState)
        return true
    }

    /** Below 33 BACK comes here; from 33 the callback registered in [onCreate] takes it. */
    @Deprecated("the platform's own, below 33")
    override fun onBackPressed() = Native.back()

    private companion object {
        var next = 1L
    }
}
