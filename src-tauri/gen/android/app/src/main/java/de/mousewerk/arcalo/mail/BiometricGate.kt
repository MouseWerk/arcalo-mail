package de.mousewerk.arcalo.mail

import android.app.Activity
import android.content.Context
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The app lock's system sign-in (security.rs, through android.rs): the fingerprint, the face or
 * the screen lock, asked by Android's BiometricPrompt. The app only learns yes or no. Android 9
 * and later; Android 11 and later also accept the screen lock's PIN, pattern or password.
 */
object BiometricGate {
    private const val WAIT_SECONDS = 120L

    /** Any context will do: the system service is the same for the whole app. */
    @JvmStatic
    fun available(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false
        val manager = context.getSystemService(BiometricManager::class.java) ?: return false
        val result = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            manager.canAuthenticate(
                BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
            )
        } else {
            @Suppress("DEPRECATION")
            manager.canAuthenticate()
        }
        return result == BiometricManager.BIOMETRIC_SUCCESS
    }

    /** Shows the prompt over the activity and waits for the answer (never call on the main thread). */
    @JvmStatic
    fun verify(activity: Activity, title: String): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) return false
        val done = CountDownLatch(1)
        val ok = AtomicBoolean(false)
        val cancel = CancellationSignal()
        activity.runOnUiThread {
            try {
                val builder = BiometricPrompt.Builder(activity).setTitle(title)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    builder.setAllowedAuthenticators(
                        BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL,
                    )
                } else {
                    builder.setNegativeButton(activity.getString(android.R.string.cancel), activity.mainExecutor) { _, _ ->
                        done.countDown()
                    }
                }
                builder.build().authenticate(
                    cancel,
                    activity.mainExecutor,
                    object : BiometricPrompt.AuthenticationCallback() {
                        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                            ok.set(true)
                            done.countDown()
                        }

                        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                            done.countDown()
                        }
                    },
                )
            } catch (e: Exception) {
                done.countDown()
            }
        }
        if (!done.await(WAIT_SECONDS, TimeUnit.SECONDS)) {
            activity.runOnUiThread { cancel.cancel() }
        }
        return ok.get()
    }
}
