package com.iosemu.emulator.ui

import android.graphics.Bitmap
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
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
import com.iosemu.emulator.AppManager
import com.iosemu.emulator.model.AppEntry
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * The single-screen launcher: renders a grid of discovered iOS apps and boots
 * the tapped one on a background thread. All native work (scan, icon decode,
 * launch) is dispatched off the main thread.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val manager = AppManager(this)
        setContent {
            MaterialTheme {
                AppGridScreen(manager)
            }
        }
    }
}

@Composable
private fun AppGridScreen(manager: AppManager) {
    val apps = remember { mutableStateListOf<AppEntry>() }
    val icons = remember { mutableStateMapOf<String, Bitmap?>() }
    var status by remember { mutableStateOf("Scanning ${manager.ipaDirectory.name}…") }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    // Initial (and only automatic) scan.
    LaunchedEffect(Unit) {
        val found = withContext(Dispatchers.IO) { manager.scan() }
        apps.clear()
        apps.addAll(found)
        status = if (found.isEmpty()) {
            "No .ipa files in ${manager.ipaDirectory.absolutePath}"
        } else {
            "${found.size} app(s)"
        }
        // Decode icons lazily in the background.
        found.forEach { entry ->
            withContext(Dispatchers.IO) { icons[entry.ipaPath] = manager.icon(entry) }
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
                        val code = withContext(Dispatchers.IO) { manager.launch(entry) }
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
