// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Turns a byte stream into [`BusFrame`]s.
//!
//! TCP delivers arbitrary chunks: a frame may be split across reads and one read may hold several frames.
//! If bytes do not decode (corruption, or a message type we do not know), the framer drops one byte and
//! tries again, reporting each skipped byte so the event log shows exactly where the stream desynchronised.

use hblctl_proto::bus::{FRAME_SIZE, HEADER_SIZE};
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
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
}

impl Framer {
    /// An empty framer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
                Ok((frame, used)) => {
                    self.buf.drain(..used);
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
        let mut f = Framer::new();
        assert_eq!(f.push(&bytes[..3]), vec![]);
        assert_eq!(f.push(&bytes[3..200]), vec![]);
        assert_eq!(frames(f.push(&bytes[200..])), vec![a, b]);
        assert_eq!(f.pending(), 0);
    }

    #[test]
    fn resynchronises_after_garbage() {
        let a = frame(7);
        let mut f = Framer::new();
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

    proptest! {
        #[test]
        fn any_chunking_yields_the_same_frames(cuts in proptest::collection::vec(1usize..100, 0..20)) {
            let want: Vec<BusFrame> = (0..3).map(frame).collect();
            let bytes: Vec<u8> = want.iter().flat_map(BusFrame::encode).collect();
            let mut f = Framer::new();
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
            let _ = Framer::new().push(&noise);
        }
    }
}
