// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! The hblctl client core, shared by every frontend.
//!
//! * [`client`] sends one command and waits for its result, collecting notifications and progress.
//! * [`safety`] classifies commands and enforces the caller's chosen ceiling.
//! * [`registry`] describes every command as data; [`codec`] turns JSON arguments into records and back.

pub mod client;
pub mod codec;
pub mod dispatch;
pub mod registry;
pub mod safety;

/// Lower-case hex.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

pub use client::{Client, Outcome, Status};
pub use safety::{Policy, Safety};
