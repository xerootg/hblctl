// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Byte transports for hblctl.
//!
//! A [`Transport`] moves raw bytes; it knows nothing about frames. Framing lives in [`framer`], so every
//! transport (TCP, the scripted [`mock::MockTransport`], [`replay::ReplayTransport`]) is tested once and the
//! protocol code runs unchanged over each.
//!
//! [`recording::Recording`] wraps any transport and writes every byte, in both directions with timestamps,
//! to a capture file ([`capture`]). A capture can be replayed later to reproduce a session exactly, which is
//! how field failures become regression tests.

pub mod capture;
pub mod framer;
pub mod mock;
pub mod recording;
pub mod replay;
pub mod tcp;

use std::future::Future;

/// Transport-level failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Socket or file I/O failed.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// No data arrived in time.
    #[error("timed out after {0:?}")]
    Timeout(std::time::Duration),
    /// A scripted or replayed transport saw bytes it did not expect.
    #[error("unexpected bytes sent at step {step}: {detail}")]
    Unexpected {
        /// Index of the script or capture record.
        step: usize,
        /// What differed.
        detail: String,
    },
    /// The script or capture has no more data.
    #[error("script exhausted")]
    Exhausted,
}

/// Moves raw bytes to and from the camera.
pub trait Transport {
    /// Send all of `bytes`.
    fn send(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), Error>> + Send;
    /// Receive the next chunk of bytes. `Ok(None)` means the peer closed the connection.
    ///
    /// Must be cancellation safe: dropping a pending receive must not consume bytes.
    /// Streaming clients select between receiving output and forwarding user input.
    fn recv(&mut self) -> impl Future<Output = Result<Option<Vec<u8>>, Error>> + Send;
}
