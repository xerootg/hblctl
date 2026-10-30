// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! The command registry: a data description of every command, its sub-functions and their field layouts.
//!
//! Everything a frontend shows (CLI subcommands and flags, TUI forms, MCP tools and their JSON Schemas) is
//! derived from this data, so supporting a new command or field means editing the registry file only.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::safety::Safety;

/// Scalar and buffer types a field can have. All integers are little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    /// Unsigned 8-bit.
    U8,
    /// Unsigned 16-bit.
    U16,
    /// Unsigned 32-bit.
    U32,
    /// Signed 32-bit.
    S32,
    /// Unsigned 64-bit.
    U64,
    /// Raw bytes of fixed length `len`.
    Bytes,
    /// UTF-8 text in a fixed `len`-byte buffer, NUL-terminated when shorter.
    Cstr,
    /// Text as `len` little-endian 32-bit Unicode code points, zero-terminated when shorter.
    Utf32,
}

impl FieldType {
    /// Encoded width, or `None` for length-carrying types.
    #[must_use]
    pub fn width(self) -> Option<usize> {
        match self {
            FieldType::U8 => Some(1),
            FieldType::U16 => Some(2),
            FieldType::U32 | FieldType::S32 => Some(4),
            FieldType::U64 => Some(8),
            FieldType::Bytes | FieldType::Cstr | FieldType::Utf32 => None,
        }
    }
}

/// One field in a request or response, at an absolute record offset (0x10..0xFC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// Argument / result key, `snake_case`.
    pub name: String,
    /// Absolute offset in the 252-byte record.
    pub offset: usize,
    /// Type.
    #[serde(rename = "type")]
    pub ty: FieldType,
    /// Length for `bytes` and `cstr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    /// Value used when the caller omits the argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Human description.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub doc: String,
}

impl Field {
    /// Bytes the field occupies.
    #[must_use]
    pub fn size(&self) -> usize {
        match self.ty {
            FieldType::Utf32 => 4 * self.len.unwrap_or(0),
            ty => ty.width().unwrap_or(self.len.unwrap_or(0)),
        }
    }
}

/// One sub-function of a command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FuncSpec {
    /// Wire value of `func`.
    pub func: u32,
    /// Name used by frontends, `kebab-case`.
    pub name: String,
    /// What this function can change.
    pub safety: Safety,
    /// Human description.
    #[serde(default)]
    pub doc: String,
    /// Arguments.
    #[serde(default)]
    pub request: Vec<Field>,
    /// Result fields (beyond the status word).
    #[serde(default)]
    pub response: Vec<Field>,
    /// True only when request and response layouts are established; frontends mark the rest experimental.
    #[serde(default)]
    pub verified: bool,
}

/// One command id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Wire value of `cmd`.
    pub id: u32,
    /// Name used by frontends, `kebab-case`.
    pub name: String,
    /// Human description.
    #[serde(default)]
    pub doc: String,
    /// Sub-functions.
    #[serde(default)]
    pub funcs: Vec<FuncSpec>,
}

/// A whole registry file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    /// Camera model and firmware this registry describes, e.g. `"X2D 100C 4.2.0"`.
    pub target: String,
    /// Commands.
    pub commands: Vec<CommandSpec>,
}

/// The registry for the original Hasselblad X2D 100C on firmware 4.2.0, built into the crate.
pub const X2D_4_2_0: &str = include_str!("../registry/x2d-4.2.0.json");

/// The built-in registry.
///
/// # Panics
///
/// Only if the shipped file is invalid, which the test suite rules out.
#[must_use]
pub fn builtin() -> Registry {
    Registry::from_json(X2D_4_2_0).expect("built-in registry is validated by the test suite")
}

/// A registry file failed validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

impl Registry {
    /// Parse and validate JSON.
    pub fn from_json(s: &str) -> Result<Self, Invalid> {
        let r: Registry = serde_json::from_str(s).map_err(|e| Invalid(format!("registry JSON: {e}")))?;
        r.validate()?;
        Ok(r)
    }

    /// Check names are unique and every field fits the data region without overlapping its neighbours.
    pub fn validate(&self) -> Result<(), Invalid> {
        let mut names = std::collections::HashSet::new();
        for c in &self.commands {
            if !names.insert(&c.name) {
                return Err(Invalid(format!("duplicate command name {}", c.name)));
            }
            let mut fnames = std::collections::HashSet::new();
            for f in &c.funcs {
                if !fnames.insert(&f.name) {
                    return Err(Invalid(format!("{}: duplicate func name {}", c.name, f.name)));
                }
                for (side, fields) in [("request", &f.request), ("response", &f.response)] {
                    check_layout(&format!("{}.{} {side}", c.name, f.name), fields)?;
                }
            }
        }
        Ok(())
    }

    /// Look a command up by name (case-insensitive) or decimal id.
    #[must_use]
    pub fn command(&self, key: &str) -> Option<&CommandSpec> {
        let id = key.parse::<u32>().ok();
        self.commands
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(key) || Some(c.id) == id)
    }

    /// Every (command, function) pair, the unit every frontend exposes.
    pub fn operations(&self) -> impl Iterator<Item = (&CommandSpec, &FuncSpec)> {
        self.commands
            .iter()
            .flat_map(|c| c.funcs.iter().map(move |f| (c, f)))
    }
}

impl CommandSpec {
    /// Look a function up by name (case-insensitive) or decimal value.
    #[must_use]
    pub fn func(&self, key: &str) -> Option<&FuncSpec> {
        let n = key.parse::<u32>().ok();
        self.funcs
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(key) || Some(f.func) == n)
    }
}

fn check_layout(what: &str, fields: &[Field]) -> Result<(), Invalid> {
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    for f in fields {
        if matches!(f.ty, FieldType::Bytes | FieldType::Cstr | FieldType::Utf32) && f.len.unwrap_or(0) == 0 {
            return Err(Invalid(format!("{what}: field {} needs a non-zero len", f.name)));
        }
        let (start, end) = (f.offset, f.offset + f.size());
        if start < 0x10 || end > 0xFC {
            return Err(Invalid(format!(
                "{what}: field {} spans 0x{start:x}..0x{end:x}, outside 0x10..0xFC",
                f.name
            )));
        }
        if let Some((_, _, other)) = spans.iter().find(|(s, e, _)| start < *e && *s < end) {
            return Err(Invalid(format!("{what}: field {} overlaps {other}", f.name)));
        }
        spans.push((start, end, &f.name));
    }
    Ok(())
}

/// JSON Schema for a function's arguments: drives MCP tool input schemas, CLI flags and TUI forms.
#[must_use]
pub fn input_schema(f: &FuncSpec) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for fld in &f.request {
        let mut s = match fld.ty {
            FieldType::U8 => json!({"type": "integer", "minimum": 0, "maximum": u8::MAX}),
            FieldType::U16 => json!({"type": "integer", "minimum": 0, "maximum": u16::MAX}),
            FieldType::U32 => json!({"type": "integer", "minimum": 0, "maximum": u32::MAX}),
            FieldType::S32 => json!({"type": "integer", "minimum": i32::MIN, "maximum": i32::MAX}),
            FieldType::U64 => json!({"type": "integer", "minimum": 0}),
            FieldType::Bytes => json!({"type": "string", "pattern": "^([0-9a-fA-F]{2})*$",
                "maxLength": 2 * fld.len.unwrap_or(0), "description": "hex"}),
            FieldType::Cstr => json!({"type": "string", "maxLength": fld.len.unwrap_or(0).saturating_sub(1)}),
            FieldType::Utf32 => json!({"type": "string", "maxLength": fld.len.unwrap_or(0).saturating_sub(1),
                "description": "characters (Unicode code points)"}),
        };
        if !fld.doc.is_empty() {
            s["description"] = Value::String(fld.doc.clone());
        }
        if let Some(d) = &fld.default {
            s["default"] = d.clone();
        } else {
            required.push(Value::String(fld.name.clone()));
        }
        props.insert(fld.name.clone(), s);
    }
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small registry used across the crate's tests. Ids and layouts are illustrative.
    pub(crate) const SAMPLE: &str = r#"{
      "target": "test",
      "commands": [
        {"id": 49, "name": "prod-config", "doc": "Factory parameters", "funcs": [
          {"func": 2, "name": "get", "safety": "read", "verified": true,
           "request": [{"name": "field", "offset": 20, "type": "u32", "doc": "field id"}],
           "response": [{"name": "field", "offset": 20, "type": "u32"}, {"name": "value", "offset": 24, "type": "u32"}]},
          {"func": 1, "name": "set", "safety": "persistent",
           "request": [{"name": "field", "offset": 20, "type": "u32"}, {"name": "value", "offset": 24, "type": "s32"}]}
        ]},
        {"id": 30, "name": "firmware-version", "funcs": [
          {"func": 0, "name": "get", "safety": "read", "verified": true,
           "request": [{"name": "which", "offset": 20, "type": "u8", "default": 0}],
           "response": [{"name": "version", "offset": 20, "type": "cstr", "len": 32}, {"name": "raw", "offset": 52, "type": "bytes", "len": 4}]}
        ]}
      ]
    }"#;

    pub(crate) fn sample() -> Registry {
        Registry::from_json(SAMPLE).unwrap()
    }

    #[test]
    fn lookups() {
        let r = sample();
        assert_eq!(r.command("PROD-CONFIG").unwrap().id, 49);
        assert_eq!(r.command("30").unwrap().name, "firmware-version");
        assert!(r.command("nope").is_none());
        let pc = r.command("prod-config").unwrap();
        assert_eq!(pc.func("set").unwrap().func, 1);
        assert_eq!(pc.func("2").unwrap().name, "get");
        assert_eq!(r.operations().count(), 3);
    }

    #[test]
    fn rejects_bad_layouts() {
        let overlap = SAMPLE.replace(
            r#""name": "value", "offset": 24, "type": "s32""#,
            r#""name": "value", "offset": 22, "type": "s32""#,
        );
        assert!(Registry::from_json(&overlap).unwrap_err().0.contains("overlaps"));
        let outside = SAMPLE.replace(
            r#""offset": 52, "type": "bytes", "len": 4"#,
            r#""offset": 250, "type": "bytes", "len": 4"#,
        );
        assert!(Registry::from_json(&outside).unwrap_err().0.contains("outside"));
        let header = SAMPLE.replace(
            r#""name": "which", "offset": 20"#,
            r#""name": "which", "offset": 8"#,
        );
        assert!(Registry::from_json(&header).is_err());
        let dup = SAMPLE.replace(r#""name": "firmware-version""#, r#""name": "prod-config""#);
        assert!(Registry::from_json(&dup).unwrap_err().0.contains("duplicate"));
    }

    #[test]
    fn schema_from_fields() {
        let r = sample();
        let s = input_schema(r.command("prod-config").unwrap().func("set").unwrap());
        assert_eq!(s["required"], json!(["field", "value"]));
        assert_eq!(s["properties"]["value"]["minimum"], json!(i32::MIN));
        let s = input_schema(r.command("firmware-version").unwrap().func("get").unwrap());
        assert_eq!(s["required"], json!([]));
        assert_eq!(s["properties"]["which"]["default"], json!(0));
        assert_eq!(s["additionalProperties"], json!(false));
    }
}
