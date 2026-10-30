// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

package org.hblctl.android

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import java.io.File
import java.util.UUID
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import uniffi.hblctl_ffi.Registry
import uniffi.hblctl_ffi.Session
import uniffi.hblctl_ffi.Safety
import uniffi.hblctl_ffi.StatusCallback
import uniffi.hblctl_ffi.dump

private data class Operation(val command: String, val function: String, val doc: String,
    val safety: Safety, val verified: Boolean) {
    val name get() = "$command.$function"
}
private fun operations(registry: Registry): List<Operation> =
    Json.parseToJsonElement(registry.commandsJson()).jsonObject.getValue("commands").jsonArray.flatMap { c ->
        val command = c.jsonObject
        command.getValue("funcs").jsonArray.map { f ->
            val function = f.jsonObject
            Operation(command.getValue("name").jsonPrimitive.content, function.getValue("name").jsonPrimitive.content,
                listOf(command["doc"]?.jsonPrimitive?.content.orEmpty(), function["doc"]?.jsonPrimitive?.content.orEmpty()).joinToString("\n"),
                Safety.valueOf(function.getValue("safety").jsonPrimitive.content.uppercase()),
                function["verified"]?.jsonPrimitive?.boolean ?: false)
        }
    }

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MaterialTheme { App() } }
    }
}

@Composable
private fun App() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val registry = remember { Registry() }
    val ops = remember { operations(registry) }
    var session by remember { mutableStateOf<Session?>(null) }
    DisposableEffect(Unit) { onDispose { val old = session; CoroutineScope(Dispatchers.IO).launch { old?.disconnect(); old?.close() }; registry.close() } }
    var ceiling by remember { mutableStateOf(Safety.READ) }
    var host by remember { mutableStateOf("192.168.42.2") }
    var port by remember { mutableStateOf("30303") }
    var page by remember { mutableStateOf("Connect") }
    var query by remember { mutableStateOf("") }
    var selected by remember { mutableStateOf<Operation?>(null) }
    var values by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var confirmation by remember { mutableStateOf("") }
    var output by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var connected by remember { mutableStateOf(false) }
    var statusText by remember { mutableStateOf("") }
    var directories by remember { mutableStateOf<List<File>>(emptyList()) }
    val sessionsRoot = remember { File(context.filesDir, "sessions").also { it.mkdirs() } }
    fun work(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        scope.launch {
            try { block() } catch (e: kotlin.Exception) { statusText = e.message ?: e.toString() }
            finally { busy = false }
        }
    }
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.padding(16.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("hblctl", style = MaterialTheme.typography.headlineMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                listOf("Connect", "Operations", "Sessions").forEach { name ->
                    TextButton(enabled = !busy, onClick = {
                        page = name
                        output = ""
                        if (name == "Sessions") directories = sessionsRoot.listFiles()?.filter { it.isDirectory }?.sortedByDescending { it.name }.orEmpty()
                    }) { Text(name) }
                }
            }
            Text(if (connected) "Connected" else "Disconnected")
            if (busy) LinearProgressIndicator(Modifier.fillMaxWidth())
            if (statusText.isNotEmpty()) Text(statusText)
            when (page) {
                "Connect" -> {
                    OutlinedTextField(host, { host = it }, label = { Text("Host") }, enabled = !connected && !busy)
                    OutlinedTextField(port, { port = it }, label = { Text("Port") }, enabled = !connected && !busy)
                    Text("The camera accepts one connection at a time. Another client replaces this connection.")
                    val explanations = listOf("Read: queries only", "Volatile: changes lost at restart", "Persistent: settings retained after restart", "Destructive: may irreversibly damage the camera or erase factory data")
                    Safety.entries.forEachIndexed { i, safety ->
                        Row {
                            RadioButton(selected = ceiling == safety, enabled = !connected && !busy,
                                onClick = { ceiling = safety })
                            Text(explanations[i], Modifier.padding(top = 12.dp))
                        }
                    }
                    Button(enabled = !busy && (connected || host.isNotBlank() && (port.toIntOrNull()?.let { it in 1..65535 } == true)), onClick = {
                        work {
                            if (connected) {
                                withContext(Dispatchers.IO) { session?.disconnect(); session?.close() }
                                session = null; connected = false
                            } else {
                                withContext(Dispatchers.IO) { session?.close() }
                                session = null
                                val dir = File(sessionsRoot, "${System.currentTimeMillis()}-${UUID.randomUUID()}")
                                val opened = withContext(Dispatchers.IO) { Session.connect(host.trim(), port.toUShort(), 5000uL, ceiling, dir.absolutePath) }
                                opened.setStatusCallback(object : StatusCallback {
                                    override fun status(message: String) { scope.launch { statusText = if (message.startsWith("{")) "Operation completed; result below" else message } }
                                })
                                session = opened; connected = true; page = "Operations"; statusText = "Session: ${dir.name}"
                            }
                        }
                    }) { Text(if (connected) "Disconnect" else "Connect") }
                }
                "Operations" -> {
                    if (selected == null) {
                        OutlinedTextField(query, { query = it }, label = { Text("Search operations") })
                        ops.filter { (it.name + it.doc).contains(query, ignoreCase = true) }.groupBy { it.command }.forEach { (command, group) ->
                            Text(command, style = MaterialTheme.typography.titleLarge)
                            group.forEach { op ->
                                TextButton(enabled = !busy && op.safety.ordinal <= ceiling.ordinal, onClick = {
                                    selected = op; values = emptyMap(); confirmation = ""; output = ""
                                }) { Text("${op.function} [${op.safety.name.lowercase()}]${if (!op.verified) " • unverified" else ""}") }
                            }
                        }
                    } else {
                        val op = selected!!
                        TextButton(enabled = !busy, onClick = { selected = null }) { Text("Back to operations") }
                        Text(op.name, style = MaterialTheme.typography.titleLarge)
                        Text("${op.safety.name.lowercase()}${if (!op.verified) " • layout unverified" else ""}")
                        Text(op.doc)
                        val fields = remember(op) { SchemaModel.fields(registry.inputSchema(op.command, op.function)) }
                        SchemaForm(fields, values) { name, value -> values = values + (name to value) }
                        if (op.safety == Safety.DESTRUCTIVE) OutlinedTextField(confirmation, { confirmation = it }, label = { Text("Type ${op.name} to confirm") })
                        Button(enabled = connected && !busy && op.safety.ordinal <= ceiling.ordinal && fields.all { SchemaModel.error(it, values[it.name] ?: it.initial) == null } &&
                            (op.safety != Safety.DESTRUCTIVE || confirmation == op.name), onClick = {
                            val args = SchemaModel.args(fields, values)
                            work {
                                try {
                                    val raw = withContext(Dispatchers.IO) { session!!.invoke(op.command, op.function, args) }
                                    output = Json { prettyPrint = true }.encodeToString(JsonElement.serializer(), Json.parseToJsonElement(raw))
                                    page = "Result"
                                } finally {
                                    confirmation = ""
                                    connected = withContext(Dispatchers.IO) { session?.isConnected() ?: false }
                                }
                            }
                        }) { Text("Invoke") }
                        Text("Result (decoded fields, raw result hex, progress and notifications)")
                        Text(output)
                    }
                }
                "Result" -> {
                    TextButton(onClick = { page = "Operations" }) { Text("Back to operation") }
                    Text("Result", style = MaterialTheme.typography.titleLarge)
                    Text("Decoded fields, raw result hex, progress and notifications")
                    Text(output)
                }
                "Sessions" -> {
                    directories.forEach { dir ->
                        Text(dir.name)
                        Row {
                            TextButton(enabled = !busy, onClick = { work { output = withContext(Dispatchers.IO) { dump(File(dir, "capture.hblcap").absolutePath).joinToString("\n") } } }) { Text("Open") }
                            TextButton(enabled = !busy && !connected, onClick = {
                                work {
                                    val zip = withContext(Dispatchers.IO) {
                                        val file = File(File(context.cacheDir, "exports").also { it.mkdirs() }, "${dir.name}.zip")
                                        ZipOutputStream(file.outputStream()).use { out ->
                                            dir.walkTopDown().filter { it.isFile }.forEach { entry ->
                                                out.putNextEntry(ZipEntry(entry.relativeTo(dir).invariantSeparatorsPath))
                                                entry.inputStream().use { it.copyTo(out) }; out.closeEntry()
                                            }
                                        }
                                        file
                                    }
                                    val uri = FileProvider.getUriForFile(context, "${context.packageName}.exports", zip)
                                    context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("application/zip").putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION), "Export session"))
                                }
                            }) { Text("Share ZIP") }
                        }
                    }
                    Text(output)
                }
            }
        }
    }
}
