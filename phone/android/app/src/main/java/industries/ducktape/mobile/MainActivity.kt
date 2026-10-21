package industries.ducktape.mobile

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ComponentCallbacks2
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
import android.provider.Settings
import android.text.Editable
import android.text.InputType
import android.text.Selection
import android.text.TextWatcher
import android.util.Log
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.VelocityTracker
import android.view.View
import android.view.ViewConfiguration
import android.view.ViewGroup
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputConnectionWrapper
import android.view.inputmethod.InputMethodManager
import android.webkit.MimeTypeMap
import android.widget.EditText
import android.window.OnBackInvokedDispatcher
import industries.ducktape.devicekey.DeviceKeyStore
import java.io.File
import java.util.concurrent.ArrayBlockingQueue
import kotlin.math.hypot

/**
 * The phone's one Activity: a `SurfaceView` gpui draws into (gm's host-driven path), and everything the OS tells
 * an Activity handed to Rust ([Native]). It draws nothing and decides nothing itself. The keyboard's methods
 * (`gpuiShowKeyboard`, `gpuiHideKeyboard`, `gpuiResetComposition`) are gm's to call, and an invisible field holds
 * the keyboard's connection for it: gm's `GpuiInputActivity` contract, ported.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {
    /** gm's id for this instance: a new instance gets a new window, this one keeps its own. */
    private val host = next++

    override fun onCreate(state: Bundle?) {
        // a walk's `am start -e DUCKTAPE_PERF 1`: an app is launched with no environment, so the extra becomes one,
        // before the library loads and Rust reads it (`perf::start`)
        intent?.getStringExtra("DUCKTAPE_PERF")?.let { android.system.Os.setenv("DUCKTAPE_PERF", it, true) }
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
        // the launching link once: not again when the Activity is made again (a font or locale change) or the app is
        // relaunched from Recents, which hands the old intent back
        if (state == null && (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY) == 0) handOn(intent)
        if (state == null) {
            Ring.askOnce(this)
            intent?.getStringExtra("DUCKTAPE_RING_TEST")?.let { Ring.test(applicationContext, it) }
        }
        Ring.token(applicationContext)
    }

    /** `singleTask`: a link while the app runs comes here, not to a new instance. */
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // what `intent` reads from now on, so a re-creation finds this one, not the launch's
        setIntent(intent)
        handOn(intent)
    }

    private fun handOn(intent: Intent?) {
        // a call's ring and a notice's row are this app's own word, read only by the entry no other app can start: no
        // other app takes a ring down or writes a row
        val own = intent?.component?.className == Ring.OWN
        if (!own && (intent?.hasExtra(Ring.TAG) == true || intent?.hasExtra(Ring.PAYLOAD) == true)) {
            Log.w("ducktape", "a ring's or a notice's extra on an intent this app did not make: not read")
        }
        // a call's Answer, or a tap on its notification: its ring goes
        intent?.getStringExtra(Ring.TAG)?.takeIf { own }?.let {
            Log.i("ducktape", "a call's ring taken down: $it")
            Ring.withdraw(this, it)
        }
        // a program's notice opened from the shade: its row read, before its link opens
        intent?.getStringExtra(Ring.PAYLOAD)?.takeIf { own }?.let {
            Log.i("ducktape", "a notice opened from the shade: its row read")
            Native.notice(it, intent.getLongExtra(Ring.AT, 0), false, true)
        }
        val url = intent?.takeIf { it.action == Intent.ACTION_VIEW }?.dataString ?: return
        Native.link(url)
    }

    /** Memory pressure, not the UI going out of sight (`TRIM_MEMORY_UI_HIDDEN`, which the lifecycle says). */
    @Suppress("DEPRECATION")
    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        if (level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_LOW && level != ComponentCallbacks2.TRIM_MEMORY_UI_HIDDEN) {
            Native.memoryWarning()
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

    /** This phone's own banner of a program's notice (Rust's `Banners::post`, the app in front): [Ring.banner]. */
    fun banner(payload: String): Boolean = Ring.banner(this, payload)

    /**
     * The system bars' icons over the app's appearance (Rust `os::system_bars`, from App settings › Appearance or
     * the system's): light icons over a [dark] app, dark ones over a light one. The theme's `values-night` sets them
     * at launch from the system alone.
     */
    @Suppress("DEPRECATION")
    fun systemBars(dark: Boolean) = runOnUiThread {
        if (Build.VERSION.SDK_INT >= 30) {
            val light = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or
                WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS
            window.insetsController?.setSystemBarsAppearance(if (dark) 0 else light, light)
        } else {
            val light = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR or View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR
            val shown = window.decorView.systemUiVisibility
            window.decorView.systemUiVisibility = if (dark) shown and light.inv() else shown or light
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
        Ring.inFront = true
        Native.resumed(host)
        Ring.shown(this)
    }

    override fun onPause() {
        Ring.inFront = false
        Native.paused(host)
        super.onPause()
    }

    override fun onDestroy() {
        if (isFinishing) {
            Native.destroyed(host)
            // an ask still out is refused: no instance is left to hear the answer
            asked?.offer(false)
            picked?.offer(emptyArray())
            kept?.offer(false)
        }
        super.onDestroy()
    }

    /**
     * A camera's or a microphone's ask (`crates/phone/src/os/android/jni.rs` `ask`), on its own thread, never the UI
     * one: whether [permission] is granted, the system's prompt raised first when it is not. Blocks until the person
     * answers. One ask at a time: Android drops a prompt asked while another shows, so a camera and a microphone
     * opening together are asked in turn.
     */
    fun ask(permission: String): Boolean {
        synchronized(asking) {
            if (checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED) return true
            // an instance on its way out hears no answer: refused, never waited on
            if (isFinishing || isDestroyed) return false
            val answer = ArrayBlockingQueue<Boolean>(1)
            asked = answer
            runOnUiThread { requestPermissions(arrayOf(permission), ASK) }
            return answer.take().also { asked = null }
        }
    }

    /** The person's answer, to the instance on screen then (one made again since the ask hears it too). */
    override fun onRequestPermissionsResult(code: Int, permissions: Array<out String>, results: IntArray) {
        if (code == ASK) asked?.offer(results.firstOrNull() == PackageManager.PERMISSION_GRANTED)
        if (code == Ring.ASK_NOTIFY && results.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            Ring.askFullScreenOnce(this)
        }
    }

    /**
     * `file.pick` (`crates/phone/src/os/android/jni.rs` `pick_files`), on Rust's own thread, never the UI one: the
     * system's open-document dialog, each file picked copied into a folder of its own under the cache for Rust to open
     * (Rust removes the copy once it holds it open); null when the person cancelled. Blocks until the person answers.
     */
    fun pickFiles(multiple: Boolean): Array<String>? {
        synchronized(picking) {
            if (isFinishing || isDestroyed) return null
            val answer = ArrayBlockingQueue<Array<String>>(1)
            picked = answer
            runOnUiThread {
                val open = Intent(Intent.ACTION_OPEN_DOCUMENT)
                    .addCategory(Intent.CATEGORY_OPENABLE)
                    .setType("*/*")
                    .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, multiple)
                startActivityForResult(open, PICK)
            }
            return answer.take().also { picked = null }.takeIf { it.isNotEmpty() }
        }
    }

    /**
     * `file.save` (`jni.rs` `keep_file`), on Rust's own thread. A picture opens in the system's viewer, through
     * [Kept] (its share and save are the viewer's); any other file goes to the system's create-document dialog, named
     * and typed as the file at [path] is, and is copied where the person chose. Whether it was kept (a picture: shown);
     * false when they cancelled. Blocks until the viewer closes or the person answers. Rust removes the file once it
     * hears.
     */
    fun keepFile(path: String, ticket: Long): Boolean {
        synchronized(keeping) {
            if (isFinishing || isDestroyed) return false
            val file = File(path)
            val answer = ArrayBlockingQueue<Boolean>(1)
            kept = answer
            keptFile = file
            runOnUiThread {
                if (!Native.keepShows(ticket)) {
                    answer.offer(false)
                    return@runOnUiThread
                }
                val type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase())
                    ?: "application/octet-stream"
                if (type.startsWith("image/")) {
                    val view = Intent(Intent.ACTION_VIEW)
                        .setDataAndType(Kept.uri(file), type)
                        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    try {
                        startActivityForResult(view, VIEW)
                    } catch (missing: ActivityNotFoundException) {
                        Log.w("ducktape", "no viewer for a picture", missing)
                        kept?.offer(false)
                    }
                    return@runOnUiThread
                }
                val create = Intent(Intent.ACTION_CREATE_DOCUMENT)
                    .addCategory(Intent.CATEGORY_OPENABLE)
                    .setType(type)
                    .putExtra(Intent.EXTRA_TITLE, file.name)
                startActivityForResult(create, KEEP)
            }
            return answer.take().also {
                kept = null
                keptFile = null
            }
        }
    }

    /** A pick's or a keep's answer; the copying is off the UI thread, since a file may be large. */
    @Deprecated("the platform's own Activity result: this Activity uses no AndroidX")
    override fun onActivityResult(code: Int, result: Int, data: Intent?) {
        when (code) {
            PICK -> Thread {
                val copies = if (result == RESULT_OK && data != null) copies(data) else emptyArray()
                picked?.offer(copies)
            }.start()
            // the viewer closed: the picture was shown
            VIEW -> kept?.offer(true)
            KEEP -> Thread {
                val target = data?.data
                val file = keptFile
                kept?.offer(result == RESULT_OK && target != null && file != null && copyOut(file, target))
            }.start()
            else -> @Suppress("DEPRECATION") super.onActivityResult(code, result, data)
        }
    }

    /** Each document [data] names, copied into a folder of its own under the cache: their paths. */
    private fun copies(data: Intent): Array<String> {
        val clip = data.clipData
        val uris = clip?.let { (0 until it.itemCount).map { at -> it.getItemAt(at).uri } } ?: listOfNotNull(data.data)
        return try {
            uris.map { uri ->
                val folder = File(cacheDir, "picked/" + java.util.UUID.randomUUID()).apply { mkdirs() }
                val copy = File(folder, File(displayName(uri) ?: "file").name)
                contentResolver.openInputStream(uri)!!.use { input -> copy.outputStream().use { input.copyTo(it) } }
                copy.path
            }.toTypedArray()
        } catch (failed: Exception) {
            // a document that cannot be read: the pick is cancelled, and the log says why
            Log.w("ducktape", "a picked file was not copied", failed)
            emptyArray()
        }
    }

    /** The name the document at [uri] shows, if its provider says. */
    private fun displayName(uri: Uri): String? =
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }

    /** [file] copied to the document at [target]: whether it was. */
    private fun copyOut(file: File, target: Uri): Boolean = try {
        contentResolver.openOutputStream(target)?.use { out -> file.inputStream().use { it.copyTo(out) } } != null
    } catch (failed: Exception) {
        Log.w("ducktape", "a file was not kept", failed)
        false
    }


    /** The shade's notices under [tag] taken down (Rust: its row read or cleared, or its program saw it). Any thread. */
    fun withdrawNotice(tag: String) = runOnUiThread { Ring.withdraw(this, tag) }

    /** The app's own page in Settings, [page] its `package:` URI: where a refused camera or microphone is allowed. */
    fun openSettings(page: String) = runOnUiThread {
        try {
            startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse(page)))
        } catch (missing: ActivityNotFoundException) {
            // a build of Android without the page: the Denied card's words still say where to go
            Log.w("ducktape", "no settings page for the app", missing)
        }
    }

    override fun onConfigurationChanged(config: Configuration) {
        super.onConfigurationChanged(config)
        Native.configuration(config.uiMode)
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        if (swiped(event)) return true
        return Native.touch(event.actionMasked, event.actionIndex, pointers(event)) || super.dispatchTouchEvent(event)
    }

    /** [event]'s pointers as [Native.touch] takes them: `[id, x, y]` each, in index order. */
    private fun pointers(event: MotionEvent): FloatArray {
        val pointers = FloatArray(event.pointerCount * 3)
        for (i in 0 until event.pointerCount) {
            pointers[i * 3] = event.getPointerId(i).toFloat()
            pointers[i * 3 + 1] = event.getX(i)
            pointers[i * 3 + 2] = event.getY(i)
        }
        return pointers
    }

    /** The finger's speed while a gesture may be the tabs' swipe; obtained at its first touch, recycled at its end. */
    private var tracker: VelocityTracker? = null
    private val slop by lazy { ViewConfiguration.get(this).scaledTouchSlop }
    /** The finger the swipe follows (its pointer id), where and when it came down. */
    private var pointer = -1
    private var downX = 0f
    private var downY = 0f
    private var downAt = 0L
    /** Where the swipe's way across counts from: where the finger stood as it began. */
    private var originX = 0f
    /** One finger down, not yet past the touch slop. */
    private var deciding = false
    /** The gesture is the swipe's: gpui hears nothing more of it. */
    private var swiping = false

    /**
     * The tabs' swipe (`crates/phone/src/shell/swipe.rs`), as the iOS launcher's pan takes it: one finger that moves
     * past the touch slop asks Rust whether it begins ([Native.swipeBegins]: a tab root on screen, nothing over it,
     * the way it went to the slop clearly sideways, `ViewPager`'s rule). Then gpui's touch is cancelled and the
     * finger's way across from there (no jump of the slop) and its speed go to Rust ([Native.swipeDrag]) until it
     * lifts ([Native.swipeLetGo]) or is cancelled ([Native.swipeCancel]); a second finger down is not a swipe, and
     * once one is, a lifted finger hands it to another still down, as `ViewPager` does. True while the swipe holds
     * the gesture. Back stays the system's: no swipe back begins here.
     */
    private fun swiped(event: MotionEvent): Boolean {
        val action = event.actionMasked
        if (action == MotionEvent.ACTION_DOWN) {
            tracker?.recycle()
            tracker = VelocityTracker.obtain()
            pointer = event.getPointerId(0)
            downX = event.x
            downY = event.y
            downAt = event.eventTime
            deciding = true
            swiping = false
        }
        val tracker = tracker ?: return false
        if (!deciding && !swiping) return false
        tracker.addMovement(event)
        val dp = resources.displayMetrics.density
        when (action) {
            MotionEvent.ACTION_POINTER_DOWN -> deciding = false
            MotionEvent.ACTION_POINTER_UP -> if (event.getPointerId(event.actionIndex) == pointer) {
                if (swiping) {
                    val other = if (event.actionIndex == 0) 1 else 0
                    originX += event.getX(other) - event.getX(event.actionIndex)
                    pointer = event.getPointerId(other)
                    tracker.clear()
                } else {
                    deciding = false
                }
            }
            MotionEvent.ACTION_MOVE -> {
                val at = event.findPointerIndex(pointer)
                val dx = event.getX(at) - downX
                val dy = event.getY(at) - downY
                if (deciding && hypot(dx, dy) > slop) {
                    deciding = false
                    val held = (event.eventTime - downAt) / 1000f
                    swiping = Native.swipeBegins(downX / dp, downY / dp, dx / dp, dy / dp, held)
                    if (swiping) {
                        originX = event.getX(at)
                        Native.touch(MotionEvent.ACTION_CANCEL, 0, pointers(event))
                    }
                }
            }
        }
        val taken = swiping
        if (swiping) {
            tracker.computeCurrentVelocity(1000)
            val x = (event.getX(event.findPointerIndex(pointer)) - originX) / dp
            val speed = tracker.getXVelocity(pointer) / dp
            when (action) {
                MotionEvent.ACTION_MOVE -> Native.swipeDrag(x, speed)
                MotionEvent.ACTION_UP -> {
                    Native.swipeDrag(x, speed)
                    Native.swipeLetGo()
                }
                MotionEvent.ACTION_CANCEL -> Native.swipeCancel()
            }
        }
        if (action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
            deciding = false
            swiping = false
            tracker.recycle()
            this.tracker = null
        }
        return taken
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

    /** The field the keyboard types into; made the first time gm asks for the keyboard. */
    private var input: InputProxy? = null

    /** gm: a field took focus. [keyboardType] is gm's `KeyboardType` (0 text .. 5 decimal). */
    fun gpuiShowKeyboard(keyboardType: Int, session: Long) = runOnUiThread {
        val field = input ?: InputProxy().also {
            it.alpha = 0f
            it.setPadding(0, 0, 0, 0)
            addContentView(it, ViewGroup.LayoutParams(1, 1))
            input = it
        }
        field.reset(session)
        field.inputType = when (keyboardType) {
            1 -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS
            2 -> InputType.TYPE_CLASS_PHONE
            3 -> InputType.TYPE_CLASS_NUMBER
            4 -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI
            5 -> InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_FLAG_DECIMAL
            else -> InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
        }
        field.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI
        field.requestFocus()
        val keyboard = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        keyboard.restartInput(field)
        keyboard.showSoftInput(field, InputMethodManager.SHOW_IMPLICIT)
    }

    /** gm: the field lost focus. */
    fun gpuiHideKeyboard(session: Long) = runOnUiThread {
        val field = input ?: return@runOnUiThread
        field.reset(session)
        (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).hideSoftInputFromWindow(field.windowToken, 0)
        field.clearFocus()
    }

    /** gm: the app changed the text under a composition; the keyboard starts over. */
    fun gpuiResetComposition(session: Long) = runOnUiThread {
        val field = input ?: return@runOnUiThread
        field.reset(session)
        (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).restartInput(field)
    }

    /**
     * The keyboard's connection, for multistage keyboards (Korean, Japanese): it holds only what is being composed
     * and hands every change to Rust as [Native.ime]; a commit empties it. Edits are batched by [depth], so one
     * keyboard action is one update. A connection made for an older [session] is refused.
     */
    private inner class InputProxy : EditText(this@MainActivity) {
        private var session = 0L
        private var depth = 0
        private var marked = false

        init {
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence, start: Int, count: Int, after: Int) {}
                override fun onTextChanged(s: CharSequence, start: Int, before: Int, count: Int) {}
                override fun afterTextChanged(text: Editable) {
                    // a hardware keyboard edits the widget outside its connection
                    if (depth == 0) {
                        depth++
                        endEdit()
                    }
                }
            })
        }

        override fun onKeyDown(code: Int, event: KeyEvent): Boolean {
            if (code == KeyEvent.KEYCODE_DEL && text.isEmpty() && !marked) {
                Native.ime(session, 3, "", 1, 0)
                return true
            }
            return super.onKeyDown(code, event)
        }

        fun reset(next: Long) {
            depth++
            text.clear()
            marked = false
            session = next
            depth = 0
        }

        fun endEdit() {
            if (--depth != 0) return
            val text = text
            val composing = BaseInputConnection.getComposingSpanStart(text) >= 0
            if (composing || marked || text.isNotEmpty()) {
                Native.ime(session, if (composing) 0 else 1, text.toString(),
                    maxOf(0, Selection.getSelectionStart(text)), maxOf(0, Selection.getSelectionEnd(text)))
                marked = composing
                if (!composing) {
                    depth++
                    text.clear()
                    depth--
                }
            }
        }

        override fun onKeyPreIme(code: Int, event: KeyEvent): Boolean {
            if (code == KeyEvent.KEYCODE_BACK && event.action == KeyEvent.ACTION_UP) Native.ime(session, 5, "", 0, 0)
            return super.onKeyPreIme(code, event)
        }

        override fun onCreateInputConnection(info: EditorInfo): InputConnection? {
            val connection = super.onCreateInputConnection(info) ?: return null
            val made = session
            return object : InputConnectionWrapper(connection, false) {
                /** Runs [edit] as one batched edit, refused for a connection of an older session. */
                private fun batched(edit: () -> Boolean): Boolean {
                    if (made != session) return false
                    depth++
                    try {
                        return edit()
                    } finally {
                        endEdit()
                    }
                }

                override fun beginBatchEdit(): Boolean {
                    if (made != session) return false
                    depth++
                    return super.beginBatchEdit()
                }

                override fun endBatchEdit(): Boolean {
                    if (made != session) return false
                    val result = super.endBatchEdit()
                    if (depth > 0) endEdit()
                    return result
                }

                override fun setComposingText(text: CharSequence, cursor: Int) =
                    batched { super.setComposingText(text, cursor) }

                override fun setComposingRegion(start: Int, end: Int) = batched { super.setComposingRegion(start, end) }

                override fun finishComposingText() = batched { super.finishComposingText() }

                override fun commitText(text: CharSequence, cursor: Int) = batched { super.commitText(text, cursor) }

                override fun setSelection(start: Int, end: Int) = batched { super.setSelection(start, end) }

                override fun deleteSurroundingText(before: Int, after: Int): Boolean {
                    if (made != session) return false
                    if (this@InputProxy.text.isEmpty() && !marked) {
                        Native.ime(session, 2, "", before, after)
                        return true
                    }
                    return batched { super.deleteSurroundingText(before, after) }
                }

                override fun deleteSurroundingTextInCodePoints(before: Int, after: Int): Boolean {
                    if (made != session) return false
                    if (this@InputProxy.text.isEmpty() && !marked) {
                        Native.ime(session, 3, "", before, after)
                        return true
                    }
                    return batched { super.deleteSurroundingTextInCodePoints(before, after) }
                }

                override fun sendKeyEvent(event: KeyEvent): Boolean {
                    if (made != session) return false
                    when (event.keyCode) {
                        KeyEvent.KEYCODE_DEL -> {
                            if (event.action == KeyEvent.ACTION_DOWN) deleteSurroundingText(1, 0)
                            return true
                        }
                        KeyEvent.KEYCODE_ENTER -> {
                            if (event.action == KeyEvent.ACTION_DOWN) commitText("\n", 1)
                            return true
                        }
                    }
                    return super.sendKeyEvent(event)
                }

                override fun performEditorAction(action: Int): Boolean {
                    if (made != session) return false
                    if (action == EditorInfo.IME_ACTION_DONE) {
                        finishComposingText()
                        Native.ime(session, 4, "", 0, 0)
                        return true
                    }
                    return commitText("\n", 1)
                }
            }
        }
    }

    private companion object {
        var next = 1L

        /** [ask]'s request code, its lock, and the answer it waits for. */
        const val ASK = 1
        val asking = Any()
        @Volatile
        var asked: ArrayBlockingQueue<Boolean>? = null

        /** [pickFiles]'s and [keepFile]'s request codes, their locks, and the answers they wait for. */
        const val PICK = 7101
        const val KEEP = 7102
        const val VIEW = 7103
        val picking = Any()
        val keeping = Any()
        @Volatile
        var picked: ArrayBlockingQueue<Array<String>>? = null
        @Volatile
        var kept: ArrayBlockingQueue<Boolean>? = null
        @Volatile
        var keptFile: File? = null
    }
}
