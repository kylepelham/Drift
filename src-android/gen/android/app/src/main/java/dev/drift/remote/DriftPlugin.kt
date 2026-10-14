package dev.drift.remote

import android.app.Activity
import android.os.Build
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

/** What Rust asks of Android: a QR scan through Google's scanner, which needs no camera permission, and this phone's name. */
@TauriPlugin
class DriftPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun scan(invoke: Invoke) {
        val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
        GmsBarcodeScanning.getClient(activity, options)
            .startScan()
            .addOnSuccessListener { code -> invoke.resolve(JSObject().put("text", code.rawValue)) }
            .addOnCanceledListener { invoke.resolve(JSObject()) }
            .addOnFailureListener { error -> invoke.reject(error.message ?: "the scanner could not start") }
    }

    @Command
    fun deviceName(invoke: Invoke) {
        val named = Settings.Global.getString(activity.contentResolver, "device_name")
        invoke.resolve(JSObject().put("name", named?.takeIf { it.isNotBlank() } ?: Build.MODEL))
    }
}
