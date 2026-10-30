// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

package org.hblctl.android

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.ui.Modifier

/** Every operation uses this form; field behavior comes only from its schema. */
@Composable
fun SchemaForm(fields: List<FieldModel>, values: Map<String, String>, onValue: (String, String) -> Unit) {
    fields.forEach { field ->
        val value = values[field.name] ?: field.initial
        val error = SchemaModel.error(field, value)
        OutlinedTextField(value = value, onValueChange = { onValue(field.name, it) },
            label = { Text(field.name + if (field.required) " *" else "") },
            supportingText = { Text(listOfNotNull(field.hint, error).joinToString("\n")) },
            isError = error != null, modifier = Modifier.fillMaxWidth())
    }
}
