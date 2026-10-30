// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Camera file operations. Diagnostic file-transfer commands are unverified and unsupported.
use crate::{
    Error, Result,
    bundle::{hash, safe_camera_path},
    ensure,
};
use hblctl_core::safety::Safety;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    process::Command,
};

/// File bytes and exact Unix mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    /// Complete file contents.
    pub bytes: Vec<u8>,
    /// Permission bits.
    pub mode: u32,
}
/// Fixed verification operations, never a bundle-supplied command string.
#[derive(Debug, Clone, Copy)]
pub enum Verification {
    /// Verify installed runtime markers and service readiness.
    Installed,
    /// Withdraw runtime effects, verify idle, and stop affected services.
    PrepareRestore,
    /// Verify restored runtime and USB state.
    Restored,
}
/// Everything a transaction may do on a camera.
pub trait CameraFs {
    /// Check that runtime verification and withdrawal are supported before any mutation.
    fn transaction_ready(&self) -> Result<()> {
        Ok(())
    }
    /// Minimum class imposed by this transport.
    fn floor(&self) -> Safety;
    /// Exact camera model and firmware.
    fn identity(&mut self) -> Result<(String, String)>;
    /// Complete original system mount description; it must be read-only.
    fn system_mount(&mut self) -> Result<String>;
    /// Reject symlinks in every existing ancestor and the target itself.
    fn check_path(&mut self, path: &str) -> Result<()>;
    /// Read a regular file with its mode, or report absence.
    fn read(&mut self, path: &str) -> Result<Option<File>>;
    /// Hash a file, distinguishing absence.
    fn hash(&mut self, path: &str) -> Result<Option<String>> {
        let h = self.read(path)?.map(|f| hash(&f.bytes));
        tracing::info!(target:"hblctl::mods",path,hash=?h,"camera hash");
        Ok(h)
    }
    /// Durably write a file, creating private toolkit parents if required.
    fn write(&mut self, path: &str, file: &File) -> Result<()>;
    /// Remove an owned file if present.
    fn remove(&mut self, path: &str) -> Result<()>;
    /// List immediate entries in a directory.
    fn list_dir(&mut self, path: &str) -> Result<Vec<String>>;
    /// Change system writeability.
    fn remount(&mut self, writable: bool) -> Result<()>;
    /// Sync camera storage.
    fn sync(&mut self) -> Result<()>;
    /// Perform a fixed runtime verification.
    fn verify(&mut self, command: Verification) -> Result<()>;
    /// Explicit reboot request; the engine reports the requirement and never calls this automatically.
    fn reboot(&mut self) -> Result<()>;
}
/// In-memory camera, including repeatable write failure and mid-write power loss injection.
#[derive(Debug, Clone)]
pub struct MemoryFs {
    /// All regular files, including toolkit evidence.
    pub files: BTreeMap<String, File>,
    /// Symlink paths to reject (including ancestors).
    pub symlinks: BTreeSet<String>,
    /// Camera model.
    pub model: String,
    /// Camera firmware.
    pub firmware: String,
    /// Read-only system mount identity.
    pub mount: String,
    /// Fail exactly this write, counting from one.
    pub fail_on_write: Option<usize>,
    /// Cut this write in half and reject all later operations until reconnect.
    pub power_loss_on_write: Option<usize>,
    /// Fail runtime verification.
    pub fail_verification: bool,
    /// Current mount mode.
    pub writable: bool,
    writes: usize,
    offline: bool,
}
impl Default for MemoryFs {
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            symlinks: BTreeSet::new(),
            model: "test".into(),
            firmware: "1".into(),
            mount: "test-ro".into(),
            fail_on_write: None,
            power_loss_on_write: None,
            fail_verification: false,
            writable: false,
            writes: 0,
            offline: false,
        }
    }
}
impl MemoryFs {
    /// Restore power without replaying the interrupted operation.
    pub fn reconnect(&mut self) {
        self.offline = false;
        self.fail_on_write = None;
        self.power_loss_on_write = None;
    }
    fn online(&self) -> Result<()> {
        if self.offline {
            Err(Error::PowerLoss)
        } else {
            Ok(())
        }
    }
}
impl CameraFs for MemoryFs {
    fn floor(&self) -> Safety {
        Safety::Read
    }
    fn identity(&mut self) -> Result<(String, String)> {
        self.online()?;
        Ok((self.model.clone(), self.firmware.clone()))
    }
    fn system_mount(&mut self) -> Result<String> {
        self.online()?;
        ensure(!self.writable, "system is not read-only")?;
        Ok(self.mount.clone())
    }
    fn check_path(&mut self, path: &str) -> Result<()> {
        self.online()?;
        safe_camera_path(path)?;
        ensure(
            !self
                .symlinks
                .iter()
                .any(|p| path == p || path.starts_with(&format!("{p}/"))),
            format!("symlink in path {path}"),
        )
    }
    fn read(&mut self, path: &str) -> Result<Option<File>> {
        self.check_path(path)?;
        Ok(self.files.get(path).cloned())
    }
    fn write(&mut self, path: &str, file: &File) -> Result<()> {
        self.check_path(path)?;
        self.writes += 1;
        if self.fail_on_write == Some(self.writes) {
            return Err(Error::Invalid(format!("injected write failure {}", self.writes)));
        }
        if self.power_loss_on_write == Some(self.writes) {
            let mut partial = file.clone();
            partial.bytes.truncate(file.bytes.len() / 2);
            self.files.insert(path.into(), partial);
            self.offline = true;
            return Err(Error::PowerLoss);
        }
        self.files.insert(path.into(), file.clone());
        Ok(())
    }
    fn remove(&mut self, path: &str) -> Result<()> {
        self.check_path(path)?;
        self.files.remove(path);
        Ok(())
    }
    fn list_dir(&mut self, path: &str) -> Result<Vec<String>> {
        self.check_path(path)?;
        let prefix = format!("{path}/");
        Ok(self
            .files
            .keys()
            .filter_map(|p| p.strip_prefix(&prefix))
            .map(|p| p.split('/').next().unwrap_or("").to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }
    fn remount(&mut self, writable: bool) -> Result<()> {
        self.online()?;
        self.writable = writable;
        Ok(())
    }
    fn sync(&mut self) -> Result<()> {
        self.online()
    }
    fn verify(&mut self, _command: Verification) -> Result<()> {
        self.online()?;
        ensure(!self.fail_verification, "injected verification failure")
    }
    fn reboot(&mut self) -> Result<()> {
        self.online()?;
        self.writable = false;
        Ok(())
    }
}
/// Explicit ADB backend. Paths are validated before use; commands and arguments are fixed.
/// Runtime acceptance/withdrawal commands are not yet reviewed, so verification fails closed.
#[derive(Debug, Default)]
pub struct AdbFs;
impl AdbFs {
    fn command(args: &[&str], mutation: bool) -> Result<Vec<u8>> {
        tracing::info!(target:"hblctl::mods",?args,"adb");
        let out = Command::new("adb").args(args).output()?;
        if !out.status.success() {
            if mutation {
                return Err(Error::PowerLoss);
            }
            return Err(Error::Invalid(format!(
                "adb {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(out.stdout)
    }
    fn text(args: &[&str]) -> Result<String> {
        String::from_utf8(Self::command(args, false)?)
            .map(|s| s.trim().into())
            .map_err(|e| Error::Invalid(e.to_string()))
    }
}
impl CameraFs for AdbFs {
    fn transaction_ready(&self) -> Result<()> {
        Err(Error::Invalid(
            "ADB runtime acceptance and withdrawal checks are not reviewed; execution disabled".into(),
        ))
    }

    fn floor(&self) -> Safety {
        Safety::Destructive
    }
    fn identity(&mut self) -> Result<(String, String)> {
        Ok((
            Self::text(&["shell", "getprop", "ro.product.model"])?,
            Self::text(&["shell", "getprop", "ro.build.version.incremental"])?,
        ))
    }
    fn system_mount(&mut self) -> Result<String> {
        let mounts = Self::text(&["exec-out", "cat", "/proc/mounts"])?;
        let line = mounts
            .lines()
            .find(|s| s.split_whitespace().nth(1) == Some("/system"))
            .ok_or_else(|| Error::Invalid("no /system mount".into()))?;
        ensure(
            line.split_whitespace()
                .nth(3)
                .is_some_and(|s| s.split(',').any(|v| v == "ro")),
            "system is not read-only",
        )?;
        Ok(line.into())
    }
    fn check_path(&mut self, path: &str) -> Result<()> {
        safe_camera_path(path)?;
        let mut parent = String::new();
        for part in path[1..].split('/') {
            parent.push('/');
            parent.push_str(part);
            let out = Command::new("adb")
                .args(["shell", "test", "-L", &parent])
                .output()?;
            ensure(
                out.status.code() == Some(1),
                format!("symlink or failed symlink probe: {parent}"),
            )?;
        }
        Ok(())
    }
    fn read(&mut self, path: &str) -> Result<Option<File>> {
        self.check_path(path)?;
        let probe = Command::new("adb").args(["shell", "test", "-e", path]).output()?;
        if probe.status.code() == Some(1) {
            return Ok(None);
        }
        ensure(probe.status.success(), format!("failed existence probe {path}"))?;
        Self::command(&["shell", "test", "-f", path], false)?;
        let mode = u32::from_str_radix(&Self::text(&["shell", "stat", "-c", "%a", path])?, 8)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(Some(File {
            bytes: Self::command(&["exec-out", "cat", path], false)?,
            mode,
        }))
    }
    fn write(&mut self, path: &str, file: &File) -> Result<()> {
        self.check_path(path)?;
        let parent = path.rsplit_once('/').map_or("/", |(p, _)| p);
        Self::command(&["shell", "mkdir", "-p", "-m", "0700", parent], true)?;
        let mut temp = tempfile::NamedTempFile::new()?;
        temp.write_all(&file.bytes)?;
        temp.as_file().sync_all()?;
        let local = temp
            .path()
            .to_str()
            .ok_or_else(|| Error::Invalid("non UTF-8 staging path".into()))?;
        Self::command(&["push", local, path], true)?;
        let mode = format!("{:o}", file.mode);
        ensure(file.mode <= 0o777, "invalid file mode")?;
        Self::command(&["shell", "chmod", &mode, path], true)?;
        self.sync()
    }
    fn remove(&mut self, path: &str) -> Result<()> {
        self.check_path(path)?;
        Self::command(&["shell", "rm", "-f", path], true)?;
        self.sync()
    }
    fn list_dir(&mut self, path: &str) -> Result<Vec<String>> {
        self.check_path(path)?;
        Ok(Self::text(&["shell", "ls", "-1A", path])?
            .lines()
            .map(str::to_owned)
            .collect())
    }
    fn remount(&mut self, writable: bool) -> Result<()> {
        let option = if writable { "remount,rw" } else { "remount,ro" };
        Self::command(&["shell", "mount", "-o", option, "/system"], true)?;
        Ok(())
    }
    fn sync(&mut self) -> Result<()> {
        Self::command(&["shell", "sync"], true)?;
        Ok(())
    }
    fn verify(&mut self, command: Verification) -> Result<()> {
        Err(Error::Invalid(format!(
            "ADB runtime check {command:?} has no reviewed fixed implementation; refusing execution"
        )))
    }
    fn reboot(&mut self) -> Result<()> {
        Self::command(&["reboot"], true)?;
        Ok(())
    }
}
