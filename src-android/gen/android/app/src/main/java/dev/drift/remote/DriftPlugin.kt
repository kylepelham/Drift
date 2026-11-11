package dev.drift.remote

import android.app.Activity
import android.os.Build
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/** What Rust asks of Android: this phone's name. */
@TauriPlugin
class DriftPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun deviceName(invoke: Invoke) {
        val named = Settings.Global.getString(activity.contentResolver, "device_name")
        invoke.resolve(JSObject().put("name", named?.takeIf { it.isNotBlank() } ?: Build.MODEL))
    }
}
