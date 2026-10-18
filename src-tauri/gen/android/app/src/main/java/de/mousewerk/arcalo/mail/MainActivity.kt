package de.mousewerk.arcalo.mail

import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import java.security.MessageDigest
import java.security.SecureRandom

class MainActivity : TauriActivity() {
  private val bars = SystemBars(this)

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Edge to edge on every Android version: the page paints under the bars and keeps its
    // content clear of them with the insets it is given (SystemBars.kt).
    bars.restore()
    MailNotifier.createChannel(this)
    MailCheckWorker.schedule(this)
    takeRedirect(intent)
    takeOpen(intent)
    takeMailto(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    takeRedirect(intent)
    takeOpen(intent)
    takeMailto(intent)
  }

  /**
   * An e-mail link of another app (mailto:, as VIEW or SENDTO): handed to Rust, which opens the
   * compose sheet. The subject and text another app may pass as extras go into the address.
   */
  private fun takeMailto(intent: Intent?) {
    val data = intent?.data ?: return
    if (data.scheme != "mailto") return
    if (intent.action != Intent.ACTION_VIEW && intent.action != Intent.ACTION_SENDTO) return
    var url = data.toString()
    if (!url.contains('?')) {
      val extras = listOfNotNull(
        intent.getStringExtra(Intent.EXTRA_SUBJECT)?.let { "subject=" + Uri.encode(it) },
        intent.getStringExtra(Intent.EXTRA_TEXT)?.let { "body=" + Uri.encode(it) },
      )
      if (extras.isNotEmpty()) url += "?" + extras.joinToString("&")
    }
    MailNative.openMailto(url)
    intent.data = null
  }

  /**
   * A notification or the widget asked to open a message (to read it or to reply): handed to Rust.
   * The activity is exported (it takes mailto: links), so only intents that carry this install's
   * token count: the app's own PendingIntents. Another app's „open message 42“ is ignored.
   */
  private fun takeOpen(intent: Intent?) {
    if (intent == null) return
    val id = intent.getLongExtra(EXTRA_OPEN, -1)
    if (id < 0) return
    val given = intent.getStringExtra(EXTRA_TOKEN)
    intent.removeExtra(EXTRA_OPEN)
    intent.removeExtra(EXTRA_TOKEN)
    if (given == null || !MessageDigest.isEqual(given.toByteArray(), token(this).toByteArray())) return
    MailNative.openMessage(id, intent.getBooleanExtra(EXTRA_REPLY, false))
  }

  companion object {
    init {
      // Debug builds only: the WebView's DevTools socket for the emulator tests (e2e/android).
      if (BuildConfig.DEBUG) WebView.setWebContentsDebuggingEnabled(true)
    }

    const val EXTRA_OPEN = "open_message"
    const val EXTRA_REPLY = "reply"
    const val EXTRA_TOKEN = "token"

    /** A random token of this install, in the app's private preferences (no other app reads it). */
    @JvmStatic
    fun token(context: Context): String {
      val prefs = context.getSharedPreferences("intents", Context.MODE_PRIVATE)
      prefs.getString("token", null)?.let { return it }
      val bytes = ByteArray(32).also { SecureRandom().nextBytes(it) }
      val made = bytes.joinToString("") { "%02x".format(it) }
      prefs.edit().putString("token", made).apply()
      return made
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    bars.attach(webView)
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    bars.onConfigurationChanged()
  }

  /** The Microsoft sign-in came back (de.mousewerk.arcalo.mail://oauth?…): handed to Rust. */
  private fun takeRedirect(intent: Intent?) {
    val data = intent?.data ?: return
    if (data.scheme == "de.mousewerk.arcalo.mail" && data.host == "oauth") {
      MailNative.onRedirect(data.toString())
      intent.data = null
    }
  }
}
