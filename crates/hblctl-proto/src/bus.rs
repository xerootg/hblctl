// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! The message-bus header `camera-gui` uses to relay frames between TCP and `camera-test`.
//!
//! ```text
//! 0 u16 LE signal   0x05 = test command to the camera, 0x09 = test result from the camera
//! 2 u8     origin   sending node (9 = TCP host)
//! 3 u8     dest     receiving node (5 = "eagle", the camera)
//! 4 u8     type     0xFC: a 252-byte sutest record follows
//! 5 ..     body     the record
//! ```
//! Values are from the public reference clients; the full set is pending the firmware analysis.

use crate::DecodeError;
use crate::sutest::{self, Sutest};

/// Signal for a test command sent to the camera.
pub const SIGNAL_TEST_RX: u16 = 0x05;
/// Signal for a test result sent by the camera.
pub const SIGNAL_TEST_TX: u16 = 0x09;
/// Node id of the TCP host (the client side of port 30303).
pub const NODE_TCP_HOST: u8 = 9;
/// Node id of the camera ("eagle").
pub const NODE_EAGLE: u8 = 5;
/// Message type: a sutest record follows.
pub const TYPE_SUTEST: u8 = 0xFC;
/// Size of the bus header.
pub const HEADER_SIZE: usize = 5;
/// Size of a header plus one sutest record.
pub const FRAME_SIZE: usize = HEADER_SIZE + sutest::SIZE;

/// The 5-byte bus header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusHeader {
    /// Signal id (`SIGNAL_TEST_RX` or `SIGNAL_TEST_TX`).
    pub signal: u16,
    /// Sending node.
    pub origin: u8,
    /// Receiving node.
    pub dest: u8,
    /// Message type (`TYPE_SUTEST`).
    pub kind: u8,
}

/// A bus header followed by one sutest record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusFrame {
    /// The header.
    pub header: BusHeader,
    /// The record.
    pub record: Sutest,
}

impl BusFrame {
    /// A command from the TCP host to the camera.
    #[must_use]
    pub fn command(record: Sutest) -> Self {
        Self {
            header: BusHeader {
                signal: SIGNAL_TEST_RX,
                origin: NODE_TCP_HOST,
                dest: NODE_EAGLE,
                kind: TYPE_SUTEST,
            },
            record,
        }
    }

    /// Encode header and record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(FRAME_SIZE);
        v.extend_from_slice(&self.header.signal.to_le_bytes());
        v.extend_from_slice(&[self.header.origin, self.header.dest, self.header.kind]);
        v.extend_from_slice(&self.record.encode());
        v
    }

    /// Decode one frame from the start of `b`; returns the frame and the bytes consumed.
    pub fn decode(b: &[u8]) -> Result<(Self, usize), DecodeError> {
        if b.len() < HEADER_SIZE {
            return Err(DecodeError::Short {
                need: HEADER_SIZE,
                have: b.len(),
            });
        }
        let header = BusHeader {
            signal: u16::from_le_bytes([b[0], b[1]]),
            origin: b[2],
            dest: b[3],
            kind: b[4],
        };
        if header.kind != TYPE_SUTEST {
            return Err(DecodeError::MessageType(header.kind));
        }
        let record = Sutest::decode(&b[HEADER_SIZE..])?;
        Ok((Self { header, record }, FRAME_SIZE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_header_bytes() {
        let f = BusFrame::command(Sutest::new(49, 2, 0x4842)).encode();
        assert_eq!(&f[..5], &[0x05, 0x00, 9, 5, 0xFC]);
        assert_eq!(f.len(), 257);
    }

    #[test]
    fn decode_roundtrip_and_errors() {
        let f = BusFrame::command(Sutest::new(65, 1, 7));
        let bytes = f.encode();
        assert_eq!(BusFrame::decode(&bytes).unwrap(), (f, 257));
        assert!(matches!(
            BusFrame::decode(&bytes[..3]),
            Err(DecodeError::Short { .. })
        ));
        let mut bad = bytes.clone();
        bad[4] = 0x01;
        assert_eq!(BusFrame::decode(&bad), Err(DecodeError::MessageType(1)));
    }
}
