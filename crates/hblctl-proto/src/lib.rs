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
    #[error("need {need} bytes, have {have}")]
    Short { need: usize, have: usize },
    #[error("unexpected message type 0x{0:02x}")]
    MessageType(u8),
    #[error("crc mismatch: stored 0x{stored:04x}, computed 0x{computed:04x}")]
    Crc { stored: u32, computed: u16 },
    #[error("payload of {len} bytes exceeds the {max}-byte field")]
    PayloadTooLong { len: usize, max: usize },
}
