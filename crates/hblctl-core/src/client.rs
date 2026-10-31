// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Sends one command and waits for its result.
//!
//! The camera answers a command with frames that echo its `cmd` and `cookie`:
//! * func 4: a notification (zero or more), carried along with the result;
//! * func 6: a progress report, word at 0x10 = sign, word at 0x14 = magnitude;
//! * the request's own func: the final result, with a status word at 0x10 (0 = ok, 1 = error) and,
//!   for errors that carry detail, an error number at 0x14 and a UTF-8 message from 0x18.
//!
//! Frames that do not belong to the pending command are logged and kept as `stray` so nothing the camera
//! says is lost. Every step is logged under the `hblctl::proto` tracing target.

use std::time::Duration;

use hblctl_proto::bus::SIGNAL_TEST_TX;
use hblctl_proto::{BusFrame, Sutest};
use hblctl_transport::framer::{Event, Framer};
use hblctl_transport::{Error as TransportError, Transport};

/// Notification sub-function used by the camera.
pub const FUNC_NOTIFICATION: u32 = 4;
/// Progress sub-function used by the camera.
pub const FUNC_PROGRESS: u32 = 6;

/// Final status of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Status word 0.
    Ok,
    /// Status word 1, with optional detail.
    Error {
        /// Error number at 0x14 (0 when absent).
        code: u32,
        /// UTF-8 message from 0x18, trimmed at the first NUL (empty when absent).
        message: String,
    },
    /// Any other status word, reported as-is.
    Other(u32),
}

impl Status {
    /// Decode from a result record.
    #[must_use]
    pub fn of(record: &Sutest) -> Self {
        match record.u32_at(0x10).unwrap_or(u32::MAX) {
            0 => Status::Ok,
            1 => {
                let code = record.u32_at(0x14).unwrap_or(0);
                let raw = record.get(0x18, 0xFC - 0x18).unwrap_or(&[]);
                let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                Status::Error {
                    code,
                    message: String::from_utf8_lossy(&raw[..end]).into_owned(),
                }
            }
            x => Status::Other(x),
        }
    }
}

/// Everything the camera sent in answer to one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The final result record.
    pub result: Sutest,
    /// Its decoded status.
    pub status: Status,
    /// Notifications (func 4) received before the result.
    pub notifications: Vec<Sutest>,
    /// Progress values (func 6) received before the result.
    pub progress: Vec<i64>,
    /// Frames that did not belong to this command.
    pub stray: Vec<BusFrame>,
}

/// Client-level failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClientError {
    /// An unfinished shell owns this connection; reconnect before reuse.
    #[error("unfinished shell session; reconnect before using the client")]
    ShellActive,
    /// Invalid shell input.
    #[error("invalid shell input: {0}")]
    ShellInput(String),
    /// Shell execution requires destructive policy.
    #[error(transparent)]
    Refused(#[from] crate::safety::Refused),
    /// The transport failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// No final result arrived in time.
    #[error("no result for cmd {cmd} func {func} cookie 0x{cookie:08x} within {timeout:?}")]
    Timeout {
        /// Command id.
        cmd: u32,
        /// Sub-function.
        func: u32,
        /// Cookie sent.
        cookie: u32,
        /// Time waited.
        timeout: Duration,
    },
    /// The camera closed the connection before answering.
    #[error("connection closed before the result for cmd {cmd} func {func}")]
    Closed {
        /// Command id.
        cmd: u32,
        /// Sub-function.
        func: u32,
    },
}

/// A protocol client over any [`Transport`].
pub struct Client<T> {
    pub(crate) transport: T,
    pub(crate) framer: Framer,
    pub(crate) next_cookie: u32,
    pub(crate) timeout: Duration,
    pub(crate) shell_active: bool,
}

impl<T: Transport> Client<T> {
    /// Whether an unfinished shell requires discarding this connection.
    #[must_use]
    pub fn shell_active(&self) -> bool {
        self.shell_active
    }

    /// A client that waits up to `timeout` for each result. Cookies start at `first_cookie` and increment,
    /// so every request in a session log is unambiguous.
    pub fn new(transport: T, timeout: Duration, first_cookie: u32) -> Self {
        Self {
            shell_active: false,
            transport,
            framer: Framer::replies(),
            next_cookie: first_cookie,
            timeout,
        }
    }

    /// The underlying transport.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Send `cmd`/`func` with the given data region (bytes 0x10..0xFC) and wait for the result.
    pub async fn call(&mut self, cmd: u32, func: u32, data: &[u8]) -> Result<Outcome, ClientError> {
        if self.shell_active {
            return Err(ClientError::ShellActive);
        }
        if cmd == 65 {
            return Err(ClientError::ShellInput("use start_shell for command 65".into()));
        }
        let cookie = self.next_cookie;
        self.next_cookie = self.next_cookie.wrapping_add(1);
        let mut record = Sutest::new(cmd, func, cookie);
        let n = data.len().min(record.data.len());
        record.data[..n].copy_from_slice(&data[..n]);
        tracing::debug!(target: "hblctl::proto", cmd, func, cookie, request = ?record, "send");
        self.transport.send(&BusFrame::command(record).encode()).await?;

        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut out = Outcome {
            result: Sutest::new(cmd, func, cookie),
            status: Status::Ok,
            notifications: Vec::new(),
            progress: Vec::new(),
            stray: Vec::new(),
        };
        loop {
            let chunk = match tokio::time::timeout_at(deadline, self.transport.recv()).await {
                Err(_) => {
                    return Err(ClientError::Timeout {
                        cmd,
                        func,
                        cookie,
                        timeout: self.timeout,
                    });
                }
                Ok(r) => r?,
            };
            let Some(chunk) = chunk else {
                return Err(ClientError::Closed { cmd, func });
            };
            for ev in self.framer.push(&chunk) {
                let frame = match ev {
                    Event::Skipped { byte, reason } => {
                        tracing::warn!(target: "hblctl::proto", byte, %reason, "skipped byte while resynchronising");
                        continue;
                    }
                    Event::Frame(f) => *f,
                };
                let r = &frame.record;
                let ours = frame.header.signal == SIGNAL_TEST_TX && r.cmd == cmd && r.cookie == cookie;
                if !ours {
                    tracing::warn!(target: "hblctl::proto", header = ?frame.header, record = ?r, "stray frame");
                    out.stray.push(frame);
                } else if r.func == func {
                    out.status = Status::of(r);
                    tracing::debug!(target: "hblctl::proto", cmd, func, cookie, status = ?out.status, result = ?r, "result");
                    out.result = frame.record;
                    return Ok(out);
                } else if r.func == FUNC_NOTIFICATION {
                    tracing::debug!(target: "hblctl::proto", cmd, cookie, record = ?r, "notification");
                    out.notifications.push(frame.record);
                } else if r.func == FUNC_PROGRESS {
                    let neg = r.u32_at(0x10).unwrap_or(0) != 0;
                    let mag = i64::from(r.u32_at(0x14).unwrap_or(0));
                    let p = if neg { -mag } else { mag };
                    tracing::debug!(target: "hblctl::proto", cmd, cookie, progress = p, "progress");
                    out.progress.push(p);
                } else {
                    tracing::warn!(target: "hblctl::proto", record = ?r, "frame for this command with unexpected func");
                    out.stray.push(frame);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hblctl_proto::bus::{BusHeader, NODE_EAGLE, NODE_TCP_HOST, TYPE_SUTEST};
    use hblctl_transport::mock::{MockTransport, Step};

    fn reply(
        cmd: u32,
        func: u32,
        cookie: u32,
        words: &[(usize, u32)],
        text: Option<(usize, &[u8])>,
    ) -> Vec<u8> {
        let mut r = Sutest::new(cmd, func, cookie);
        for &(o, v) in words {
            r.put(o, &v.to_le_bytes()).unwrap();
        }
        if let Some((o, t)) = text {
            r.put(o, t).unwrap();
        }
        BusFrame {
            header: BusHeader {
                signal: SIGNAL_TEST_TX,
                origin: NODE_EAGLE,
                dest: NODE_TCP_HOST,
                kind: TYPE_SUTEST,
            },
            record: r,
        }
        .encode_reply_message()
    }

    fn request(cmd: u32, func: u32, cookie: u32, data: &[u8]) -> Vec<u8> {
        let mut r = Sutest::new(cmd, func, cookie);
        r.data[..data.len()].copy_from_slice(data);
        BusFrame::command(r).encode()
    }

    #[tokio::test]
    async fn ok_result_with_progress_and_notification_split_across_reads() {
        let mut stream = reply(49, FUNC_PROGRESS, 100, &[(0x10, 1), (0x14, 5)], None);
        stream.extend(reply(49, FUNC_NOTIFICATION, 100, &[(0x14, 7)], None));
        stream.extend(reply(49, 2, 100, &[(0x10, 0), (0x18, 6)], None));
        let (a, b) = stream.split_at(1300);
        let t = MockTransport::new([
            Step::Expect(request(49, 2, 100, &[0, 0, 0, 0, 13, 0, 0, 0])),
            Step::Reply(a.to_vec()),
            Step::Reply(b.to_vec()),
        ]);
        let mut c = Client::new(t, Duration::from_secs(1), 100);
        let out = c.call(49, 2, &[0, 0, 0, 0, 13, 0, 0, 0]).await.unwrap();
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.progress, vec![-5]);
        assert_eq!(out.notifications.len(), 1);
        assert_eq!(out.result.u32_at(0x18).unwrap(), 6);
        assert_eq!(out.stray, vec![]);
        assert!(c.transport().finished());
    }

    #[tokio::test]
    async fn error_detail_and_stray_frames() {
        let mut stream = reply(52, 1, 999, &[(0x10, 0)], None); // wrong cookie
        stream.extend(reply(
            52,
            1,
            7,
            &[(0x10, 1), (0x14, 13)],
            Some((0x18, b"permission denied\0junk")),
        ));
        let t = MockTransport::new([Step::Expect(request(52, 1, 7, &[])), Step::Reply(stream)]);
        let mut c = Client::new(t, Duration::from_secs(1), 7);
        let out = c.call(52, 1, &[]).await.unwrap();
        assert_eq!(
            out.status,
            Status::Error {
                code: 13,
                message: "permission denied".into()
            }
        );
        assert_eq!(out.stray.len(), 1);
    }

    #[tokio::test]
    async fn cookies_increment_and_close_is_reported() {
        let t = MockTransport::new([
            Step::Expect(request(1, 2, 5, &[])),
            Step::Reply(reply(1, 2, 5, &[], None)),
            Step::Expect(request(1, 2, 6, &[])),
            Step::Close,
        ]);
        let mut c = Client::new(t, Duration::from_secs(1), 5);
        c.call(1, 2, &[]).await.unwrap();
        assert!(matches!(
            c.call(1, 2, &[]).await,
            Err(ClientError::Closed { cmd: 1, func: 2 })
        ));
    }

    #[tokio::test]
    async fn other_status_word() {
        let t = MockTransport::new([
            Step::Expect(request(3, 0, 1, &[])),
            Step::Reply(reply(3, 0, 1, &[(0x10, 42)], None)),
        ]);
        let mut c = Client::new(t, Duration::from_secs(1), 1);
        assert_eq!(c.call(3, 0, &[]).await.unwrap().status, Status::Other(42));
    }
}
