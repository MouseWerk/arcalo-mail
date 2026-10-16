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
}
