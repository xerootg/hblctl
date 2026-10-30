// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Safety classes and the policy that gates them.
//!
//! Every command and sub-function is assigned the highest class of anything it can do. Callers choose a
//! ceiling; anything above it is refused before a byte is sent.

use serde::{Deserialize, Serialize};

/// What a command can change, from least to most dangerous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    /// Reads state only.
    Read,
    /// Changes runtime state that a reboot undoes.
    Volatile,
    /// Writes files or settings that survive a reboot.
    Persistent,
    /// Can damage the camera: calibration, factory configuration, camera type, firmware, erase, root shell.
    Destructive,
}

impl std::fmt::Display for Safety {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Safety::Read => "read",
            Safety::Volatile => "volatile",
            Safety::Persistent => "persistent",
            Safety::Destructive => "destructive",
        })
    }
}

/// Highest class the caller allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Ceiling, inclusive.
    pub max: Safety,
}

impl Default for Policy {
    /// Read-only unless the caller opts in.
    fn default() -> Self {
        Self { max: Safety::Read }
    }
}

/// A command was refused by the policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what} is classed {needed}; current limit is {limit} (raise it explicitly to allow)")]
pub struct Refused {
    /// The command and function refused.
    pub what: String,
    /// Its class.
    pub needed: Safety,
    /// The policy ceiling.
    pub limit: Safety,
}

impl Policy {
    /// Allow anything up to and including `max`.
    #[must_use]
    pub fn up_to(max: Safety) -> Self {
        Self { max }
    }

    /// Check one operation.
    pub fn check(&self, what: &str, class: Safety) -> Result<(), Refused> {
        if class <= self.max {
            Ok(())
        } else {
            Err(Refused {
                what: what.to_owned(),
                needed: class,
                limit: self.max,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_read_only() {
        let p = Policy::default();
        assert!(p.check("x", Safety::Read).is_ok());
        let e = p.check("ProdConfig.set", Safety::Persistent).unwrap_err();
        assert_eq!(e.needed, Safety::Persistent);
        assert!(e.to_string().contains("ProdConfig.set"));
    }

    #[test]
    fn ordering_and_ceiling() {
        assert!(
            Safety::Read < Safety::Volatile
                && Safety::Volatile < Safety::Persistent
                && Safety::Persistent < Safety::Destructive
        );
        let p = Policy::up_to(Safety::Persistent);
        assert!(p.check("a", Safety::Persistent).is_ok());
        assert!(p.check("b", Safety::Destructive).is_err());
    }

    #[test]
    fn serde_names() {
        assert_eq!(
            serde_json::to_string(&Safety::Destructive).unwrap(),
            "\"destructive\""
        );
        assert_eq!(
            serde_json::from_str::<Safety>("\"volatile\"").unwrap(),
            Safety::Volatile
        );
    }
}
