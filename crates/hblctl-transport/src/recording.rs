// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Wraps any [`Transport`] and writes every byte that crosses it to a capture.

use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::capture::{CaptureWriter, Direction};
use crate::{Error, Transport};

/// A transport that records everything passing through `inner`.
///
/// The capture is written before a send and after a receive, so if the program dies the capture still
/// shows what was put on the wire. Failures to write the capture are returned as errors rather than
/// ignored: a session without its record is worse than a failed session.
pub struct Recording<T, W: Write> {
    inner: T,
    capture: Arc<Mutex<CaptureWriter<W>>>,
}

impl<T: Transport, W: Write + Send> Recording<T, W> {
    /// Record `inner` into `capture`.
    pub fn new(inner: T, capture: CaptureWriter<W>) -> Self {
        Self {
            inner,
            capture: Arc::new(Mutex::new(capture)),
        }
    }

    fn log(&self, d: Direction, b: &[u8]) -> Result<(), Error> {
        let mut c = self
            .capture
            .lock()
            .map_err(|_| Error::Io(std::io::Error::other("capture lock poisoned")))?;
        c.write(d, b)?;
        Ok(())
    }
}

impl<T: Transport + Send, W: Write + Send> Transport for Recording<T, W> {
    async fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.log(Direction::Sent, bytes)?;
        tracing::trace!(target: "hblctl::wire", dir = "tx", len = bytes.len(), hex = %hex(bytes));
        self.inner.send(bytes).await
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let r = self.inner.recv().await?;
        if let Some(b) = &r {
            self.log(Direction::Received, b)?;
            tracing::trace!(target: "hblctl::wire", dir = "rx", len = b.len(), hex = %hex(b));
        }
        Ok(r)
    }
}

/// Lower-case hex, for log lines.
#[must_use]
pub fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::with_capacity(b.len() * 2), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureReader, Record};
    use crate::mock::{MockTransport, Step};

    #[tokio::test]
    async fn records_both_directions_in_order() {
        let mock = MockTransport::new([Step::Expect(vec![1]), Step::Reply(vec![2, 3]), Step::Close]);
        let mut buf = Vec::new();
        {
            let mut t = Recording::new(mock, CaptureWriter::new(&mut buf).unwrap());
            t.send(&[1]).await.unwrap();
            assert_eq!(t.recv().await.unwrap(), Some(vec![2, 3]));
            assert_eq!(t.recv().await.unwrap(), None);
        }
        let recs = CaptureReader::new(&buf[..]).unwrap().records().unwrap();
        let summary: Vec<(Direction, Vec<u8>)> = recs
            .into_iter()
            .map(|Record { direction, bytes, .. }| (direction, bytes))
            .collect();
        assert_eq!(
            summary,
            vec![(Direction::Sent, vec![1]), (Direction::Received, vec![2, 3])]
        );
    }

    #[tokio::test]
    async fn records_the_send_even_when_the_transport_fails() {
        let mock = MockTransport::new([Step::Expect(vec![9])]);
        let mut buf = Vec::new();
        {
            let mut t = Recording::new(mock, CaptureWriter::new(&mut buf).unwrap());
            assert!(t.send(&[1]).await.is_err());
        }
        let recs = CaptureReader::new(&buf[..]).unwrap().records().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].bytes, vec![1]);
    }

    #[test]
    fn hex_format() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
    }
}
