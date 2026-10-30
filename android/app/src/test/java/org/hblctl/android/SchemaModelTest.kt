// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

package org.hblctl.android

import org.junit.Assert.*
import org.junit.Test
import kotlinx.serialization.json.*

class SchemaModelTest {
    private val schema = """{"properties":{"n":{"type":"integer","minimum":0,"maximum":18446744073709551615,"default":7},"bytes":{"type":"string","pattern":"^([0-9a-fA-F]{2})*$","maxLength":4},"text":{"type":"string","maxLength":2}},"required":["bytes","text"]}"""
    @Test fun defaultsAndUnsigned64StayExact() {
        val f = SchemaModel.fields(schema)
        assertEquals("7", f.first().initial)
        val args = Json.parseToJsonElement(SchemaModel.args(f, mapOf("n" to "18446744073709551615", "bytes" to "AB00", "text" to "😀a"))).jsonObject
        assertEquals("18446744073709551615", args.getValue("n").jsonPrimitive.content)
        assertEquals("😀a", args.getValue("text").jsonPrimitive.content)
    }
    @Test fun rejectsRangesHexAndLengths() {
        val f = SchemaModel.fields(schema)
        assertNotNull(SchemaModel.error(f[0], "18446744073709551616"))
        assertNotNull(SchemaModel.error(f[0], "-1"))
        assertNotNull(SchemaModel.error(f[0], "1.2"))
        assertNotNull(SchemaModel.error(f[1], "abc"))
        assertNotNull(SchemaModel.error(f[1], "001122"))
        assertNotNull(SchemaModel.error(f[2], "abc"))
        assertNotNull(SchemaModel.error(f[2], "a\u0000"))
        assertThrows(IllegalArgumentException::class.java) { SchemaModel.args(f, mapOf("bytes" to "xx")) }
    }
    @Test fun defaultsAndEscaping() {
        val f = SchemaModel.fields(schema)
        val args = Json.parseToJsonElement(SchemaModel.args(f, mapOf("bytes" to "", "text" to "\"\n"))).jsonObject
        assertEquals("7", args.getValue("n").jsonPrimitive.content)
        assertEquals("\"\n", args.getValue("text").jsonPrimitive.content)
    }
}
