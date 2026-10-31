// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Exclusive streaming `HblShell` sessions. Output is raw wire bytes: firmware UTF-8
//! truncation and Latin-1 final drains cannot be losslessly repaired by the client.
use crate::{Client, Policy, Safety, client::ClientError};
use hblctl_proto::{BusFrame, Sutest, bus::SIGNAL_TEST_TX};
use hblctl_transport::{Transport, framer::Event};
use std::collections::VecDeque;

/// An exclusive borrow of a client until shell completion. Dropping an unfinished
/// session leaves the client blocked; reconnect rather than sending into a live shell.
pub struct ShellSession<'a, T> {
    client: &'a mut Client<T>,
    pending: VecDeque<BusFrame>,
    status: Option<u32>,
    cookie: u32,
    deadline: tokio::time::Instant,
}
impl<T: Transport> Client<T> {
    /// Start `/bin/sh -c command`, or `/bin/sh -i` for an empty command.
    pub async fn start_shell(
        &mut self,
        policy: Policy,
        command: &str,
    ) -> Result<ShellSession<'_, T>, ClientError> {
        self.start_shell_with_delay(policy, command, 0).await
    }

    /// Start a shell with the firmware output-delay parameter (zero selects 10 ms).
    pub async fn start_shell_with_delay(
        &mut self,
        policy: Policy,
        command: &str,
        delay: u32,
    ) -> Result<ShellSession<'_, T>, ClientError> {
        policy.check("shell.run", Safety::Destructive)?;
        if self.shell_active {
            return Err(ClientError::ShellActive);
        }
        if command.len() > 231 || command.contains('\0') {
            return Err(ClientError::ShellInput(
                "command must fit 231 UTF-8 bytes without NUL".into(),
            ));
        }
        self.shell_active = true;
        let cookie = self.next_cookie;
        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut session = ShellSession {
            client: self,
            pending: VecDeque::new(),
            status: None,
            cookie,
            deadline,
        };
        session.send_record(1, command.as_bytes(), delay).await?;
        Ok(session)
    }
}
impl<T: Transport> ShellSession<'_, T> {
    async fn send(&mut self, func: u32, bytes: &[u8]) -> Result<(), ClientError> {
        self.send_record(func, bytes, 0).await
    }
    async fn send_record(&mut self, func: u32, bytes: &[u8], delay: u32) -> Result<(), ClientError> {
        if self.status.is_some() {
            return Err(ClientError::ShellInput("shell has completed".into()));
        }
        let cookie = self.client.next_cookie;
        self.client.next_cookie = cookie.wrapping_add(1);
        let mut record = Sutest::new(65, func, cookie);
        record.data[..4].copy_from_slice(&delay.to_le_bytes());
        record.data[4..4 + bytes.len()].copy_from_slice(bytes);
        tracing::debug!(target: "hblctl::proto", request = ?record, "shell send");
        tokio::time::timeout_at(
            self.deadline,
            self.client.transport.send(&BusFrame::command(record).encode()),
        )
        .await
        .map_err(|_| self.timeout())??;
        Ok(())
    }
    fn timeout(&self) -> ClientError {
        ClientError::Timeout {
            cmd: 65,
            func: 1,
            cookie: self.cookie,
            timeout: self.client.timeout,
        }
    }
    /// Reject stdin uploads: the bridge has no framing or acknowledgement for them.
    /// Use a complete command in the initial request instead. ETX is a signal,
    /// available explicitly through `interrupt()`, never stdin data.
    pub fn write_stdin(&mut self, text: &str) -> std::future::Ready<Result<(), ClientError>> {
        let reason = if text.contains('\x03') {
            "stdin contains ETX; use interrupt() for SIGINT"
        } else if text.contains('\0') {
            "stdin contains NUL"
        } else {
            "stdin uploads unsupported: bridge has no record acknowledgement or framing"
        };
        std::future::ready(Err(ClientError::ShellInput(reason.into())))
    }
    /// Send SIGINT to the child PID (not its process group).
    pub async fn interrupt(&mut self) -> Result<(), ClientError> {
        self.send(1, &[3]).await
    }
    /// Send SIGKILL to the child PID; wait for the eventual original result.
    pub async fn kill(&mut self) -> Result<(), ClientError> {
        self.send(5, &[]).await
    }
    /// Receive the next output notification, preserving bytes up to the first NUL.
    /// Empty drain records are output, not EOF. No cookie matching is performed.
    /// Cancellation is safe while waiting for transport input.
    pub async fn next_output(&mut self) -> Result<Option<Vec<u8>>, ClientError> {
        loop {
            if self.status.is_some() {
                return Ok(None);
            }
            if let Some(frame) = self.pending.pop_front() {
                tracing::debug!(target: "hblctl::proto", header = ?frame.header, record = ?frame.record, "shell receive");
                let r = frame.record;
                if frame.header.signal != SIGNAL_TEST_TX || r.cmd != 65 {
                    continue;
                }
                if r.func == 4 {
                    let b = &r.data[4..];
                    return Ok(Some(
                        b[..b.iter().position(|b| *b == 0).unwrap_or(b.len())].to_vec(),
                    ));
                }
                if r.func == 1 {
                    self.status = Some(r.u32_at(16).unwrap_or(1));
                    self.client.shell_active = false;
                }
                continue;
            }
            let b = tokio::time::timeout_at(self.deadline, self.client.transport.recv())
                .await
                .map_err(|_| self.timeout())??
                .ok_or(ClientError::Closed { cmd: 65, func: 1 })?;
            for event in self.client.framer.push(&b) {
                match event {
                    Event::Frame(f) => self.pending.push_back(*f),
                    Event::Skipped { byte, reason } => {
                        tracing::warn!(target:"hblctl::proto", byte, %reason, "shell skipped byte");
                    }
                }
            }
        }
    }
    /// Drain remaining output and return the wire success/failure status (0 or 1).
    /// `HblShell` does not transmit the child's numeric exit code.
    pub async fn wait(&mut self) -> Result<u32, ClientError> {
        while self.next_output().await?.is_some() {}
        Ok(self.status.unwrap_or(1))
    }
}

impl<T> Drop for ShellSession<'_, T> {
    fn drop(&mut self) {
        // A cancelled or abandoned session cannot release the connection for commands:
        // the camera may still route them into the shell. Only completion releases it.
        self.client.shell_active = self.status.is_none();
    }
}
