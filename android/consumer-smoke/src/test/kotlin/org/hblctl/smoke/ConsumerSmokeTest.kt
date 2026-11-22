// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

package org.hblctl.smoke

import java.nio.file.Files
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import uniffi.hblctl_ffi.ByteStream
import uniffi.hblctl_ffi.Registry
import uniffi.hblctl_ffi.Safety
import uniffi.hblctl_ffi.Session
import uniffi.hblctl_ffi.StreamException
import uniffi.hblctl_ffi.Exception as HblctlException

/** Uses the AAR the way an app does: resolved from Maven, called through its generated Kotlin. */
class ConsumerSmokeTest {
    @Test
    fun registryLoadsThroughTheBindings() {
        Registry().use { registry ->
            val commands = registry.commandsJson()
            assertTrue("registry JSON should list commands", commands.contains("\"commands\""))
            val (command, func) = firstReadOperation(commands)
            assertTrue(registry.inputSchema(command, func).startsWith("{"))
        }
    }

    @Test
    fun appSuppliedStreamCarriesTheSession() {
        val closes = AtomicInteger()
        val sent = AtomicInteger()
        // The camera end hangs up at once, so the first command fails with a transport error.
        val stream = object : ByteStream {
            override fun send(bytes: ByteArray) { sent.incrementAndGet() }
            override fun recv(timeoutMs: ULong): ByteArray? = null
            override fun close() { closes.incrementAndGet() }
        }
        val dir = Files.createTempDirectory("hblctl-smoke").resolve("session").toString()
        val (command, func) = Registry().use { firstReadOperation(it.commandsJson()) }
        Session.connectStream(stream, "smoke test", 500uL, Safety.READ, dir).use { session ->
            assertTrue(session.isConnected())
            try {
                session.invoke(command, func, "{}")
                fail("a closed stream must fail the call")
            } catch (e: HblctlException.Transport) {
                assertTrue(e.detail, e.detail.contains("closed"))
            }
            assertFalse(session.isConnected())
        }
        assertEquals(1, sent.get())
        assertEquals("close() runs exactly once", 1, closes.get())
    }

    @Test
    fun streamTimeoutsBecomeTransportErrors() {
        val stream = object : ByteStream {
            override fun send(bytes: ByteArray) {}
            override fun recv(timeoutMs: ULong): ByteArray? = throw StreamException.Timeout()
            override fun close() {}
        }
        val dir = Files.createTempDirectory("hblctl-smoke").resolve("session").toString()
        val (command, func) = Registry().use { firstReadOperation(it.commandsJson()) }
        Session.connectStream(stream, "smoke test", 250uL, Safety.READ, dir).use { session ->
            try {
                session.invoke(command, func, "{}")
                fail("a timed-out stream must fail the call")
            } catch (e: HblctlException.Transport) {
                assertTrue(e.detail, e.detail.contains("timed out"))
            }
        }
    }

    /** The first read-only operation that takes no arguments, as (command, func). */
    private fun firstReadOperation(registryJson: String): Pair<String, String> {
        val commands = Json.parseToJsonElement(registryJson).jsonObject.getValue("commands").jsonArray
        for (command in commands.map { it.jsonObject }) {
            for (func in command.getValue("funcs").jsonArray.map { it.jsonObject }) {
                val takesNothing = func["request"]?.jsonArray?.isEmpty() ?: true
                if (func.getValue("safety").jsonPrimitive.content == "read" && takesNothing) {
                    return command.getValue("name").jsonPrimitive.content to func.getValue("name").jsonPrimitive.content
                }
            }
        }
        error("the registry has no argument-free read operation")
    }
}
