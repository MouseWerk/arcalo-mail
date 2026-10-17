package de.mousewerk.arcalo.mail

import android.content.Context

/** The Rust side (src-tauri/src/mobile/android.rs), also in a process without the app's activity. */
object MailNative {
    init {
        System.loadLibrary("arcalo_mail_lib")
    }

    /** One check of every account; the report (background::Report) as JSON. Blocks: never on the main thread. */
    @JvmStatic external fun backgroundCheck(context: Context, dataDir: String): String

    /** The sign-in's redirect reached the app. */
    @JvmStatic external fun onRedirect(url: String)

    /** WorkManager stops the check. */
    @JvmStatic external fun cancelCheck()

    /** A notification's button: "archive" or "read". The unread count, or -1 when the running app does it. */
    @JvmStatic external fun notificationAction(context: Context, dataDir: String, id: Long, action: String): Int

    /** A notification or the widget opened the app on a message (to read it or to reply). */
    @JvmStatic external fun openMessage(id: Long, reply: Boolean)

    /** An e-mail link (mailto:) of another app: the compose sheet opens with it. */
    @JvmStatic external fun openMailto(url: String)
}
