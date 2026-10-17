package de.mousewerk.arcalo.mail

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationManagerCompat

/**
 * The buttons of a new-mail notification that act without opening the app: archive and mark
 * read. Rust does it (android.rs, notificationAction): through the running app, or in the store
 * when the app is closed, and the next check carries it to the server (asked for soon).
 */
class MailActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getLongExtra(EXTRA_ID, -1)
        val action = intent.getStringExtra(EXTRA_ACTION) ?: return
        if (id < 0) return
        NotificationManagerCompat.from(context).cancel(intent.getIntExtra(EXTRA_NOTE, 0))
        val pending = goAsync()
        Thread {
            try {
                val unread = MailNative.notificationAction(context, context.dataDir.absolutePath, id, action)
                if (unread >= 0) {
                    MailNotifier.setUnread(context, unread)
                    MailCheckWorker.soon(context)
                }
            } catch (e: Throwable) {
                // The library could not act; the message stays as it was.
            } finally {
                pending.finish()
            }
        }.start()
    }

    companion object {
        const val EXTRA_ID = "message_id"
        const val EXTRA_ACTION = "action"
        const val EXTRA_NOTE = "note"
    }
}
