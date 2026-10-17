package de.mousewerk.arcalo.mail

import android.Manifest
import android.annotation.SuppressLint
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject

/**
 * Notifications about new mail from the background check (MailCheckWorker.kt), grouped per
 * account, and the unread count: launchers show the notification's number as the app's badge.
 * Local only: nothing comes from or goes to a push service.
 */
object MailNotifier {
    private const val CHANNEL = "new-mail"
    private const val GROUP_SUMMARY = 1

    fun createChannel(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL,
            context.getString(R.string.channel_new_mail),
            NotificationManager.IMPORTANCE_DEFAULT
        ).apply {
            description = context.getString(R.string.channel_new_mail_text)
            setShowBadge(true)
        }
        context.getSystemService(NotificationManager::class.java)?.createNotificationChannel(channel)
    }

    private fun allowed(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun openApp(context: Context): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun base(context: Context, unread: Int) =
        NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_mail)
            .setCategory(NotificationCompat.CATEGORY_EMAIL)
            .setNumber(unread)
            .setAutoCancel(true)
            .setContentIntent(openApp(context))

    /** Opens the app on a message, to read it or (`reply`) to answer it. */
    private fun openMessage(context: Context, id: Long, reply: Boolean, request: Int): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            .putExtra(MainActivity.EXTRA_OPEN, id)
            .putExtra(MainActivity.EXTRA_REPLY, reply)
            .putExtra(MainActivity.EXTRA_TOKEN, MainActivity.token(context))
        return PendingIntent.getActivity(context, request, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    /** Archive or mark read without the app (MailActionReceiver.kt). */
    private fun act(context: Context, id: Long, action: String, note: Int, request: Int): PendingIntent {
        val intent = Intent(context, MailActionReceiver::class.java)
            .putExtra(MailActionReceiver.EXTRA_ID, id)
            .putExtra(MailActionReceiver.EXTRA_ACTION, action)
            .putExtra(MailActionReceiver.EXTRA_NOTE, note)
        return PendingIntent.getBroadcast(context, request, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    /** Shows what a check found (`{"skipped":…, "notes":[{key, group, title, body, id}], "unread":n}`). */
    @JvmStatic
    @SuppressLint("MissingPermission") // allowed() checks it; a later refusal throws SecurityException.
    fun show(context: Context, reportJson: String) {
        val report = try { JSONObject(reportJson) } catch (e: Exception) { return }
        if (report.optBoolean("skipped")) return
        val unread = report.optInt("unread")
        val notes = report.optJSONArray("notes")
        val manager = NotificationManagerCompat.from(context)
        if (unread == 0) {
            manager.cancelAll()
            return
        }
        if (notes == null || notes.length() == 0 || !allowed(context)) return
        createChannel(context)
        var group = ""
        try {
            for (i in 0 until notes.length()) {
                val note = notes.getJSONObject(i)
                group = note.optString("group")
                val key = note.optString("key").hashCode()
                val n = base(context, unread)
                    .setContentTitle(note.optString("title"))
                    .setContentText(note.optString("body"))
                    .setStyle(NotificationCompat.BigTextStyle().bigText(note.optString("body")))
                    .setGroup(group)
                // A note about one message: its buttons, and a tap opens that message.
                if (note.has("id") && !note.isNull("id")) {
                    val id = note.getLong("id")
                    val r = (id % 100000).toInt() * 4
                    n.setContentIntent(openMessage(context, id, false, r))
                        .addAction(0, context.getString(R.string.action_archive), act(context, id, "archive", key, r + 1))
                        .addAction(0, context.getString(R.string.action_read), act(context, id, "read", key, r + 2))
                        .addAction(0, context.getString(R.string.action_reply), openMessage(context, id, true, r + 3))
                }
                manager.notify(key, n.build())
            }
            val summary = base(context, unread)
                .setContentTitle(context.getString(R.string.channel_new_mail))
                .setGroup(group)
                .setGroupSummary(true)
                .setSilent(true)
                .build()
            manager.notify(GROUP_SUMMARY, summary)
        } catch (e: SecurityException) {
            // The permission was taken back in the meantime.
        }
    }

    /** The app read or archived mail: the count follows; nothing unread, no notifications. */
    @JvmStatic
    @SuppressLint("MissingPermission") // allowed() checks it; a later refusal throws SecurityException.
    fun setUnread(context: Context, unread: Int) {
        val manager = NotificationManagerCompat.from(context)
        if (unread == 0) {
            manager.cancelAll()
            return
        }
        if (!allowed(context)) return
        val active = context.getSystemService(NotificationManager::class.java)?.activeNotifications ?: return
        if (active.none { it.id == GROUP_SUMMARY }) return
        try {
            val summary = base(context, unread)
                .setContentTitle(context.getString(R.string.channel_new_mail))
                .setGroupSummary(true)
                .setSilent(true)
                .build()
            manager.notify(GROUP_SUMMARY, summary)
        } catch (e: SecurityException) {
            // The permission was taken back in the meantime.
        }
    }
}
