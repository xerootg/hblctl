// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Capture file format: every byte that crossed the socket, in order, with direction and time.
//!
//! ```text
//! header : b"HBLCAP\0\x01"                      (magic + format version 1)
//! record : u8 direction (0 = sent, 1 = received)
//!          u64 LE nanoseconds since the capture started
//!          u32 LE length
//!          length bytes
//! ```
//! Records are flushed one by one, so a crash loses at most the record being written.

use std::io::{self, Read, Write};
use std::time::Instant;

/// File magic, including the format version byte.
pub const MAGIC: &[u8; 8] = b"HBLCAP\0\x01";

/// Which way a record travelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From this program to the camera.
    Sent,
    /// From the camera to this program.
    Received,
}

/// One captured chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Direction of travel.
    pub direction: Direction,
    /// Nanoseconds since the capture started.
    pub t_ns: u64,
    /// The bytes exactly as they crossed the socket.
    pub bytes: Vec<u8>,
}

/// Writes a capture.
pub struct CaptureWriter<W: Write> {
    out: W,
    start: Instant,
}

impl<W: Write> CaptureWriter<W> {
    /// Start a capture on `out` (writes the header).
    pub fn new(mut out: W) -> io::Result<Self> {
        out.write_all(MAGIC)?;
        out.flush()?;
        Ok(Self {
            out,
            start: Instant::now(),
        })
    }

    /// Append one record, timestamped now.
    pub fn write(&mut self, direction: Direction, bytes: &[u8]) -> io::Result<()> {
        let t_ns = u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.write_record(&Record {
            direction,
            t_ns,
            bytes: bytes.to_vec(),
        })
    }

    /// Append a record with an explicit timestamp.
    pub fn write_record(&mut self, r: &Record) -> io::Result<()> {
        let len = u32::try_from(r.bytes.len()).map_err(|_| io::Error::other("record larger than 4 GiB"))?;
        self.out.write_all(&[match r.direction {
            Direction::Sent => 0,
            Direction::Received => 1,
        }])?;
        self.out.write_all(&r.t_ns.to_le_bytes())?;
        self.out.write_all(&len.to_le_bytes())?;
        self.out.write_all(&r.bytes)?;
        self.out.flush()
    }
}

/// Reads a capture back.
pub struct CaptureReader<R: Read> {
    input: R,
}

impl<R: Read> CaptureReader<R> {
    /// Open a capture and check its header.
    pub fn new(mut input: R) -> io::Result<Self> {
        let mut magic = [0u8; 8];
        input.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not an hblctl capture (bad magic/version)",
            ));
        }
        Ok(Self { input })
    }

    /// Next record, `Ok(None)` at a clean end of file. A record cut off mid-way (crash while writing)
    /// is reported as `UnexpectedEof`.
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        let mut dir = [0u8; 1];
        if self.input.read(&mut dir)? == 0 {
            return Ok(None);
        }
        let direction = match dir[0] {
            0 => Direction::Sent,
            1 => Direction::Received,
            x => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad direction byte {x}"),
                ));
            }
        };
        let mut t = [0u8; 8];
        self.input.read_exact(&mut t)?;
        let mut l = [0u8; 4];
        self.input.read_exact(&mut l)?;
        let mut bytes = vec![0u8; u32::from_le_bytes(l) as usize];
        self.input.read_exact(&mut bytes)?;
        Ok(Some(Record {
            direction,
            t_ns: u64::from_le_bytes(t),
            bytes,
        }))
    }

    /// All remaining records.
    pub fn records(mut self) -> io::Result<Vec<Record>> {
        let mut v = Vec::new();
        while let Some(r) = self.next_record()? {
            v.push(r);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn rec(direction: Direction, t_ns: u64, bytes: &[u8]) -> Record {
        Record {
            direction,
            t_ns,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn roundtrip_and_layout() {
        let mut buf = Vec::new();
        let mut w = CaptureWriter::new(&mut buf).unwrap();
        w.write_record(&rec(Direction::Sent, 5, b"ab")).unwrap();
        w.write_record(&rec(Direction::Received, 9, b"")).unwrap();
        assert_eq!(&buf[..8], MAGIC);
        assert_eq!(&buf[8..23], &[0, 5, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, b'a', b'b']);
        let got = CaptureReader::new(&buf[..]).unwrap().records().unwrap();
        assert_eq!(
            got,
            vec![rec(Direction::Sent, 5, b"ab"), rec(Direction::Received, 9, b"")]
        );
    }

    #[test]
    fn rejects_foreign_files_and_reports_truncation() {
        assert!(CaptureReader::new(&b"PCAPPCAP"[..]).is_err());
        let mut buf = Vec::new();
        CaptureWriter::new(&mut buf)
            .unwrap()
            .write_record(&rec(Direction::Sent, 1, b"abcd"))
            .unwrap();
        buf.truncate(buf.len() - 1);
        let err = CaptureReader::new(&buf[..]).unwrap().records().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    proptest! {
        #[test]
        fn any_records_roundtrip(rs in proptest::collection::vec((any::<bool>(), any::<u64>(), proptest::collection::vec(any::<u8>(), 0..64)), 0..20)) {
            let recs: Vec<Record> = rs.into_iter().map(|(s, t, b)| rec(if s { Direction::Sent } else { Direction::Received }, t, &b)).collect();
            let mut buf = Vec::new();
            let mut w = CaptureWriter::new(&mut buf).unwrap();
            for r in &recs { w.write_record(r).unwrap(); }
            prop_assert_eq!(CaptureReader::new(&buf[..]).unwrap().records().unwrap(), recs);
        }
    }
}
