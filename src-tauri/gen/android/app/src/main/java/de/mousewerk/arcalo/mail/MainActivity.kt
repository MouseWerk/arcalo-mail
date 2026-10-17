package de.mousewerk.arcalo.mail

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import android.view.View
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  private var webView: WebView? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // The app stays clear of the status bar, the navigation bar, a display cutout and the
    // keyboard on every Android version (older WebViews report no safe-area insets), as Arcalo.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    MailNotifier.createChannel(this)
    MailCheckWorker.schedule(this)
    takeRedirect(intent)
    takeOpen(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    takeRedirect(intent)
    takeOpen(intent)
  }

  /** A notification or the widget asked to open a message (to read it or to reply): handed to Rust. */
  private fun takeOpen(intent: Intent?) {
    if (intent == null) return
    val id = intent.getLongExtra(EXTRA_OPEN, -1)
    if (id < 0) return
    MailNative.openMessage(id, intent.getBooleanExtra(EXTRA_REPLY, false))
    intent.removeExtra(EXTRA_OPEN)
  }

  companion object {
    const val EXTRA_OPEN = "open_message"
    const val EXTRA_REPLY = "reply"
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    followFontScale()
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    followFontScale()
  }

  /** The system's font size (Settings → Display → Font size) for the app's text. */
  private fun followFontScale() {
    webView?.settings?.textZoom = (resources.configuration.fontScale * 100).toInt().coerceIn(85, 200)
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
