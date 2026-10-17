package de.mousewerk.arcalo.mail

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.view.View
import android.widget.RemoteViews
import org.json.JSONObject
import java.io.File

/**
 * The home-screen widget: the unread count of the inboxes and the newest three mails of the
 * People tab. It reads widget.json, which the app writes next to its database after each sync
 * and background check (mail_core::widget), so it needs neither the app running nor the
 * database. A tap on a mail opens it in the app, a tap elsewhere opens the app.
 */
class MailWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        val views = build(context)
        for (id in ids) manager.updateAppWidget(id, views)
    }

    companion object {
        private val ROWS = intArrayOf(R.id.widget_row_1, R.id.widget_row_2, R.id.widget_row_3)
        private val FROM = intArrayOf(R.id.widget_from_1, R.id.widget_from_2, R.id.widget_from_3)
        private val SUBJECT = intArrayOf(R.id.widget_subject_1, R.id.widget_subject_2, R.id.widget_subject_3)

        /** The app wrote widget.json again (android.rs, refresh_widget). */
        @JvmStatic
        fun refresh(context: Context) {
            val manager = AppWidgetManager.getInstance(context) ?: return
            val ids = manager.getAppWidgetIds(ComponentName(context, MailWidget::class.java))
            if (ids.isEmpty()) return
            val views = build(context)
            for (id in ids) manager.updateAppWidget(id, views)
        }

        private fun open(context: Context, id: Long, request: Int): PendingIntent {
            val intent = Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            if (id >= 0) intent.putExtra(MainActivity.EXTRA_OPEN, id)
            return PendingIntent.getActivity(
                context, request, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            )
        }

        private fun build(context: Context): RemoteViews {
            val views = RemoteViews(context.packageName, R.layout.widget_mail)
            val data = try {
                JSONObject(File(context.dataDir, "widget.json").readText())
            } catch (e: Exception) {
                null
            }
            val unread = data?.optInt("unread") ?: 0
            views.setTextViewText(
                R.id.widget_unread,
                if (unread == 0) context.getString(R.string.widget_none_unread)
                else context.resources.getQuantityString(R.plurals.widget_unread, unread, unread)
            )
            views.setOnClickPendingIntent(R.id.widget_root, open(context, -1, 100))
            val people = data?.optJSONArray("people")
            val count = people?.length() ?: 0
            views.setViewVisibility(R.id.widget_empty, if (count == 0) View.VISIBLE else View.GONE)
            for (i in ROWS.indices) {
                val row = if (i < count) people!!.optJSONObject(i) else null
                if (row == null) {
                    views.setViewVisibility(ROWS[i], View.GONE)
                    continue
                }
                views.setViewVisibility(ROWS[i], View.VISIBLE)
                views.setTextViewText(FROM[i], row.optString("from"))
                views.setTextViewText(SUBJECT[i], row.optString("subject"))
                // Unread in the text's full color, read ones quieter.
                val color = context.getColor(if (row.optBoolean("unread")) R.color.widget_text else R.color.widget_text_2)
                views.setTextColor(FROM[i], color)
                views.setOnClickPendingIntent(ROWS[i], open(context, row.optLong("id"), 101 + i))
            }
            return views
        }
    }
}
