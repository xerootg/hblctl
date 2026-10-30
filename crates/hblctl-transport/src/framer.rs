// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Turns a byte stream into [`BusFrame`]s.
//!
//! TCP delivers arbitrary chunks: a message may be split across reads and one read may hold several.
//! Requests are bare frames; replies from the camera are fixed-size messages (a frame followed by filler),
//! so the framer is built for one direction ([`Framer::requests`] or [`Framer::replies`]).
//! If bytes do not decode (corruption, or a message type we do not know), the framer drops one byte and
//! tries again, reporting each skipped byte so the event log shows exactly where the stream desynchronised.

use hblctl_proto::bus::{FRAME_SIZE, HEADER_SIZE, REPLY_MESSAGE_SIZE};
use hblctl_proto::{BusFrame, DecodeError};

/// What the framer produced from the bytes fed so far.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// A complete, CRC-valid frame.
    Frame(Box<BusFrame>),
    /// One byte was discarded to resynchronise.
    Skipped {
        /// The discarded byte.
        byte: u8,
        /// Why the bytes at this position did not decode.
        reason: DecodeError,
    },
}

/// Incremental frame decoder.
#[derive(Debug)]
pub struct Framer {
    buf: Vec<u8>,
    message_size: usize,
}

impl Framer {
    /// For bytes sent to the camera: each message is exactly one frame.
    #[must_use]
    pub fn requests() -> Self {
        Self {
            buf: Vec::new(),
            message_size: FRAME_SIZE,
        }
    }

    /// For bytes received from the camera: each message is [`REPLY_MESSAGE_SIZE`] bytes, a frame then filler.
    #[must_use]
    pub fn replies() -> Self {
        Self {
            buf: Vec::new(),
            message_size: REPLY_MESSAGE_SIZE,
        }
    }

    /// Bytes buffered but not yet decoded.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Feed bytes; returns every event they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < HEADER_SIZE {
                break;
            }
            match BusFrame::decode(&self.buf) {
                Ok(_) if self.buf.len() < self.message_size => break,
                Ok((frame, _)) => {
                    self.buf.drain(..self.message_size);
                    out.push(Event::Frame(Box::new(frame)));
                }
                Err(DecodeError::Short { .. }) if self.buf.len() < FRAME_SIZE => break,
                Err(reason) => {
                    let byte = self.buf.remove(0);
                    out.push(Event::Skipped { byte, reason });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hblctl_proto::Sutest;
    use proptest::prelude::*;

    fn frame(cookie: u32) -> BusFrame {
        BusFrame::command(Sutest::new(49, 2, cookie))
    }

    fn frames(ev: Vec<Event>) -> Vec<BusFrame> {
        ev.into_iter()
            .filter_map(|e| if let Event::Frame(f) = e { Some(*f) } else { None })
            .collect()
    }

    #[test]
    fn split_and_coalesced_reads() {
        let (a, b) = (frame(1), frame(2));
        let bytes = [a.encode(), b.encode()].concat();
        let mut f = Framer::requests();
        assert_eq!(f.push(&bytes[..3]), vec![]);
        assert_eq!(f.push(&bytes[3..200]), vec![]);
        assert_eq!(frames(f.push(&bytes[200..])), vec![a, b]);
        assert_eq!(f.pending(), 0);
    }

    #[test]
    fn resynchronises_after_garbage() {
        let a = frame(7);
        let mut f = Framer::requests();
        let ev = f.push(&[[0xAA, 0xBB].as_slice(), &a.encode()].concat());
        let skipped: Vec<u8> = ev
            .iter()
            .filter_map(|e| {
                if let Event::Skipped { byte, .. } = e {
                    Some(*byte)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(skipped, vec![0xAA, 0xBB]);
        assert_eq!(frames(ev), vec![a]);
    }

    /// A reply message as `msg2dbus` sends it: frame, three zeros, then arbitrary filler to 1024 bytes.
    fn reply_message(cookie: u32, filler: u8) -> Vec<u8> {
        let mut m = frame(cookie).encode();
        m.extend_from_slice(&[0, 0, 0]);
        m.resize(REPLY_MESSAGE_SIZE, filler);
        m
    }

    #[test]
    fn reply_messages_are_fixed_size_and_filler_is_ignored() {
        let bytes = [reply_message(1, 0xAA), reply_message(2, 0x05)].concat();
        let mut f = Framer::replies();
        assert_eq!(
            f.push(&bytes[..600]),
            vec![],
            "a frame is not complete until its whole message arrived"
        );
        let ev = f.push(&bytes[600..]);
        assert_eq!(frames(ev), vec![frame(1), frame(2)]);
        assert_eq!(f.pending(), 0);
    }

    proptest! {
        #[test]
        fn any_chunking_yields_the_same_frames(cuts in proptest::collection::vec(1usize..100, 0..20)) {
            let want: Vec<BusFrame> = (0..3).map(frame).collect();
            let bytes: Vec<u8> = want.iter().flat_map(BusFrame::encode).collect();
            let mut f = Framer::requests();
            let mut got = Vec::new();
            let mut pos = 0;
            for c in cuts {
                let end = (pos + c).min(bytes.len());
                got.extend(frames(f.push(&bytes[pos..end])));
                pos = end;
            }
            got.extend(frames(f.push(&bytes[pos..])));
            prop_assert_eq!(got, want);
        }

        #[test]
        fn never_panics_on_noise(noise in proptest::collection::vec(any::<u8>(), 0..1000)) {
            let _ = Framer::requests().push(&noise);
        }
    }
}
