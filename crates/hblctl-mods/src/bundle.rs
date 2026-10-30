// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Bounded archive loading, pinned schema validation and deterministic byte edits.
use crate::{Error, Result, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

/// SHA-256 encoded as lowercase hex.
#[must_use]
pub fn hash(bytes: &[u8]) -> String {
    let digest = format!("{:x}", Sha256::digest(bytes));
    tracing::trace!(target:"hblctl::mods", %digest, len=bytes.len(), "SHA-256");
    digest
}

pub(crate) fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub(crate) fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().map_or(&[], Vec::as_slice)
}

/// A validated passive manifest and its verified artifact bytes.
#[derive(Debug)]
pub struct Bundle {
    manifest: Value,
    members: BTreeMap<String, Vec<u8>>,
    digest: String,
}
impl Bundle {
    /// Load an archive and its mandatory `<archive>.sha256` corruption sidecar.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        ensure(
            bytes.len() <= 64 * 1024 * 1024,
            "compressed bundle exceeds 64 MiB",
        )?;
        let sidecar = std::fs::read_to_string(path.with_file_name(format!(
                "{}.sha256",
                path.file_name()
                    .ok_or_else(|| Error::Invalid("missing archive filename".into()))?
                    .to_string_lossy()
            )))?;
        let expected = sidecar.split_whitespace().next().unwrap_or("");
        ensure(hash(&bytes) == expected, "archive sidecar SHA-256 mismatch")?;
        let decoder = zstd::stream::read::Decoder::new(bytes.as_slice())?;
        let mut archive = tar::Archive::new(decoder.take(128 * 1024 * 1024 + 1));
        let mut members = BTreeMap::new();
        let mut total = 0u64;
        for entry in archive.entries()? {
            let mut entry = entry?;
            ensure(
                entry.header().entry_type().is_file(),
                "archive contains a non-regular member",
            )?;
            let name = entry
                .path()?
                .to_str()
                .ok_or_else(|| Error::Invalid("non UTF-8 member".into()))?
                .to_owned();
            safe_relative(&name)?;
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| Error::Invalid("archive size overflow".into()))?;
            ensure(total <= 128 * 1024 * 1024, "expanded bundle exceeds 128 MiB")?;
            let mut data = Vec::new();
            entry.read_to_end(&mut data)?;
            ensure(
                members.insert(name.clone(), data).is_none(),
                format!("duplicate archive member {name}"),
            )?;
        }
        let inventory: BTreeMap<String, String> = serde_json::from_slice(
            members
                .get("inventory.json")
                .ok_or_else(|| Error::Invalid("missing inventory.json".into()))?,
        )?;
        ensure(
            inventory.len() + 1 == members.len(),
            "inventory does not cover exact archive members",
        )?;
        for (name, digest) in inventory {
            ensure(
                members.get(&name).is_some_and(|b| hash(b) == digest),
                format!("inventory hash mismatch: {name}"),
            )?;
        }
        let manifest = serde_json::from_slice(
            members
                .get("manifest.json")
                .ok_or_else(|| Error::Invalid("missing manifest.json".into()))?,
        )?;
        Self::from_manifest(manifest, members)
    }
    /// Validate an in-memory manifest and artifacts against the engine's pinned schema.
    /// `members` must include the reviewed `schema.json`; archive integrity is checked by [`Self::load`].
    pub fn from_manifest(manifest: Value, members: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let validator = validator(&members)?;
        if let Some(e) = validator.iter_errors(&manifest).next() {
            return Err(Error::Invalid(format!("schema {}: {e}", e.instance_path)));
        }
        for kind in ["features", "files", "recipes", "steps"] {
            let mut ids = BTreeSet::new();
            for item in array(&manifest, kind) {
                ensure(
                    ids.insert(string(item, "id")),
                    format!("duplicate {kind} ID {}", string(item, "id")),
                )?;
            }
        }
        for feature in array(&manifest, "features") {
            for key in ["requires", "conflicts"] {
                for id in array(feature, key) {
                    find(&manifest, "features", id.as_str().unwrap_or(""))?;
                }
            }
            for id in array(feature, "files") {
                find(&manifest, "files", id.as_str().unwrap_or(""))?;
            }
            visit(
                &manifest,
                string(feature, "id"),
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
            )?;
        }
        for phase in ["install", "restore"] {
            let mut orders: Vec<_> = array(&manifest, "steps")
                .iter()
                .filter(|s| string(s, "phase") == phase)
                .map(|s| s["order"].as_u64().unwrap_or(u64::MAX))
                .collect();
            orders.sort_unstable();
            ensure(
                orders.iter().copied().eq(0..orders.len() as u64),
                format!("{phase} step order must be contiguous from zero"),
            )?;
        }
        for file in array(&manifest, "files") {
            safe_camera_path(string(file, "target"))?;
            if file.get("source").is_some() {
                let source = string(file, "source");
                safe_relative(source)?;
                if let Some(digest) = file["source_sha256"].as_str() {
                    ensure(
                        members
                            .get(&format!("sources/{source}"))
                            .or_else(|| members.get(source))
                            .is_some_and(|b| hash(b) == digest),
                        format!("source hash mismatch: {source}"),
                    )?;
                }
            }
            if let Some(id) = file["recipe"].as_str() {
                let r = find(&manifest, "recipes", id)?;
                ensure(
                    r["output_sha256"] == file["output_sha256"],
                    format!("recipe output mismatch: {id}"),
                )?;
            }
            if let Some(name) = file["artifact"].as_str() {
                safe_relative(name)?;
                ensure(
                    string(file, "kind") != "stock-recipe",
                    "stock derivatives cannot be archive artifacts",
                )?;
                ensure(
                    members
                        .get(name)
                        .is_some_and(|b| hash(b) == string(file, "output_sha256")),
                    format!("artifact hash mismatch: {name}"),
                )?;
            }
        }
        for r in array(&manifest, "recipes") {
            validate_edits(r)?;
            safe_camera_path(string(r, "input"))?;
        }
        safe_camera_path(string(&manifest["backup"], "root"))?;
        for stock in array(&manifest["preconditions"], "stock_hashes") {
            safe_camera_path(string(stock, "path"))?;
        }
        let digest = hash(&serde_json::to_vec(&manifest)?);
        tracing::info!(target:"hblctl::mods",%digest,"validated bundle");
        Ok(Self {
            manifest,
            members,
            digest,
        })
    }
    /// Full validated schema data, including preconditions, recipes and step safety classes.
    #[must_use]
    pub fn manifest(&self) -> &Value {
        &self.manifest
    }
    /// Stable identity of the validated manifest.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
    pub(crate) fn artifact(&self, name: &str) -> Result<Vec<u8>> {
        self.members
            .get(name)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("missing artifact {name}")))
    }
}
fn validator(members: &BTreeMap<String, Vec<u8>>) -> Result<&'static jsonschema::Validator> {
    const SCHEMA_SHA256: &str = "baa2773f68a40593c6820a0d10ed78c60340b5ee1c9ca7153e522c9ddff6d57f";
    static VALIDATOR: std::sync::OnceLock<std::result::Result<jsonschema::Validator, String>> =
        std::sync::OnceLock::new();
    let bytes = members
        .get("schema.json")
        .ok_or_else(|| Error::Invalid("missing schema.json".into()))?;
    ensure(
        hash(bytes) == SCHEMA_SHA256,
        "schema differs from reviewed engine contract",
    )?;
    let schema: Value = serde_json::from_slice(bytes)?;
    VALIDATOR
        .get_or_init(|| jsonschema::validator_for(&schema).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| Error::Invalid(e.clone()))
}
pub(crate) fn find<'a>(manifest: &'a Value, kind: &str, id: &str) -> Result<&'a Value> {
    array(manifest, kind)
        .iter()
        .find(|v| string(v, "id") == id)
        .ok_or_else(|| Error::Invalid(format!("unknown {kind} reference {id}")))
}
pub(crate) fn visit(
    m: &Value,
    id: &str,
    active: &mut BTreeSet<String>,
    done: &mut BTreeSet<String>,
) -> Result<()> {
    if done.contains(id) {
        return Ok(());
    }
    ensure(active.insert(id.into()), format!("dependency cycle at {id}"))?;
    for dep in array(find(m, "features", id)?, "requires") {
        visit(m, dep.as_str().unwrap_or(""), active, done)?;
    }
    active.remove(id);
    done.insert(id.into());
    Ok(())
}
pub(crate) fn safe_relative(path: &str) -> Result<()> {
    ensure(
        !path.is_empty()
            && !path.starts_with('/')
            && path.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
            && !path.contains('\\'),
        format!("unsafe archive path {path}"),
    )
}
/// Validate a literal camera path; only shell-inert characters and canonical components are accepted.
pub fn safe_camera_path(path: &str) -> Result<()> {
    ensure(
        path.starts_with('/')
            && path
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c))
            && path[1..]
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != ".."),
        format!("unsafe camera path {path}"),
    )
}
fn hex(s: &str) -> Result<Vec<u8>> {
    ensure(
        s.len().is_multiple_of(2)
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid recipe hex",
    )?;
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| Error::Invalid(e.to_string())))
        .collect()
}
fn validate_edits(r: &Value) -> Result<()> {
    ensure(
        string(r, "algorithm") == "byte-edits-v1",
        "unknown recipe primitive",
    )?;
    let mut end = 0u64;
    let mut previous = None;
    for edit in array(r, "edits") {
        let offset = edit["offset"]
            .as_u64()
            .ok_or_else(|| Error::Invalid("invalid edit offset".into()))?;
        let before = hex(string(edit, "before_hex"))?;
        hex(string(edit, "after_hex"))?;
        ensure(
            offset >= end && previous.is_none_or(|p| offset > p),
            "recipe offsets overlap or repeat",
        )?;
        end = offset
            .checked_add(before.len() as u64)
            .ok_or_else(|| Error::Invalid("edit overflow".into()))?;
        previous = Some(offset);
    }
    Ok(())
}
/// Apply increasing offsets against original input bytes and check both whole-file digests.
pub fn recipe(r: &Value, input: &[u8]) -> Result<Vec<u8>> {
    validate_edits(r)?;
    ensure(
        hash(input) == string(r, "input_sha256"),
        "recipe input hash mismatch",
    )?;
    let mut out = Vec::new();
    let mut cursor = 0;
    for edit in array(r, "edits") {
        let offset = usize::try_from(edit["offset"].as_u64().unwrap_or(u64::MAX))
            .map_err(|_| Error::Invalid("edit offset overflow".into()))?;
        let before = hex(string(edit, "before_hex"))?;
        let end = offset
            .checked_add(before.len())
            .ok_or_else(|| Error::Invalid("edit end overflow".into()))?;
        ensure(
            offset >= cursor && end <= input.len(),
            "recipe offset out of range",
        )?;
        ensure(input[offset..end] == before, "recipe preimage mismatch")?;
        out.extend_from_slice(&input[cursor..offset]);
        out.extend(hex(string(edit, "after_hex"))?);
        cursor = end;
    }
    out.extend_from_slice(&input[cursor..]);
    ensure(
        hash(&out) == string(r, "output_sha256"),
        "recipe output hash mismatch",
    )?;
    Ok(out)
}
