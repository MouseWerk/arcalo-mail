package de.mousewerk.arcalo.mail

import android.content.Context
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit

/**
 * New mail while the app is not in front: Android allows no open connection in the background
 * and Arcalo Mail has no push server, so WorkManager runs one check about every 15 minutes (the
 * shortest period Android allows; Doze and the battery saver delay it) when a network is there.
 * The check itself is Rust's (background.rs): every account once, then the notifications. It
 * does nothing while the app is in front (the app's own connections are open then).
 */
class MailCheckWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val report = try {
            MailNative.backgroundCheck(applicationContext, applicationContext.dataDir.absolutePath)
        } catch (e: Throwable) {
            return Result.retry()
        }
        MailNotifier.show(applicationContext, report)
        MailWidget.refresh(applicationContext)
        return Result.success()
    }

    override fun onStopped() {
        MailNative.cancelCheck()
    }

    companion object {
        private const val NAME = "mail-check"

        fun schedule(context: Context) {
            val request = PeriodicWorkRequestBuilder<MailCheckWorker>(15, TimeUnit.MINUTES)
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(NAME, ExistingPeriodicWorkPolicy.KEEP, request)
        }

        /** One check as soon as a network is there: a notification's button changed a message. */
        fun soon(context: Context) {
            val request = OneTimeWorkRequestBuilder<MailCheckWorker>()
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork("$NAME-soon", ExistingWorkPolicy.REPLACE, request)
        }
    }
}
