// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! The hblctl client core, shared by every frontend.
//!
//! * [`client`] sends one command and waits for its result, collecting notifications and progress.
//! * [`safety`] classifies commands and enforces the caller's chosen ceiling.

pub mod client;
pub mod safety;

pub use client::{Client, Outcome, Status};
pub use safety::{Policy, Safety};
