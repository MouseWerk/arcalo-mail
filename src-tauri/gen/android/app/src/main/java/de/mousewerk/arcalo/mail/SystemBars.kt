package de.mousewerk.arcalo.mail

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.view.View
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import org.json.JSONObject

/**
 * The system bars, the display cutout and the keyboard for the web content. Android 15 draws
 * every app edge to edge, and the WebView reports no safe-area insets of its own, so the space
 * the bars cover goes to the page as CSS variables on <html>, in CSS pixels, whenever it changes:
 * `--sat`, `--sar`, `--sab`, `--sal` (status bar and cutout, navigation bar), `--ime` (the
 * keyboard's height) and `--type-scale` (the system's font size). The page keeps its content
 * clear of them and paints its own colors under the bars (ui/src/styles/tokens.css). The keyboard
 * makes the web view smaller, as adjustResize did before edge to edge, so the page ends above it.
 *
 * The other way, the page says which theme it shows (`ArcaloSystemBars.setTheme`): the bars' icons
 * follow it, and its background is kept for the window behind the page at the next start, so the
 * app opens in its own colors instead of a white or system-colored frame.
 */
class SystemBars(private val activity: Activity) {
  private var webView: WebView? = null
  @Volatile private var values = JSONObject()

  /** Before the page exists: the window in the colors of the last start. */
  fun restore() {
    val prefs = prefs(activity)
    if (!prefs.contains(KEY_BACKGROUND)) return
    val background = prefs.getInt(KEY_BACKGROUND, Color.BLACK)
    activity.window.setBackgroundDrawable(ColorDrawable(background))
    lightIcons(!prefs.getBoolean(KEY_DARK, false))
  }

  fun attach(view: WebView) {
    webView = view
    // Nothing white before the page's first paint.
    view.setBackgroundColor(prefs(activity).getInt(KEY_BACKGROUND, Color.TRANSPARENT))
    // Read by ui/src/lib/viewport.ts at start; changes are pushed (push()).
    view.addJavascriptInterface(Bridge(), "ArcaloSystemBars")
    view.settings.textZoom = 100
    val content = activity.findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { v, insets ->
      val density = activity.resources.displayMetrics.density
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      val keyboard = insets.isVisible(WindowInsetsCompat.Type.ime())
      val ime = if (keyboard) insets.getInsets(WindowInsetsCompat.Type.ime()).bottom else 0
      // The keyboard covers the navigation bar: the page ends above the keyboard.
      v.setPadding(0, 0, 0, ime)
      val css = { px: Int -> "${Math.round(px / density)}px" }
      values = JSONObject()
        .put("--sat", css(bars.top))
        .put("--sar", css(bars.right))
        .put("--sab", css(if (keyboard) 0 else bars.bottom))
        .put("--sal", css(bars.left))
        .put("--ime", css(ime))
        .put("--type-scale", fontScale())
      push()
      WindowInsetsCompat.CONSUMED
    }
    ViewCompat.requestApplyInsets(content)
  }

  /** The system's font size changed (Settings → Display → Font size). */
  fun onConfigurationChanged() {
    values = JSONObject(values.toString()).put("--type-scale", fontScale())
    push()
  }

  private fun fontScale(): String = activity.resources.configuration.fontScale.coerceIn(0.85f, 2f).toString()

  private fun push() {
    val script = "(function(v){var s=document.documentElement.style;for(var k in v)s.setProperty(k,v[k]);" +
      "window.dispatchEvent(new Event('arcalo-insets'));})($values)"
    webView?.post { webView?.evaluateJavascript(script, null) }
  }

  private fun lightIcons(light: Boolean) {
    val controller = WindowCompat.getInsetsController(activity.window, activity.window.decorView)
    controller.isAppearanceLightStatusBars = light
    controller.isAppearanceLightNavigationBars = light
  }

  private inner class Bridge {
    /** The current values (the page asks once at start; a reload starts without them). */
    @JavascriptInterface
    fun insets(): String = values.toString()

    /** The page's theme: `dark`, and its background as #rrggbb. */
    @JavascriptInterface
    fun setTheme(dark: Boolean, background: String) {
      val color = try {
        Color.parseColor(background)
      } catch (e: IllegalArgumentException) {
        return
      }
      prefs(activity).edit().putBoolean(KEY_DARK, dark).putInt(KEY_BACKGROUND, color).apply()
      activity.runOnUiThread {
        activity.window.setBackgroundDrawable(ColorDrawable(color))
        webView?.setBackgroundColor(color)
        lightIcons(!dark)
      }
    }
  }

  companion object {
    private const val KEY_DARK = "dark"
    private const val KEY_BACKGROUND = "background"

    private fun prefs(context: Context) = context.getSharedPreferences("theme", Context.MODE_PRIVATE)
  }
}
