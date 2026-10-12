package dev.drift.remote

import android.graphics.Color
import android.os.Bundle
import android.view.View
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    // Drift is dark by default: light status and navigation icons over its own background.
    enableEdgeToEdge(
      statusBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
      navigationBarStyle = SystemBarStyle.dark(Color.TRANSPARENT),
    )
    super.onCreate(savedInstanceState)

    // The WebView keeps clear of the status bar, the navigation bar, a cutout and the keyboard, so the page,
    // its drawers and dialogs, and the composer while typing all sit in the space left.
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val kinds = WindowInsetsCompat.Type.systemBars() or
        WindowInsetsCompat.Type.displayCutout() or
        WindowInsetsCompat.Type.ime()
      val clear = insets.getInsets(kinds)
      view.setPadding(clear.left, clear.top, clear.right, clear.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }
}
