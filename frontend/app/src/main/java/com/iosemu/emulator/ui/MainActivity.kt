package com.iosemu.emulator.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.iosemu.emulator.emu.EmulatorBridge
import com.iosemu.emulator.model.AppEntry
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import java.io.File

/**
 * Single-screen launcher: scans the IPA directory via the native core, shows the
 * apps in a grid, and boots the tapped app on a background thread.
 */
class MainActivity : ComponentActivity() {

    /** Where users drop .ipa files (app-specific external dir — no permission). */
    private val ipaDir: File by lazy {
        File(getExternalFilesDir(null), "IPAs").apply { mkdirs() }
    }

    /** Per-app sandbox containers and the shared read-only system image. */
    private val containersRoot: File by lazy { File(filesDir, "Containers").apply { mkdirs() } }
    private val systemRoot: File by lazy { File(filesDir, "System").apply { mkdirs() } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                AppGridScreen(
                    ipaDir = ipaDir,
                    scan = { EmulatorBridge.nativeScan(ipaDir.absolutePath).parseApps() },
                    icon = { entry -> entry.decodeIcon() },
                    launch = { entry ->
                        EmulatorBridge.nativeLaunch(
                            entry.ipaPath,
                            containersRoot.absolutePath,
                            systemRoot.absolutePath,
                        )
                    },
                )
            }
        }
    }

    /** Decode the primary icon PNG for [entry] via the native bridge. */
    private fun AppEntry.decodeIcon(): Bitmap? {
        if (!hasIcon) return null
        val bytes = EmulatorBridge.nativeIcon(ipaPath)
        if (bytes.isEmpty()) return null
        return runCatching { BitmapFactory.decodeByteArray(bytes, 0, bytes.size) }.getOrNull()
    }
}

/** Parse the JSON array returned by [EmulatorBridge.nativeScan] into models. */
private fun String.parseApps(): List<AppEntry> {
    val arr = runCatching { JSONArray(this) }.getOrNull() ?: return emptyList()
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

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AppGridScreen(
    ipaDir: File,
    scan: () -> List<AppEntry>,
    icon: (AppEntry) -> Bitmap?,
    launch: (AppEntry) -> Int,
) {
    val apps = remember { mutableStateListOf<AppEntry>() }
    val icons = remember { mutableStateMapOf<String, Bitmap?>() }
    var status by remember { mutableStateOf("Scanning ${ipaDir.name}…") }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        val found = withContext(Dispatchers.IO) { scan() }
        apps.clear()
        apps.addAll(found)
        status = if (found.isEmpty()) "No .ipa files in ${ipaDir.absolutePath}"
        else "${found.size} app(s)"
        found.forEach { entry ->
            withContext(Dispatchers.IO) { icons[entry.ipaPath] = icon(entry) }
        }
    }

    Scaffold(
        topBar = { TopAppBar(title = { Text("iOS Apps — $status") }) },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        LazyVerticalGrid(
            columns = GridCells.Adaptive(minSize = 96.dp),
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(12.dp),
            horizontalArrangement = Arrangement.spacedBy(12.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            items(apps, key = { it.ipaPath }) { entry ->
                AppTile(entry, icons[entry.ipaPath]) {
                    scope.launch {
                        snackbar.showSnackbar("Launching ${entry.name}…")
                        val code = withContext(Dispatchers.IO) { launch(entry) }
                        snackbar.showSnackbar("${entry.name} exited with code $code")
                    }
                }
            }
        }
    }
}

@Composable
private fun AppTile(entry: AppEntry, icon: Bitmap?, onClick: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(1f)
                .clip(RoundedCornerShape(18.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant),
            contentAlignment = Alignment.Center,
        ) {
            if (icon != null) {
                Image(
                    bitmap = icon.asImageBitmap(),
                    contentDescription = entry.name,
                    modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(18.dp)),
                )
            } else {
                Text(
                    entry.name.take(1).uppercase(),
                    style = MaterialTheme.typography.headlineMedium,
                )
            }
        }
        Text(
            text = entry.name,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            textAlign = TextAlign.Center,
            style = MaterialTheme.typography.bodySmall,
            modifier = Modifier.fillMaxWidth().padding(top = 6.dp),
        )
    }
}
