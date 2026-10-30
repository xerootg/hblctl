// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Per-run session directory: everything needed to understand, replay or turn a run into a test.
//!
//! ```text
//! <state dir>/hblctl/sessions/<UTC time>-<pid>/
//!   meta.json       hblctl version, command line, start time, OS
//!   events.jsonl    every log event as JSON (all hblctl targets at TRACE: wire hex, frames, results)
//!   capture.hblcap  every socket byte with direction and timestamp (see hblctl_transport::capture)
//! ```
//! Files are written as the run progresses and flushed per record, so a crash still leaves a usable session.

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hblctl_transport::capture::CaptureWriter;
use serde_json::json;
use time::OffsetDateTime;
use time::macros::format_description;

/// Default location: the platform state directory (`$XDG_STATE_HOME` on Linux), falling back to the local
/// data directory.
#[must_use]
pub(crate) fn default_root() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("hblctl")
        .join("sessions")
}

/// An open session directory.
#[derive(Debug)]
pub(crate) struct Session {
    /// The session's directory.
    pub(crate) dir: PathBuf,
}

impl Session {
    /// Create a new session directory under `root` and write `meta.json`.
    pub(crate) fn create(root: &Path, argv: &[String]) -> Result<Self> {
        let now = OffsetDateTime::now_utc();
        let stamp = now
            .format(format_description!("[year][month][day]T[hour][minute][second]Z"))
            .context("formatting session timestamp")?;
        let dir = root.join(format!("{stamp}-{}", std::process::id()));
        fs::create_dir_all(&dir).with_context(|| format!("creating session directory {}", dir.display()))?;
        let meta = json!({
            "hblctl_version": env!("CARGO_PKG_VERSION"),
            "started_utc": now.format(&time::format_description::well_known::Rfc3339).context("formatting start time")?,
            "argv": argv,
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        });
        fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta)?).context("writing meta.json")?;
        Ok(Self { dir })
    }

    /// Path of the JSON event log.
    #[must_use]
    pub(crate) fn events_path(&self) -> PathBuf {
        self.dir.join("events.jsonl")
    }

    /// Path of the binary socket capture.
    #[must_use]
    pub(crate) fn capture_path(&self) -> PathBuf {
        self.dir.join("capture.hblcap")
    }

    /// Open the capture for writing.
    pub(crate) fn capture(&self) -> Result<CaptureWriter<BufWriter<File>>> {
        let f = File::create(self.capture_path()).context("creating capture file")?;
        Ok(CaptureWriter::new(BufWriter::new(f))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_directory_with_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Session::create(tmp.path(), &["hblctl".into(), "info".into()]).unwrap();
        assert!(s.dir.starts_with(tmp.path()));
        let meta: serde_json::Value =
            serde_json::from_slice(&fs::read(s.dir.join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["argv"], json!(["hblctl", "info"]));
        assert_eq!(meta["hblctl_version"], json!(env!("CARGO_PKG_VERSION")));
        let mut c = s.capture().unwrap();
        c.write(hblctl_transport::capture::Direction::Sent, &[1, 2])
            .unwrap();
        drop(c);
        let recs = hblctl_transport::capture::CaptureReader::new(File::open(s.capture_path()).unwrap())
            .unwrap()
            .records()
            .unwrap();
        assert_eq!(recs.len(), 1);
    }

    #[test]
    fn default_root_ends_in_hblctl_sessions() {
        assert!(default_root().ends_with(Path::new("hblctl").join("sessions")));
    }
}
