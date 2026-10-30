// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Passive bundle loading and policy-gated, recoverable camera file transactions.
pub mod bundle;
pub mod camera;
pub mod engine;

/// Engine and bundle failures retain context suitable for the session log.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid or unsupported content or camera state.
    #[error("{0}")]
    Invalid(String),
    /// Local storage error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Invalid JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// User policy refused an operation before execution.
    #[error(transparent)]
    Refused(#[from] hblctl_core::safety::Refused),
    /// Connection was lost; inspect state before recovery, never replay writes.
    #[error("camera power/transport loss: recovery required")]
    PowerLoss,
}
/// Result shared by the bundle loader, camera backend and engine.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn ensure(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(message.into()))
    }
}
