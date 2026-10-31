// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Replays a capture as a transport: sends must match what was recorded, receives return what was
//! recorded. A failing field session becomes an integration test by dropping its capture into a test.

use std::collections::VecDeque;

use crate::capture::{Direction, Record};
use crate::mock::describe_mismatch;
use crate::{Error, Transport};

/// Transport that reproduces a recorded session.
#[derive(Debug)]
pub struct ReplayTransport {
    records: VecDeque<Record>,
    index: usize,
    strict: bool,
}

impl ReplayTransport {
    /// Replay `records`. In strict mode every send must equal the recorded send byte for byte; otherwise
    /// sends are only checked for length (useful when cookies differ between runs).
    #[must_use]
    pub fn new(records: Vec<Record>, strict: bool) -> Self {
        Self {
            records: records.into(),
            index: 0,
            strict,
        }
    }

    /// True when the whole capture was consumed.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.records.is_empty()
    }
}

impl ReplayTransport {
    fn send_now(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let step = self.index;
        let Some(r) = self.records.pop_front() else {
            return Err(Error::Exhausted);
        };
        self.index += 1;
        if r.direction != Direction::Sent {
            return Err(Error::Unexpected {
                step,
                detail: format!(
                    "capture has a receive of {} bytes here, code sent {}",
                    r.bytes.len(),
                    bytes.len()
                ),
            });
        }
        let ok = if self.strict {
            r.bytes == bytes
        } else {
            r.bytes.len() == bytes.len()
        };
        if ok {
            Ok(())
        } else {
            Err(Error::Unexpected {
                step,
                detail: describe_mismatch(&r.bytes, bytes),
            })
        }
    }

    fn recv_now(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let step = self.index;
        match self.records.front() {
            None => Ok(None),
            Some(r) if r.direction == Direction::Sent => Err(Error::Unexpected {
                step,
                detail: format!(
                    "code waited for data, capture has a send of {} bytes here",
                    r.bytes.len()
                ),
            }),
            Some(_) => {
                self.index += 1;
                Ok(self.records.pop_front().map(|r| r.bytes))
            }
        }
    }
}

impl Transport for ReplayTransport {
    fn send(&mut self, bytes: &[u8]) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        std::future::ready(self.send_now(bytes))
    }

    // The lazy body is required: constructing and dropping an unpolled recv must not consume a record.
    #[allow(clippy::unused_async_trait_impl)]
    async fn recv(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.recv_now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(direction: Direction, b: &[u8]) -> Record {
        Record {
            direction,
            t_ns: 0,
            bytes: b.to_vec(),
        }
    }

    #[tokio::test]
    async fn strict_replay() {
        let mut t = ReplayTransport::new(
            vec![r(Direction::Sent, &[1, 2]), r(Direction::Received, &[3])],
            true,
        );
        t.send(&[1, 2]).await.unwrap();
        assert_eq!(t.recv().await.unwrap(), Some(vec![3]));
        assert_eq!(
            t.recv().await.unwrap(),
            None,
            "end of capture reads as connection closed"
        );
        assert!(t.finished());
    }

    #[tokio::test]
    async fn detects_divergence() {
        let mut t = ReplayTransport::new(vec![r(Direction::Sent, &[1, 2])], true);
        assert!(matches!(
            t.send(&[1, 3]).await,
            Err(Error::Unexpected { step: 0, .. })
        ));
        let mut t = ReplayTransport::new(vec![r(Direction::Sent, &[1, 2])], false);
        t.send(&[9, 9]).await.unwrap();
        let mut t = ReplayTransport::new(vec![r(Direction::Sent, &[1])], true);
        assert!(
            t.recv().await.is_err(),
            "receiving when the capture says we sent is a divergence"
        );
    }
    #[tokio::test]
    async fn dropping_unpolled_receive_preserves_record() {
        let mut t = ReplayTransport::new(vec![r(Direction::Received, &[9])], true);
        drop(t.recv());
        assert_eq!(t.recv().await.unwrap(), Some(vec![9]));
        assert_eq!(t.recv().await.unwrap(), None);
        assert!(t.finished());
    }
}
