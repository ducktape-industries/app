package industries.ducktape.mobile

import android.Manifest
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ActivityNotFoundException
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * The ring (huddle call SPEC §5): a call's high-priority data message becomes a call notification that rings until
 * it is answered, declined, withdrawn or past its `until`. The payload is the carrier's (pushd): `tag` names the call,
 * `title` is who calls, `link` is what Answer opens (an `ACTION_VIEW` intent to [MainActivity], the link path any
 * `duck://` link takes), `until` is Unix seconds. Decline is this phone's alone (SPEC D11): nothing is sent.
 * Firebase is the owner's project: without its `google-services.json` the build has no Firebase app, and nothing here
 * asks it for anything.
 */
object Ring {
    /** The intent extra naming the call an Answer came from, so the app takes its ring down. */
    const val TAG = "industries.ducktape.mobile.RING_TAG"
    private const val CHANNEL = "calls"
    /** Every call's notification is this id under its own tag. */
    private const val ID = 1
    /** [MainActivity]'s request code for the notification permission, apart from a view's asks. */
    const val ASK_NOTIFY = 2

    /** `call` rung, unless it is past its `until`. */
    fun ring(context: Context, call: Map<String, String>) {
        val tag = call["tag"] ?: return
        val until = call["until"]?.toLongOrNull() ?: 0
        val left = until * 1000 - System.currentTimeMillis()
        if (until != 0L && left <= 0) return
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Calls", NotificationManager.IMPORTANCE_HIGH).apply {
                setSound(
                    RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build(),
                )
                enableVibration(true)
            },
        )
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val answer = PendingIntent.getActivity(
            context, tag.hashCode(),
            Intent(Intent.ACTION_VIEW, Uri.parse(call["link"].orEmpty()), context, MainActivity::class.java)
                .putExtra(TAG, tag),
            flags,
        )
        val decline = PendingIntent.getBroadcast(
            context, tag.hashCode(), Intent(context, Decline::class.java).putExtra(TAG, tag), flags,
        )
        // a tap on the notification: the app, and the ring goes (the call is still the person's to answer there)
        val tapped = PendingIntent.getActivity(
            context, tag.hashCode(), Intent(context, MainActivity::class.java).putExtra(TAG, tag), flags,
        )
        // a locked or sleeping phone: the call's own screen over the lock, never the app
        val screen = PendingIntent.getActivity(
            context, tag.hashCode(),
            Intent(context, RingActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK).putExtra(TAG, tag)
                .putExtra(RingActivity.TITLE, call["title"].orEmpty()).putExtra(RingActivity.BODY, call["body"].orEmpty())
                .putExtra(RingActivity.LINK, call["link"].orEmpty()).putExtra(RingActivity.UNTIL, until),
            flags,
        )
        val notification = Notification.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.sym_call_incoming)
            .setContentTitle(call["title"].orEmpty())
            .setContentText(call["body"].orEmpty())
            .setCategory(Notification.CATEGORY_CALL)
            .setOngoing(true)
            .setContentIntent(tapped)
            .setFullScreenIntent(screen, true)
            .addAction(Notification.Action.Builder(null, "Decline", decline).build())
            .addAction(Notification.Action.Builder(null, "Answer", answer).build())
            .apply { if (until != 0L) setTimeoutAfter(left) }
            .build()
        // rings on until it is taken down, not once
        notification.flags = notification.flags or Notification.FLAG_INSISTENT
        manager.notify(tag, ID, notification)
    }

    /** The call under [tag] stops ringing, and its screen goes. */
    fun withdraw(context: Context, tag: String) {
        context.getSystemService(NotificationManager::class.java).cancel(tag, ID)
        RingActivity.over(tag)
    }

    /**
     * Firebase's token for this device to Rust, when the build has the owner's Firebase app.
     * firebase-messaging 26 deprecates `token` and `onNewToken` for `register()` and `onRegistered`, which take a
     * manifest opt-in (`firebase_messaging_installation_id_enabled`) and change when a token is handed over: a move
     * for when the device registry (pushd M2) reads the tokens, not a rename.
     */
    @Suppress("DEPRECATION")
    fun token(context: Context) {
        if (FirebaseApp.getApps(context).isEmpty()) return
        FirebaseMessaging.getInstance().token.addOnSuccessListener { Native.pushToken("fcm", it) }
    }

    /**
     * The notification permission (Android 13 on), asked on the app's first open and never again by the app; once it
     * is granted, the full-screen ring ([askFullScreenOnce], from [MainActivity]'s answer when it was asked here).
     */
    fun askOnce(activity: Activity) {
        if (Build.VERSION.SDK_INT < 33 ||
            activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        ) {
            return askFullScreenOnce(activity)
        }
        val prefs = activity.getSharedPreferences("ring", Context.MODE_PRIVATE)
        if (prefs.getBoolean("asked", false)) return
        prefs.edit().putBoolean("asked", true).apply()
        activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), ASK_NOTIFY)
    }

    /**
     * Android 14 on, the full-screen ring over a locked phone is the person's to allow (the Play Store grants it to
     * calling apps only); without it a call is a heads-up notification. Its Settings page opens once, when it is off.
     */
    fun askFullScreenOnce(activity: Activity) {
        if (Build.VERSION.SDK_INT < 34) return
        val prefs = activity.getSharedPreferences("ring", Context.MODE_PRIVATE)
        if (prefs.getBoolean("asked_full_screen", false)) return
        prefs.edit().putBoolean("asked_full_screen", true).apply()
        if (activity.getSystemService(NotificationManager::class.java).canUseFullScreenIntent()) return
        try {
            activity.startActivity(
                Intent(Settings.ACTION_MANAGE_APP_USE_FULL_SCREEN_INTENT, Uri.parse("package:${activity.packageName}")),
            )
        } catch (missing: ActivityNotFoundException) {
            Log.w("ducktape", "no full-screen ring page in Settings", missing)
        }
    }

    /**
     * A debug launch's `DUCKTAPE_RING_TEST` extra (`adb shell am start -n industries.ducktape.mobile/.MainActivity
     * -e DUCKTAPE_RING_TEST answer`): a sample call 2 s later through the message's own path, ringing 30 s; `answer`
     * or `decline` then fire that action's own intent 3 s later, as the person's tap does, so the flow runs without
     * FCM and without a finger. Any other value only rings.
     */
    fun test(context: Context, mode: String) {
        val tag = "call/ring-test"
        val call = mapOf(
            "kind" to "call", "chain" to "test", "program" to "huddle", "tag" to tag, "title" to "Ring test",
            "body" to "", "link" to "duck://test/huddle/call/ring-test",
            "until" to (System.currentTimeMillis() / 1000 + 30).toString(),
        )
        val main = Handler(Looper.getMainLooper())
        main.postDelayed({
            ring(context, call)
            val title = when (mode) {
                "answer" -> "Answer"
                "decline" -> "Decline"
                else -> return@postDelayed
            }
            main.postDelayed({
                val rung = context.getSystemService(NotificationManager::class.java).activeNotifications
                    .firstOrNull { it.tag == tag }?.notification
                val action = rung?.actions?.firstOrNull { it.title?.toString() == title }
                Log.i("ducktape", "ring test: $mode ${if (action != null) "tapped" else "found no ring"}")
                action?.actionIntent?.send()
            }, 3000)
        }, 2000)
    }

    /** A call's Decline: its ring down, on this phone alone. */
    class Decline : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            intent.getStringExtra(TAG)?.let { withdraw(context, it) }
        }
    }

    /** The owner's Firebase project's messages and tokens. */
    class Messages : FirebaseMessagingService() {
        override fun onMessageReceived(message: RemoteMessage) {
            when (message.data["kind"]) {
                "call" -> ring(this, message.data)
                "withdraw" -> message.data["tag"]?.let { withdraw(this, it) }
            }
        }

        @Suppress("OVERRIDE_DEPRECATION")
        override fun onNewToken(token: String) = Native.pushToken("fcm", token)
    }
}
