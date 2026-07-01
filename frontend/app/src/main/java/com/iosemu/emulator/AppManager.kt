package com.iosemu.emulator

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import com.iosemu.emulator.emu.EmulatorBridge
import com.iosemu.emulator.model.AppEntry
import org.json.JSONArray
import java.io.File

/**
 * The app-manager: bridges the Compose UI to the native core. It owns the
 * on-device directory conventions (where IPAs are looked for, where sandboxes
 * and the system image live) and converts native results into UI models.
 */
class AppManager(context: Context) {

    /** Where users drop `.ipa` files (app-specific external dir — no permission). */
    val ipaDirectory: File = File(context.getExternalFilesDir(null), "IPAs").apply { mkdirs() }

    /** Root of all per-app sandbox containers. */
    private val containersRoot: File =
        File(context.filesDir, "Containers").apply { mkdirs() }

    /** Shared read-only iOS runtime image (fonts, stub frameworks, ...). */
    private val systemRoot: File =
        File(context.filesDir, "System").apply { mkdirs() }

    /** Scan the IPA directory and return the discovered apps. */
    fun scan(): List<AppEntry> {
        val json = EmulatorBridge.nativeScan(ipaDirectory.absolutePath)
        return parse(json)
    }

    /** Decode the primary icon of [entry], or null if it has none / fails. */
    fun icon(entry: AppEntry): Bitmap? {
        if (!entry.hasIcon) return null
        val bytes = EmulatorBridge.nativeIcon(entry.ipaPath)
        if (bytes.isEmpty()) return null
        return runCatching { BitmapFactory.decodeByteArray(bytes, 0, bytes.size) }.getOrNull()
    }

    /**
     * Launch [entry]. Blocking — callers must invoke off the main thread. Returns
     * the guest exit code (negative on host-side failure).
     */
    fun launch(entry: AppEntry): Int =
        EmulatorBridge.nativeLaunch(
            entry.ipaPath,
            containersRoot.absolutePath,
            systemRoot.absolutePath,
        )

    private fun parse(json: String): List<AppEntry> {
        val arr = runCatching { JSONArray(json) }.getOrNull() ?: return emptyList()
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            AppEntry(
                name = o.optString("name").ifBlank { "Untitled" },
                bundleId = o.optString("bundleId"),
                minOs = o.optString("minOs"),
                ipaPath = o.optString("ipaPath"),
                hasIcon = o.optBoolean("hasIcon", false),
            )
        }
    }
}
