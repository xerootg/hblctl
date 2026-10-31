// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Terminal ownership and a serial connection worker. No I/O lives in App.
use super::{
    app::{App, Effect, Event},
    view,
};
use crate::{camera::CameraSession, cli::Call};
use anyhow::{Context, Result};
use crossterm::event::{self, KeyCode, KeyEventKind, KeyModifiers};
use hblctl_core::{Client, registry::Registry, safety::Policy};
use hblctl_transport::Transport;
use serde_json::Value;
use std::{
    sync::mpsc::{self, Receiver},
    time::Duration,
};

pub(crate) async fn invoke<T: Transport>(
    client: &mut Client<T>,
    registry: &Registry,
    policy: Policy,
    call: Call,
) -> Result<Value, String> {
    hblctl_core::dispatch::invoke(client, registry, policy, &call.command, &call.func, &call.args)
        .await
        .map_err(|e| e.to_string())
}

fn event_channel() -> (mpsc::SyncSender<Event>, Receiver<Event>) {
    mpsc::sync_channel(64)
}

pub(crate) fn run(
    camera: CameraSession,
    registry: Registry,
    policy: Policy,
    logs: &Receiver<String>,
) -> Result<()> {
    let mut app = App::new(registry.clone(), policy);
    let (requests, work) = mpsc::channel();
    let (events, updates) = event_channel();
    let runtime = tokio::runtime::Runtime::new()?;
    let worker = std::thread::spawn(move || {
        runtime.block_on(async move {
            let mut client = None;
            while let Ok(effect) = work.recv() {
                let reconnect = matches!(effect, Effect::Reconnect);
                if reconnect {
                    client = None;
                } // Drop the old socket before opening another.
                if client.is_none() {
                    let _ = events.send(Event::Connection("connecting".into()));
                    match camera.connect().await {
                        Ok(c) => {
                            client = Some(c);
                            let _ = events.send(Event::Connection("connected".into()));
                        }
                        Err(e) => {
                            let _ = events.send(Event::Connection("disconnected".into()));
                            let _ = events.send(Event::Completed(Err(format!("{e:#}"))));
                            continue;
                        }
                    }
                }
                match effect {
                    Effect::Invoke(call) => {
                        let is_shell = call.command == "hbl-shell";
                        let result = if is_shell {
                            let result: anyhow::Result<Value> = async {
                                let command = call
                                    .args
                                    .get("command_utf8_z")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                anyhow::ensure!(
                                    !command.is_empty(),
                                    "non-interactive shell command must not be empty"
                                );
                                let spec = registry
                                    .command(&call.command)
                                    .and_then(|c| c.func(&call.func))
                                    .ok_or_else(|| anyhow::anyhow!("unknown shell operation"))?;
                                let data = hblctl_core::codec::encode(spec, &call.args)?;
                                let delay = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                                let mut shell = client
                                    .as_mut()
                                    .expect("connected")
                                    .start_shell_with_delay(policy, command, delay)
                                    .await?;

                                let mut decoder = ShellDecoder::default();
                                while let Some(bytes) = shell.next_output().await? {
                                    let chunk = decoder.push(&bytes, false);
                                    let _ = events.send(Event::ShellOutput(chunk));
                                }
                                let _ = events.send(Event::ShellOutput(decoder.push(&[], true)));
                                Ok(serde_json::json!({"exit_status":shell.wait().await?}))
                            }
                            .await;
                            result.map_err(|e| e.to_string())
                        } else {
                            invoke(client.as_mut().expect("connected"), &registry, policy, call).await
                        };
                        // After any failed exchange, drop the stream: its framing/cookie
                        // state may no longer be safe to reuse.
                        if result.is_err() {
                            client = None;
                            let _ = events.send(Event::Connection("disconnected (operation failed)".into()));
                        }
                        let _ = events.send(if is_shell {
                            Event::ShellCompleted(result)
                        } else {
                            Event::Completed(result)
                        });
                    }
                    Effect::Reconnect => {
                        let _ = events.send(Event::Completed(Ok(
                            serde_json::json!({"connection":"connected"}),
                        )));
                    }
                }
            }
        });
    });
    let result = terminal_loop(&mut app, &requests, &updates, logs);
    drop(requests);
    drop(updates); // Release a worker blocked on a full update queue after UI exit.
    // A running operation finishes and flushes its capture before process exit.
    let joined = worker
        .join()
        .map_err(|_| anyhow::anyhow!("camera worker panicked"));
    result.and(joined)
}

fn terminal_loop(
    app: &mut App,
    requests: &mpsc::Sender<Effect>,
    updates: &Receiver<Event>,
    logs: &Receiver<String>,
) -> Result<()> {
    let mut terminal = ratatui::try_init().context("initializing terminal")?;
    let result = (|| {
        loop {
            for event in updates.try_iter().take(64) {
                app.update(event);
            }
            for line in logs.try_iter() {
                app.update(Event::Log(line));
            }
            terminal.draw(|frame| view::draw(frame, app))?;
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            let event::Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                break;
            }
            let action = match key.code {
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Event::Reconnect,
                KeyCode::Up => Event::Up,
                KeyCode::Down => Event::Down,
                KeyCode::Tab | KeyCode::BackTab => Event::Tab,
                KeyCode::Enter => Event::Run,
                KeyCode::Esc => Event::Escape,
                KeyCode::Backspace => Event::Backspace,
                KeyCode::PageUp => Event::Scroll(-5),
                KeyCode::PageDown => Event::Scroll(5),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => Event::Text(c),
                _ => continue,
            };
            if let Some(effect) = app.update(action) {
                requests.send(effect).context("camera worker stopped")?;
            }
        }
        anyhow::Ok(())
    })();
    ratatui::restore();
    result
}

// Keep only an incomplete UTF-8 suffix between output notifications.
#[derive(Default)]
struct ShellDecoder {
    pending: Vec<u8>,
}
impl ShellDecoder {
    fn push(&mut self, bytes: &[u8], finish: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        let mut used = 0;
        while used < self.pending.len() {
            match std::str::from_utf8(&self.pending[used..]) {
                Ok(text) => {
                    output.push_str(text);
                    used = self.pending.len();
                }
                Err(e) => {
                    let end = used + e.valid_up_to();
                    output.push_str(std::str::from_utf8(&self.pending[used..end]).expect("valid prefix"));
                    used = end;
                    match e.error_len() {
                        Some(n) => {
                            output.push('�');
                            used += n;
                        }
                        None if finish => {
                            output.push('�');
                            used = self.pending.len();
                        }
                        None => break,
                    }
                }
            }
        }
        self.pending.drain(..used);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_queue_is_bounded_and_chunks_stay_incremental() {
        let (tx, rx) = event_channel();
        let chunk = "x".repeat(232);
        for _ in 0..64 {
            tx.try_send(Event::ShellOutput(chunk.clone())).unwrap();
        }
        assert!(matches!(
            tx.try_send(Event::ShellOutput(chunk.clone())),
            Err(mpsc::TrySendError::Full(_))
        ));
        let mut app = App::new(hblctl_core::registry::builtin(), Policy::default());
        for event in rx.try_iter() {
            app.update(event);
        }
        assert_eq!(app.result, chunk.repeat(64));
        tx.try_send(Event::ShellOutput("tail".into())).unwrap();
        app.update(rx.recv().unwrap());
        assert!(app.result.ends_with("tail"));
    }

    #[test]
    fn shell_chunks_decode_utf8_incrementally() {
        let mut decoder = ShellDecoder::default();
        assert_eq!(decoder.push(&[b'a', 0xc3], false), "a");
        assert_eq!(decoder.push(&[0xa9, 0xff, 0xe2], false), "é�");
        assert_eq!(decoder.push(&[], true), "�");
        assert!(decoder.pending.is_empty());
    }

    #[test]
    fn shell_output_events_append_only_the_new_chunk() {
        let mut app = App::new(hblctl_core::registry::builtin(), Policy::default());
        app.update(Event::ShellOutput("a".into()));
        app.update(Event::ShellOutput("é".into()));
        assert_eq!(app.result, "aé");
    }
    #[test]
    fn shell_completion_preserves_lines_and_errors() {
        let mut app = App::new(hblctl_core::registry::builtin(), Policy::default());
        app.update(Event::ShellOutput("one\ntwo\n".into()));
        app.update(Event::ShellCompleted(Ok(serde_json::json!({"exit_status":0}))));
        assert!(app.result.starts_with("one\ntwo\n"));
        app.update(Event::ShellCompleted(Err("timeout".into())));
        assert!(app.result.starts_with("one\ntwo\n"));
        assert!(app.result.contains("timeout"));
        app.update(Event::ShellOutput(
            "x".repeat(hblctl_core::shell::OUTPUT_LIMIT + 10),
        ));
        assert!(app.result.len() <= hblctl_core::shell::OUTPUT_LIMIT + 256);
    }
}
