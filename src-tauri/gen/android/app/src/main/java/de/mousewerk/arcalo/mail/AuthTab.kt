package de.mousewerk.arcalo.mail

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent

/**
 * The Microsoft sign-in in a Custom Tab over the app (RFC 8252: the system browser, with its
 * saved sign-ins; the app never sees the password). The redirect comes back to MainActivity.
 * Without a browser that offers Custom Tabs, the browser opens on its own.
 */
object AuthTab {
    @JvmStatic
    fun open(activity: Activity, url: String) {
        activity.runOnUiThread {
            val uri = Uri.parse(url)
            try {
                CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(activity, uri)
            } catch (e: ActivityNotFoundException) {
                try {
                    activity.startActivity(Intent(Intent.ACTION_VIEW, uri))
                } catch (e: ActivityNotFoundException) {
                    // No browser at all: back in the app, the sign-in counts as cancelled.
                }
            }
        }
    }
}
