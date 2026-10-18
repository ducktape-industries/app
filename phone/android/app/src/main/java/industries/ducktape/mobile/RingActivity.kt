package industries.ducktape.mobile

import android.app.Activity
import android.app.KeyguardManager
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.lang.ref.WeakReference

/**
 * A ringing call over the lock screen: the call notification's full-screen intent on a locked or sleeping phone
 * ([Ring.ring]). It shows the call alone (who, Decline, Answer), never the app, which stays behind the lock: Answer
 * asks the person to unlock, then hands the call's link to [MainActivity] (SPEC D11). It goes with the ring: Decline,
 * Answer, [Ring.withdraw] or the call's `until`.
 */
class RingActivity : Activity() {
    private var tag: String? = null
    private val main = Handler(Looper.getMainLooper())

    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        // ponytail: Android 8.0 (26) has neither; the ring stays a notification on its lock screen there
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
        show(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        show(intent)
    }

    private fun show(intent: Intent) {
        val tag = intent.getStringExtra(Ring.TAG) ?: return finish()
        val link = intent.getStringExtra(LINK).orEmpty()
        this.tag = tag
        current = WeakReference(this)
        val title = TextView(this).apply {
            text = intent.getStringExtra(TITLE)
            textSize = 28f
            setTextColor(Color.WHITE)
            gravity = Gravity.CENTER
        }
        val body = TextView(this).apply {
            text = intent.getStringExtra(BODY)
            textSize = 16f
            setTextColor(Color.LTGRAY)
            gravity = Gravity.CENTER
        }
        val decline = Button(this).apply {
            text = "Decline"
            setOnClickListener {
                Ring.withdraw(this@RingActivity, tag)
                finish()
            }
        }
        val answer = Button(this).apply {
            text = "Answer"
            setOnClickListener { answer(tag, link) }
        }
        setContentView(LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setBackgroundColor(Color.BLACK)
            addView(title)
            addView(body)
            addView(LinearLayout(this@RingActivity).apply {
                gravity = Gravity.CENTER
                addView(decline)
                addView(answer)
            })
        })
        main.removeCallbacksAndMessages(null)
        val left = intent.getLongExtra(UNTIL, 0) * 1000 - System.currentTimeMillis()
        if (left > 0) main.postDelayed(::finish, left)
    }

    /** The lock first (the app behind it holds the person's keys), then the call's link, as the notification's Answer. */
    private fun answer(tag: String, link: String) {
        val open = {
            startActivity(
                Intent(Intent.ACTION_VIEW, Uri.parse(link)).setClassName(this, Ring.OWN).putExtra(Ring.TAG, tag),
            )
            finish()
        }
        val keyguard = getSystemService(KeyguardManager::class.java)
        if (!keyguard.isKeyguardLocked) return open()
        keyguard.requestDismissKeyguard(this, object : KeyguardManager.KeyguardDismissCallback() {
            override fun onDismissSucceeded() = open()
        })
    }

    override fun onDestroy() {
        main.removeCallbacksAndMessages(null)
        if (current?.get() === this) current = null
        super.onDestroy()
    }

    companion object {
        const val TITLE = "industries.ducktape.mobile.RING_TITLE"
        const val BODY = "industries.ducktape.mobile.RING_BODY"
        const val LINK = "industries.ducktape.mobile.RING_LINK"
        const val UNTIL = "industries.ducktape.mobile.RING_UNTIL"
        private var current: WeakReference<RingActivity>? = null

        /** The call under [tag] is over: its screen goes, if it shows. From any thread. */
        fun over(tag: String) {
            Handler(Looper.getMainLooper()).post { current?.get()?.takeIf { it.tag == tag }?.finish() }
        }
    }
}
