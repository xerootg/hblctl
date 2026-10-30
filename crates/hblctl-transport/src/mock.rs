// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! A scripted transport for unit tests: each step either expects exact bytes to be sent or supplies
//! bytes to receive.

use std::collections::VecDeque;

use crate::{Error, Transport};

/// One scripted step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The code under test must send exactly these bytes next.
    Expect(Vec<u8>),
    /// The next `recv` returns these bytes.
    Reply(Vec<u8>),
    /// The next `recv` reports the peer closing the connection.
    Close,
}

/// Transport driven by a script of [`Step`]s.
#[derive(Debug, Default)]
pub struct MockTransport {
    steps: VecDeque<Step>,
    done: usize,
}

impl MockTransport {
    /// A transport that will play `steps` in order.
    #[must_use]
    pub fn new(steps: impl IntoIterator<Item = Step>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            done: 0,
        }
    }

    /// True when every step was consumed.
    #[must_use]
    pub fn finished(&self) -> bool {
        self.steps.is_empty()
    }
}

impl MockTransport {
    fn send_now(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let step = self.done;
        match self.steps.pop_front() {
            Some(Step::Expect(want)) if want == bytes => {
                self.done += 1;
                Ok(())
            }
            Some(Step::Expect(want)) => Err(Error::Unexpected {
                step,
                detail: describe_mismatch(&want, bytes),
            }),
            Some(other) => Err(Error::Unexpected {
                step,
                detail: format!("sent {} bytes, script expected {other:?}", bytes.len()),
            }),
            None => Err(Error::Exhausted),
        }
    }

    fn recv_now(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let step = self.done;
        match self.steps.pop_front() {
            Some(Step::Reply(b)) => {
                self.done += 1;
                Ok(Some(b))
            }
            Some(Step::Close) => {
                self.done += 1;
                Ok(None)
            }
            Some(other) => Err(Error::Unexpected {
                step,
                detail: format!("recv called, script expected {other:?}"),
            }),
            None => Err(Error::Exhausted),
        }
    }
}

impl Transport for MockTransport {
    fn send(&mut self, bytes: &[u8]) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        std::future::ready(self.send_now(bytes))
    }

    fn recv(&mut self) -> impl std::future::Future<Output = Result<Option<Vec<u8>>, Error>> + Send {
        std::future::ready(self.recv_now())
    }
}

/// First differing offset and the bytes around it, for readable test failures.
pub(crate) fn describe_mismatch(want: &[u8], got: &[u8]) -> String {
    let at = want
        .iter()
        .zip(got)
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));
    let w = &want[at..want.len().min(at + 8)];
    let g = &got[at..got.len().min(at + 8)];
    format!(
        "lengths {}/{} (want/got); first difference at offset {at}: want {w:02x?}, got {g:02x?}",
        want.len(),
        got.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn plays_script_and_reports_mismatch() {
        let mut t = MockTransport::new([Step::Expect(vec![1, 2]), Step::Reply(vec![9]), Step::Close]);
        t.send(&[1, 2]).await.unwrap();
        assert_eq!(t.recv().await.unwrap(), Some(vec![9]));
        assert_eq!(t.recv().await.unwrap(), None);
        assert!(t.finished());
        assert!(matches!(t.recv().await, Err(Error::Exhausted)));

        let mut t = MockTransport::new([Step::Expect(vec![1, 2, 3])]);
        let Err(Error::Unexpected { step, detail }) = t.send(&[1, 7, 3]).await else {
            panic!("expected mismatch")
        };
        assert_eq!(step, 0);
        assert!(detail.contains("offset 1"), "{detail}");
    }
}
