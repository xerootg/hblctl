// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Encodes JSON arguments into a record's data region and decodes result fields back to JSON, using the
//! field layouts in the [`registry`](crate::registry).

use hblctl_proto::Sutest;
use hblctl_proto::sutest::{DATA_OFFSET, DATA_SIZE};
use serde_json::{Map, Value, json};

use crate::registry::{Field, FieldType, FuncSpec};

/// An argument did not fit its field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgError {
    /// A required argument is missing.
    #[error("missing argument `{0}`")]
    Missing(String),
    /// An argument the function does not take.
    #[error("unknown argument `{0}`")]
    Unknown(String),
    /// Wrong type or out of range.
    #[error("argument `{name}`: {why}")]
    Invalid {
        /// Argument name.
        name: String,
        /// What was wrong.
        why: String,
    },
}

fn invalid(f: &Field, why: impl Into<String>) -> ArgError {
    ArgError::Invalid {
        name: f.name.clone(),
        why: why.into(),
    }
}

/// Build the 236-byte data region (record bytes 0x10..0xFC) from `args`.
pub fn encode(func: &FuncSpec, args: &Map<String, Value>) -> Result<[u8; DATA_SIZE], ArgError> {
    if let Some(k) = args.keys().find(|k| !func.request.iter().any(|f| &f.name == *k)) {
        return Err(ArgError::Unknown(k.clone()));
    }
    let mut data = [0u8; DATA_SIZE];
    for f in &func.request {
        let Some(v) = args.get(&f.name).or(f.default.as_ref()) else {
            return Err(ArgError::Missing(f.name.clone()));
        };
        let bytes = field_bytes(f, v)?;
        let start = f.offset - DATA_OFFSET;
        data[start..start + bytes.len()].copy_from_slice(&bytes);
    }
    Ok(data)
}

fn int(f: &Field, v: &Value) -> Result<i128, ArgError> {
    match v {
        Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)),
        Value::String(s) => parse_int(s),
        _ => None,
    }
    .ok_or_else(|| invalid(f, format!("expected an integer, got {v}")))
}

/// Decimal, or hex with a `0x` prefix.
fn parse_int(s: &str) -> Option<i128> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => i128::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

fn ranged(f: &Field, v: &Value, lo: i128, hi: i128) -> Result<i128, ArgError> {
    let n = int(f, v)?;
    if (lo..=hi).contains(&n) {
        Ok(n)
    } else {
        Err(invalid(f, format!("{n} is outside {lo}..={hi}")))
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // every cast is preceded by a range check
fn field_bytes(f: &Field, v: &Value) -> Result<Vec<u8>, ArgError> {
    Ok(match f.ty {
        FieldType::U8 => vec![ranged(f, v, 0, u8::MAX.into())? as u8],
        FieldType::U16 => (ranged(f, v, 0, u16::MAX.into())? as u16).to_le_bytes().to_vec(),
        FieldType::U32 => (ranged(f, v, 0, u32::MAX.into())? as u32).to_le_bytes().to_vec(),
        FieldType::S32 => (ranged(f, v, i32::MIN.into(), i32::MAX.into())? as i32)
            .to_le_bytes()
            .to_vec(),
        FieldType::U64 => (ranged(f, v, 0, u64::MAX.into())? as u64).to_le_bytes().to_vec(),
        FieldType::Bytes => {
            let s = v.as_str().ok_or_else(|| invalid(f, "expected a hex string"))?;
            let b = from_hex(s).ok_or_else(|| invalid(f, "not valid hex"))?;
            let len = f.len.unwrap_or(0);
            if b.len() > len {
                return Err(invalid(f, format!("{} bytes, field holds {len}", b.len())));
            }
            b
        }
        FieldType::Cstr => {
            let s = v.as_str().ok_or_else(|| invalid(f, "expected a string"))?;
            if s.as_bytes().contains(&0) {
                return Err(invalid(f, "contains a NUL byte"));
            }
            let len = f.len.unwrap_or(0);
            // Leave room for the terminator so the camera never reads past the buffer.
            if s.len() >= len {
                return Err(invalid(
                    f,
                    format!(
                        "{} bytes, field holds {} plus NUL",
                        s.len(),
                        len.saturating_sub(1)
                    ),
                ));
            }
            let mut b = s.as_bytes().to_vec();
            b.push(0);
            b
        }
    })
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Decode a function's response fields from a result record.
#[must_use]
pub fn decode(func: &FuncSpec, record: &Sutest) -> Map<String, Value> {
    let mut out = Map::new();
    for f in &func.response {
        let Ok(b) = record.get(f.offset, f.size()) else {
            continue;
        };
        let v = match f.ty {
            FieldType::U8 => json!(b[0]),
            FieldType::U16 => json!(u16::from_le_bytes([b[0], b[1]])),
            FieldType::U32 => json!(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            FieldType::S32 => json!(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            FieldType::U64 => json!(u64::from_le_bytes([
                b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]
            ])),
            FieldType::Bytes => json!(crate::hex(b)),
            FieldType::Cstr => {
                let end = b.iter().position(|&x| x == 0).unwrap_or(b.len());
                json!(String::from_utf8_lossy(&b[..end]))
            }
        };
        out.insert(f.name.clone(), v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::sample;

    fn args(v: &Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn encodes_at_absolute_offsets() {
        let r = sample();
        let set = r.command("prod-config").unwrap().func("set").unwrap();
        let d = encode(set, &args(&json!({"field": 13, "value": -2}))).unwrap();
        assert_eq!(&d[20 - 16..24 - 16], &13u32.to_le_bytes());
        assert_eq!(&d[24 - 16..28 - 16], &(-2i32).to_le_bytes());
        let d2 = encode(set, &args(&json!({"field": "0x0d", "value": "-2"}))).unwrap();
        assert_eq!(d, d2, "hex and decimal strings are accepted");
    }

    #[test]
    fn argument_errors() {
        let r = sample();
        let set = r.command("prod-config").unwrap().func("set").unwrap();
        assert_eq!(
            encode(set, &args(&json!({"field": 1}))),
            Err(ArgError::Missing("value".into()))
        );
        assert_eq!(
            encode(set, &args(&json!({"field": 1, "value": 1, "x": 1}))),
            Err(ArgError::Unknown("x".into()))
        );
        assert!(matches!(
            encode(set, &args(&json!({"field": -1, "value": 1}))),
            Err(ArgError::Invalid { .. })
        ));
        assert!(matches!(
            encode(set, &args(&json!({"field": 1, "value": 3_000_000_000u64}))),
            Err(ArgError::Invalid { .. })
        ));
        assert!(matches!(
            encode(set, &args(&json!({"field": "abc", "value": 1}))),
            Err(ArgError::Invalid { .. })
        ));
    }

    #[test]
    fn defaults_and_strings() {
        let r = sample();
        let get = r.command("firmware-version").unwrap().func("get").unwrap();
        assert_eq!(encode(get, &Map::new()).unwrap()[4], 0);
        let mut rec = Sutest::new(30, 0, 1);
        rec.put(20, b"4.2.0\0garbage").unwrap();
        rec.put(52, &[0xde, 0xad, 0xbe, 0xef]).unwrap();
        let out = decode(get, &rec);
        assert_eq!(out["version"], json!("4.2.0"));
        assert_eq!(out["raw"], json!("deadbeef"));
    }

    #[test]
    fn cstr_and_bytes_limits() {
        let f = |ty, len| Field {
            name: "s".into(),
            offset: 20,
            ty,
            len: Some(len),
            default: None,
            doc: String::new(),
        };
        assert_eq!(
            field_bytes(&f(FieldType::Cstr, 4), &json!("abc")).unwrap(),
            b"abc\0"
        );
        assert!(
            field_bytes(&f(FieldType::Cstr, 4), &json!("abcd")).is_err(),
            "no room for NUL"
        );
        assert!(field_bytes(&f(FieldType::Cstr, 8), &json!("a\u{0}b")).is_err());
        assert_eq!(
            field_bytes(&f(FieldType::Bytes, 2), &json!("ab CD")).unwrap(),
            vec![0xab, 0xcd]
        );
        assert!(field_bytes(&f(FieldType::Bytes, 1), &json!("abcd")).is_err());
        assert!(field_bytes(&f(FieldType::Bytes, 4), &json!("abc")).is_err());
    }
}
