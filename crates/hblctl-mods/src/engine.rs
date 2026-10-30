// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Planning, durable evidence, verified mutation and explicit interrupted-transaction recovery.
use crate::{
    Error, Result,
    bundle::{Bundle, array, find, hash, recipe, string, visit},
    camera::{CameraFs, File, Verification},
    ensure,
};
use hblctl_core::safety::{Policy, Safety};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, File as HostFile},
    io::Write,
    path::Path,
};

/// Comparison of observed camera bytes and transaction evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    /// Every selected target matches its stock expectation.
    Stock,
    /// Every selected target matches the bundle and a committed journal owns it.
    Installed,
    /// A journal records an unfinished install or restore.
    Interrupted,
    /// Bytes, ownership or backups cannot establish a known state.
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    id: String,
    path: String,
    original: Option<File>,
    output: File,
    backup: String,
    owners: BTreeSet<String>,
}
/// Immutable plan, bound to one manifest and exact observed input bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    bundle: String,
    features: BTreeSet<String>,
    entries: Vec<Entry>,
    mount: String,
}
impl Plan {
    /// Resolved dependency closure.
    #[must_use]
    pub fn features(&self) -> &BTreeSet<String> {
        &self.features
    }
    /// Reviewable plan metadata; original and derived camera bytes stay private.
    #[must_use]
    pub fn summary(&self) -> Value {
        serde_json::json!({
            "bundle": self.bundle,
            "features": self.features,
            "targets": self.entries.iter().map(|e| serde_json::json!({
                "path": e.path,
                "before": e.original.as_ref().map(|f| hash(&f.bytes)),
                "after": hash(&e.output.bytes),
                "mode": format!("{:04o}", e.output.mode)
            })).collect::<Vec<_>>(),
            "reboot_required": true
        })
    }
    /// Exact target paths, in payload-before-hook order.
    pub fn targets(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.path.as_str())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    bundle: String,
    version: String,
    features: BTreeSet<String>,
    state: String,
    step: String,
    mount: String,
    entries: Vec<Entry>,
    root: String,
    reboot_required: bool,
}
/// Engine borrowing a validated bundle, camera and private recovery session directory.
pub struct Engine<'a, F: CameraFs> {
    bundle: &'a Bundle,
    camera: &'a mut F,
    session: &'a Path,
    policy: Policy,
}
impl<'a, F: CameraFs> Engine<'a, F> {
    /// Construct without connecting or changing either filesystem.
    pub fn new(bundle: &'a Bundle, camera: &'a mut F, session: &'a Path, policy: Policy) -> Self {
        Self {
            bundle,
            camera,
            session,
            policy,
        }
    }
    fn check(&self, name: &str, class: Safety) -> Result<()> {
        self.policy.check(name, class.max(self.camera.floor()))?;
        Ok(())
    }
    fn read(&mut self, path: &str) -> Result<Option<File>> {
        self.check("camera.read", Safety::Read)?;
        self.camera.check_path(path)?;
        let out = self.camera.read(path)?;
        tracing::info!(target:"hblctl::mods",path,hash=?out.as_ref().map(|f|hash(&f.bytes)),"read");
        Ok(out)
    }
    fn steps(&self, phase: &str) -> Vec<Value> {
        let mut steps: Vec<_> = array(self.bundle.manifest(), "steps")
            .iter()
            .filter(|s| string(s, "phase") == phase)
            .cloned()
            .collect();
        steps.sort_by_key(|s| s["order"].as_u64());
        steps
    }
    fn check_steps(&self, phase: &str) -> Result<()> {
        self.check("mods transaction", Safety::Persistent)?;
        for step in self.steps(phase) {
            let op = string(&step, "operation");
            let allowed = if phase == "install" {
                matches!(
                    op,
                    "validate-profile"
                        | "verify-stock"
                        | "prepare-recipes"
                        | "backup"
                        | "stage-and-verify"
                        | "journal-prepared"
                        | "remount-rw"
                        | "write-files"
                        | "write-init"
                        | "sync-remount-ro"
                        | "journal-installed"
                        | "reboot"
                        | "verify-running"
                )
            } else {
                matches!(
                    op,
                    "validate-profile"
                        | "verify-stock"
                        | "verify-ownership"
                        | "withdraw-runtime"
                        | "verify-idle"
                        | "journal-restoring"
                        | "stop-services"
                        | "remount-rw"
                        | "restore-backups"
                        | "remove-owned"
                        | "sync-remount-ro"
                        | "clear-journal"
                        | "reboot"
                )
            };
            ensure(allowed, format!("operation {op} is invalid in phase {phase}"))?;
            let effect: Safety = serde_json::from_value(step["safety"].clone())?;
            let floor: Safety = serde_json::from_value(step["transport_floor"].clone())?;
            self.check(string(&step, "id"), effect.max(floor))?;
        }
        Ok(())
    }
    /// Resolve dependencies, enforce policy and preconditions, and stage deterministic outputs in memory.
    pub fn plan(&mut self, selected: &[String]) -> Result<Plan> {
        let m = self.bundle.manifest();
        ensure(
            string(m, "status") == "ready",
            format!("bundle blocked: {}", string(m, "reason")),
        )?;
        self.check_steps("install")?;
        self.check_steps("restore")?;
        ensure(!selected.is_empty(), "empty feature selection")?;
        ensure(
            !self.session.join("journal.json").exists(),
            "session journal exists: recover or revert first",
        )?;
        let root = string(&m["backup"], "root");
        ensure(
            self.camera.list_dir(root)?.is_empty(),
            "unknown camera toolkit state: inspect existing receipts",
        )?;
        let mut features = BTreeSet::new();
        for id in selected {
            visit(m, id, &mut BTreeSet::new(), &mut features)?;
        }
        let mut ids = BTreeSet::new();
        for id in &features {
            let feature = find(m, "features", id)?;
            ensure(
                string(feature, "status") == "ready",
                format!("feature {id} blocked"),
            )?;
            for conflict in array(feature, "conflicts") {
                ensure(
                    !features.contains(conflict.as_str().unwrap_or("")),
                    format!("feature {id} conflicts with {conflict}"),
                )?;
            }
            for file in array(feature, "files") {
                ids.insert(file.as_str().unwrap_or("").to_owned());
            }
        }
        let pre = &m["preconditions"];
        let (model, firmware) = self.camera.identity()?;
        ensure(
            array(pre, "models").iter().any(|v| v == &model),
            format!("model precondition failed: {model}"),
        )?;
        ensure(
            array(pre, "firmware").iter().any(|v| v == &firmware),
            format!("firmware precondition failed: {firmware}"),
        )?;
        let mount = self.camera.system_mount()?;
        ensure(
            mount == string(pre, "system_mount"),
            "system mount precondition failed",
        )?;
        for stock in array(pre, "stock_hashes") {
            let path = string(stock, "path");
            ensure(
                self.camera.hash(path)?.as_deref() == Some(string(stock, "sha256")),
                format!("stock hash precondition failed: {path}"),
            )?;
        }
        let mut entries = self.stage(&ids, &features)?;
        entries.sort_by_key(|e| (e.path.contains("/init/"), e.path.clone()));
        for (i, e) in entries.iter_mut().enumerate() {
            e.backup = format!("backup-{i}");
        }
        let steps = self.steps("install");
        for e in &entries {
            let op = if e.path.contains("/init/") {
                "write-init"
            } else {
                "write-files"
            };
            ensure(
                steps.iter().any(|s| string(s, "operation") == op),
                format!("plan has no {op} step for {}", e.path),
            )?;
        }
        if entries.iter().any(|e| e.path.contains("/init/")) {
            let payload = steps.iter().position(|s| string(s, "operation") == "write-files");
            let hook = steps.iter().position(|s| string(s, "operation") == "write-init");
            ensure(
                payload.zip(hook).is_none_or(|(a, b)| a < b),
                "startup hooks precede payloads",
            )?;
        }
        tracing::info!(target:"hblctl::mods",?features,"plan validated");
        Ok(Plan {
            bundle: self.bundle.digest().into(),
            features,
            entries,
            mount,
        })
    }
    fn stage(&mut self, ids: &BTreeSet<String>, features: &BTreeSet<String>) -> Result<Vec<Entry>> {
        let m = self.bundle.manifest();
        let root = string(&m["backup"], "root");
        let mut entries = Vec::new();
        let mut targets = BTreeSet::new();
        for id in ids {
            let file = find(m, "files", id)?;
            ensure(string(file, "status") == "ready", format!("file {id} blocked"))?;
            let path = string(file, "target");
            ensure(
                targets.insert(path.to_owned()),
                format!("duplicate selected target: {path}"),
            )?;
            ensure(
                path != root && !path.starts_with(&format!("{root}/")),
                "target overlaps toolkit recovery root",
            )?;
            let original = self.read(path)?;
            ensure(
                matches_before(original.as_ref(), string(file, "before")),
                format!("stock target precondition failed: {path}"),
            )?;
            let bytes = if let Some(recipe_id) = file["recipe"].as_str() {
                let r = find(m, "recipes", recipe_id)?;
                let input = self
                    .read(string(r, "input"))?
                    .ok_or_else(|| Error::Invalid("missing recipe input".into()))?;
                recipe(r, &input.bytes)?
            } else {
                self.bundle.artifact(string(file, "artifact"))?
            };
            ensure(
                hash(&bytes) == string(file, "output_sha256"),
                format!("staged output hash mismatch: {path}"),
            )?;
            let mode =
                u32::from_str_radix(string(file, "mode"), 8).map_err(|e| Error::Invalid(e.to_string()))?;
            let owners = features
                .iter()
                .filter(|id| {
                    find(m, "features", id).is_ok_and(|f| {
                        array(f, "files")
                            .iter()
                            .any(|v| v.as_str() == Some(&id_for_file(file)))
                    })
                })
                .cloned()
                .collect();
            entries.push(Entry {
                id: id.clone(),
                path: path.into(),
                original,
                output: File { bytes, mode },
                backup: String::new(),
                owners,
            });
        }
        Ok(entries)
    }
    /// Apply a freshly revalidated plan; ordinary failures attempt verified rollback.
    /// Transport loss retains evidence for explicit recovery without replaying the mutation.
    pub fn apply(&mut self, plan: &Plan) -> Result<()> {
        ensure(
            plan.bundle == self.bundle.digest(),
            "plan belongs to another bundle",
        )?;
        let fresh = self.plan(&plan.features.iter().cloned().collect::<Vec<_>>())?;
        ensure(&fresh == plan, "camera changed since planning")?;
        self.camera.transaction_ready()?;
        private_dir(self.session)?;
        let root = string(&self.bundle.manifest()["backup"], "root").to_owned();
        let mut j = Journal {
            bundle: plan.bundle.clone(),
            version: string(self.bundle.manifest(), "version").into(),
            features: plan.features.clone(),
            state: "PREPARED".into(),
            step: "backups".into(),
            mount: plan.mount.clone(),
            entries: plan.entries.clone(),
            root,
            reboot_required: true,
        };
        // Persist every original and staged output before any on-camera mutation.
        for entry in &j.entries {
            if let Some(original) = &entry.original {
                durable(&self.session.join(&entry.backup), &original.bytes)?;
                tracing::info!(target:"hblctl::mods",path=%self.session.join(&entry.backup).display(),hash=%hash(&original.bytes),"host backup");
            }
        }
        self.save_host(&j)?;
        let result = self.install(&mut j);
        if let Err(error) = result {
            tracing::error!(target:"hblctl::mods",%error,"transaction failed");
            if !matches!(error, Error::PowerLoss)
                && let Err(rollback) = self.restore(&mut j, None)
            {
                return Err(Error::Invalid(format!(
                    "{error}; rollback failed: {rollback}; recovery evidence retained"
                )));
            }
            return Err(error);
        }
        Ok(())
    }
    fn install(&mut self, j: &mut Journal) -> Result<()> {
        self.save(j)?;
        for entry in &j.entries {
            if let Some(original) = &entry.original {
                let path = format!("{}/{}", j.root, entry.backup);
                self.camera.write(&path, original)?;
                ensure(
                    self.read(&path)?.as_ref() == Some(original),
                    format!("camera backup mismatch: {path}"),
                )?;
                tracing::info!(target:"hblctl::mods",%path,hash=%hash(&original.bytes),"camera backup");
            }
        }
        for step in self.steps("install") {
            j.step = string(&step, "id").into();
            self.save(j)?;
            let operation = string(&step, "operation");
            tracing::info!(target:"hblctl::mods",step=%j.step,operation,"step");
            match operation {
                "write-files" | "write-init" => {
                    self.camera.remount(true)?;
                    for entry in &j.entries {
                        if entry.path.contains("/init/") != (operation == "write-init") {
                            continue;
                        }
                        ensure(
                            self.read(&entry.path)? == entry.original,
                            format!("target changed before write: {}", entry.path),
                        )?;
                        self.camera.write(&entry.path, &entry.output)?;
                        ensure(
                            self.read(&entry.path)?.as_ref() == Some(&entry.output),
                            format!("post-write verification failed: {}", entry.path),
                        )?;
                    }
                }
                "remount-rw" => self.camera.remount(true)?,
                "sync-remount-ro" => self.readonly(j)?,
                "verify-running" => self.camera.verify(Verification::Installed)?,
                "reboot" => {
                    tracing::info!(target:"hblctl::mods","reboot deferred until commit and owner request");
                }
                // These declarative phases are fulfilled by mandatory engine preflight/staging/evidence.
                "validate-profile" | "verify-stock" | "prepare-recipes" | "backup" | "stage-and-verify"
                | "journal-prepared" | "journal-installed" => {}
                other => return Err(Error::Invalid(format!("unsupported install operation {other}"))),
            }
        }
        self.readonly(j)?;
        self.camera.verify(Verification::Installed)?;
        j.step = "committed".into();
        self.commit(j)
    }
    fn commit(&mut self, j: &mut Journal) -> Result<()> {
        // A lost commit write must leave the host in a recoverable, uncommitted state.
        self.save_host(j)?;
        let mut committed = j.clone();
        committed.state = "INSTALLED".into();
        tracing::info!(target:"hblctl::mods", path=%format!("{}/journal.json",j.root), "camera journal commit");
        self.camera.write(
            &format!("{}/journal.json", j.root),
            &File {
                bytes: serde_json::to_vec(&committed)?,
                mode: 0o600,
            },
        )?;
        self.camera.sync()?;
        self.save_host(&committed)?;
        *j = committed;
        Ok(())
    }
    fn readonly(&mut self, j: &Journal) -> Result<()> {
        self.camera.sync()?;
        self.camera.remount(false)?;
        ensure(
            self.camera.system_mount()? == j.mount,
            "read-only mount did not return to original state",
        )
    }
    fn save_host(&self, j: &Journal) -> Result<()> {
        tracing::info!(target:"hblctl::mods",state=%j.state,step=%j.step,path=%self.session.join("journal.json").display(),"journal write");
        durable(&self.session.join("journal.json"), &serde_json::to_vec_pretty(j)?)
    }
    fn save(&mut self, j: &Journal) -> Result<()> {
        self.save_host(j)?;
        let path = format!("{}/journal.json", j.root);
        tracing::info!(target:"hblctl::mods", %path, state=%j.state, step=%j.step, "camera journal write");
        self.camera.write(
            &path,
            &File {
                bytes: serde_json::to_vec(j)?,
                mode: 0o600,
            },
        )?;
        self.camera.sync()
    }
    fn journal(&self) -> Result<Option<Journal>> {
        let path = self.session.join("journal.json");
        if !path.exists() {
            return Ok(None);
        }
        let j: Journal = serde_json::from_slice(&fs::read(path)?)?;
        ensure(
            j.bundle == self.bundle.digest(),
            "journal belongs to another bundle",
        )?;
        ensure(
            j.root == string(&self.bundle.manifest()["backup"], "root"),
            "journal root mismatch",
        )?;
        Ok(Some(j))
    }
    /// Restore one selected feature or all, in reverse file order. Also recovers interrupted installs.
    pub fn revert(&mut self, feature: Option<&str>) -> Result<()> {
        ensure(
            string(self.bundle.manifest(), "status") == "ready",
            "blocked bundles cannot execute restore",
        )?;
        self.check_steps("restore")?;
        let mut j = self
            .journal()?
            .ok_or_else(|| Error::Invalid("no recovery journal in this session directory".into()))?;
        self.restore(&mut j, feature)
    }
    fn validate_journal(&self, j: &Journal) -> Result<()> {
        ensure(
            j.version == string(self.bundle.manifest(), "version"),
            "journal version mismatch",
        )?;
        ensure(
            matches!(j.state.as_str(), "PREPARED" | "INSTALLED" | "RESTORING"),
            "unknown journal state",
        )?;
        ensure(
            j.mount == string(&self.bundle.manifest()["preconditions"], "system_mount"),
            "journal mount differs from profile",
        )?;
        let mut seen = BTreeSet::new();
        for entry in &j.entries {
            let record = find(self.bundle.manifest(), "files", &entry.id)?;
            ensure(seen.insert(&entry.path), "duplicate journal target")?;
            ensure(
                entry.path == string(record, "target")
                    && hash(&entry.output.bytes) == string(record, "output_sha256")
                    && matches_before(entry.original.as_ref(), string(record, "before")),
                "journal ownership evidence differs from bundle",
            )?;
            ensure(
                entry.backup.starts_with("backup-")
                    && entry.backup.len() > 7
                    && entry.backup[7..].bytes().all(|b| b.is_ascii_digit()),
                "invalid backup name",
            )?;
        }
        Ok(())
    }
    fn validate_recovery(&mut self, j: &Journal) -> Result<()> {
        // Validate all recovery evidence and ownership before withdrawing runtime effects.
        for entry in &j.entries {
            if let Some(original) = &entry.original {
                ensure(
                    fs::read(self.session.join(&entry.backup))? == original.bytes,
                    format!("host backup corrupted: {}", entry.backup),
                )?;
            }
            let current = self.read(&entry.path)?;
            let exact = current == entry.original || current.as_ref() == Some(&entry.output);
            let prefix = j.state != "INSTALLED"
                && current.as_ref().is_some_and(|c| {
                    (c.mode == entry.output.mode || entry.original.as_ref().is_some_and(|o| o.mode == c.mode))
                        && (entry.output.bytes.starts_with(&c.bytes)
                            || entry
                                .original
                                .as_ref()
                                .is_some_and(|o| o.bytes.starts_with(&c.bytes)))
                });
            ensure(exact || prefix, format!("unowned current bytes: {}", entry.path))?;
        }
        if j.state == "INSTALLED" {
            let receipt = self.read(&format!("{}/journal.json", j.root))?;
            ensure(
                receipt.is_some_and(|f| f.bytes == serde_json::to_vec(j).unwrap_or_default()),
                "camera journal differs from host evidence",
            )?;
            for entry in &j.entries {
                if let Some(original) = &entry.original {
                    ensure(
                        self.read(&format!("{}/{}", j.root, entry.backup))?.as_ref() == Some(original),
                        "camera backup differs from host evidence",
                    )?;
                }
            }
        }
        Ok(())
    }
    fn restore_entries(&mut self, j: &Journal, remaining: &BTreeSet<String>) -> Result<()> {
        self.camera.remount(true)?;
        for entry in j.entries.iter().rev().filter(|e| e.owners.is_disjoint(remaining)) {
            self.save(j)?;
            match &entry.original {
                Some(original) => self.camera.write(&entry.path, original)?,
                None => self.camera.remove(&entry.path)?,
            }
            ensure(
                self.read(&entry.path)? == entry.original,
                format!("restore verification failed: {}", entry.path),
            )?;
            tracing::info!(target:"hblctl::mods",path=%entry.path,"restored original");
        }
        Ok(())
    }
    fn restore(&mut self, j: &mut Journal, feature: Option<&str>) -> Result<()> {
        self.check_steps("restore")?;
        self.camera.transaction_ready()?;
        self.validate_journal(j)?;
        let mut remaining = j.features.clone();
        if let Some(id) = feature {
            ensure(remaining.remove(id), format!("feature {id} is not installed"))?;
            for other in &remaining {
                let mut dependencies = BTreeSet::new();
                visit(
                    self.bundle.manifest(),
                    other,
                    &mut BTreeSet::new(),
                    &mut dependencies,
                )?;
                ensure(!dependencies.contains(id), format!("{other} still requires {id}"))?;
            }
        } else {
            remaining.clear();
        }
        self.validate_recovery(j)?;
        self.camera.verify(Verification::PrepareRestore)?;
        j.state = "RESTORING".into();
        j.step = "restore-backups".into();
        self.save(j)?;
        let mut restored = false;
        for step in self.steps("restore") {
            j.step = string(&step, "id").into();
            self.save(j)?;
            let operation = string(&step, "operation");
            tracing::info!(target:"hblctl::mods", step=%j.step, operation, "restore step");
            match operation {
                "restore-backups" => {
                    self.restore_entries(j, &remaining)?;
                    restored = true;
                }
                "remount-rw" => self.camera.remount(true)?,
                "sync-remount-ro" => self.readonly(j)?,
                // Mandatory validation and runtime withdrawal surround all declared restore steps.
                _ => {}
            }
        }
        if !restored {
            self.restore_entries(j, &remaining)?;
        }
        self.readonly(j)?;
        self.camera.verify(Verification::Restored)?;
        // The journal remains until all restores and mount/runtime checks succeeded.
        let removed: Vec<_> = j
            .entries
            .iter()
            .filter(|e| e.owners.is_disjoint(&remaining))
            .map(|e| e.backup.clone())
            .collect();
        for backup in &removed {
            self.camera.remove(&format!("{}/{backup}", j.root))?;
        }
        j.features = remaining;
        j.entries.retain(|e| !e.owners.is_disjoint(&j.features));
        if j.features.is_empty() {
            self.camera.remove(&format!("{}/journal.json", j.root))?;
            fs::remove_file(self.session.join("journal.json"))?;
            HostFile::open(self.session)?.sync_all()?;
        } else {
            j.step = "committed-partial-restore".into();
            self.commit(j)?;
        }
        tracing::info!(target:"hblctl::mods",reboot_required=true,"restore complete");
        Ok(())
    }
    /// Classify a feature from current hashes and this session's ownership journal.
    pub fn status(&mut self, feature: &str) -> Result<Status> {
        self.check("mods.status", Safety::Read)?;
        let f = find(self.bundle.manifest(), "features", feature)?;
        let j = self.journal()?;
        if j.as_ref()
            .is_some_and(|j| j.features.contains(feature) && j.state != "INSTALLED")
        {
            return Ok(Status::Interrupted);
        }
        if let Some(receipt) = &j
            && receipt.features.contains(feature)
            && (self.validate_journal(receipt).is_err() || self.validate_recovery(receipt).is_err())
        {
            return Ok(Status::Unknown);
        }
        let mut stock = true;
        let mut installed = true;
        for id in array(f, "files") {
            let file = find(self.bundle.manifest(), "files", id.as_str().unwrap_or(""))?;
            let current = self.read(string(file, "target"))?;
            stock &= matches_before(current.as_ref(), string(file, "before"));
            installed &= current.as_ref().is_some_and(|c| {
                hash(&c.bytes) == string(file, "output_sha256")
                    && format!("{:04o}", c.mode) == string(file, "mode")
            });
        }
        Ok(if installed && j.is_some_and(|j| j.features.contains(feature)) {
            Status::Installed
        } else if stock {
            Status::Stock
        } else {
            Status::Unknown
        })
    }
}
fn id_for_file(file: &Value) -> String {
    string(file, "id").into()
}
fn matches_before(file: Option<&File>, before: &str) -> bool {
    match file {
        None => before == "ABSENT",
        Some(f) => hash(&f.bytes) == before,
    }
}
fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("missing storage parent".into()))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| Error::Io(e.error))?;
    HostFile::open(parent)?.sync_all()?;
    Ok(())
}
