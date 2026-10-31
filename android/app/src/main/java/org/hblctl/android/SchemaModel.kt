// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

package org.hblctl.android

import java.math.BigInteger
import kotlinx.serialization.json.*

/** Pure schema -> fields -> validation -> JSON pipeline, independent of Android. */
data class FieldModel(
    val name: String, val type: String, val description: String,
    val minimum: BigInteger?, val maximum: BigInteger?, val maxLength: Int?,
    val pattern: String?, val required: Boolean, val default: JsonElement?,
    val names: Set<String> = emptySet(), val integers: Set<BigInteger>? = null
) {
    val initial: String get() = (default as? JsonPrimitive)?.content ?: ""
    val hint: String get() = listOfNotNull(description.takeIf { it.isNotEmpty() },
        minimum?.let { "range $it … $maximum" }, maxLength?.let { "max length $it" },
        pattern?.let { "hex bytes (pairs of digits)" },
        names.takeIf { it.isNotEmpty() }?.let { "integer or name: ${it.joinToString()}" }).joinToString("; ")
}
object SchemaModel {
    fun fields(schema: String): List<FieldModel> {
        val root = Json.parseToJsonElement(schema).jsonObject
        val required = root["required"]?.jsonArray?.map { it.jsonPrimitive.content }?.toSet().orEmpty()
        return root.getValue("properties").jsonObject.map { (name, element) ->
            val p = element.jsonObject
            val alternatives = p["anyOf"]?.jsonArray?.map { it.jsonObject }
            val integer = alternatives?.single { it["type"]?.jsonPrimitive?.content == "integer" }
            val names = alternatives?.single { it["type"]?.jsonPrimitive?.content == "string" }
                ?.getValue("enum")?.jsonArray?.map { it.jsonPrimitive.content }?.toSet().orEmpty()
            val value = integer ?: p
            FieldModel(name, value.getValue("type").jsonPrimitive.content,
                p["description"]?.jsonPrimitive?.content.orEmpty(),
                value["minimum"]?.jsonPrimitive?.content?.toBigInteger(),
                value["maximum"]?.jsonPrimitive?.content?.toBigInteger(),
                p["maxLength"]?.jsonPrimitive?.int, p["pattern"]?.jsonPrimitive?.content,
                name in required, p["default"], names,
                value["enum"]?.jsonArray?.map { it.jsonPrimitive.content.toBigInteger() }?.toSet())
        }
    }
    fun error(field: FieldModel, text: String): String? {
        if (text.isEmpty() && !field.required && field.default == null) return null
        if (field.type == "integer") {
            if (text in field.names) return null
            val number = text.toBigIntegerOrNull() ?: return "Enter an integer"
            if (field.integers != null && number !in field.integers) return "Choose a listed integer or name"
            if (field.minimum != null && number < field.minimum || field.maximum != null && number > field.maximum) return "Outside allowed range"
        } else {
            if ('\u0000' in text) return "NUL is not allowed"
            if (field.maxLength != null && text.codePointCount(0, text.length) > field.maxLength) return "Text is too long"
            if (field.pattern != null && !Regex(field.pattern).matches(text)) return "Enter complete hex byte pairs"
        }
        return null
    }
    fun args(fields: List<FieldModel>, values: Map<String, String>): String = buildJsonObject {
        fields.forEach { f ->
            val text = values[f.name] ?: f.initial
            require(error(f, text) == null) { "${f.name}: ${error(f, text)}" }
            if (text.isEmpty() && !f.required && f.default == null) return@forEach
            if (f.type == "integer" && text !in f.names) put(f.name, Json.parseToJsonElement(text.toBigInteger().toString()))
            else if (text.isNotEmpty() || f.required || f.default != null) put(f.name, text)
        }
    }.toString()
}
