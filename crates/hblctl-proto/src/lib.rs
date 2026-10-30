// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Wire format of the Hasselblad X-system factory diagnostic protocol ("sutest").
//!
//! Layering, outermost first:
//! * [`bus`]: the 5-byte message-bus header that `camera-gui` relays between TCP port 30303 and
//!   the root `camera-test` service.
//! * [`sutest`]: the fixed 252-byte command/result record carried inside it.
//! * [`crc`]: CRC-16/XMODEM, which guards bytes `0x10..0xFC` of the record.
//!
//! This crate does no I/O and never panics on untrusted input: decoding returns [`DecodeError`].

pub mod bus;
pub mod crc;
pub mod sutest;

pub use bus::{BusFrame, BusHeader};
pub use sutest::Sutest;

/// Why a byte sequence could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// The input ended early.
    #[error("need {need} bytes, have {have}")]
    Short {
        /// Bytes required.
        need: usize,
        /// Bytes available.
        have: usize,
    },
    /// The bus header announced a body other than a sutest record.
    #[error("unexpected message type 0x{0:02x}")]
    MessageType(u8),
    /// The stored CRC does not match the record.
    #[error("crc mismatch: stored 0x{stored:04x}, computed 0x{computed:04x}")]
    Crc {
        /// CRC field as found on the wire.
        stored: u32,
        /// CRC computed over bytes 0x10..0xFC.
        computed: u16,
    },
    /// A write would run past the end of the record.
    #[error("payload of {len} bytes exceeds the {max}-byte field")]
    PayloadTooLong {
        /// Bytes requested.
        len: usize,
        /// Bytes that fit.
        max: usize,
    },
}
