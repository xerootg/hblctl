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

fn event_channel() -> (
    tokio::sync::mpsc::Sender<Event>,
    tokio::sync::mpsc::Receiver<Event>,
) {
    tokio::sync::mpsc::channel(64)
}

fn is_shell(registry: &Registry, command: &str) -> bool {
    registry.command(command).is_some_and(|c| c.id == 65)
}

async fn until_shutdown<T>(
    cancellable: bool,
    cancelled: impl std::future::Future<Output = Result<(), tokio::sync::oneshot::error::RecvError>>,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    if !cancellable {
        return Some(work.await);
    }
    tokio::select! {
        biased;
        _ = cancelled => None,
        result = work => Some(result),
    }
}

async fn stream_shell(
    client: &mut crate::camera::CameraClient,
    registry: &Registry,
    policy: Policy,
    call: &Call,
    events: &tokio::sync::mpsc::Sender<Event>,
) -> anyhow::Result<Value> {
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
    let mut shell = client.start_shell_with_delay(policy, command, delay).await?;

    let mut decoder = ShellDecoder::default();
    while let Some(bytes) = shell.next_output().await? {
        let chunk = decoder.push(&bytes, false);
        tokio::time::timeout_at(shell.deadline(), events.send(Event::ShellOutput(chunk)))
            .await
            .map_err(|_| anyhow::anyhow!("shell output delivery timed out"))?
            .map_err(|_| anyhow::anyhow!("UI closed"))?;
    }
    tokio::time::timeout_at(
        shell.deadline(),
        events.send(Event::ShellOutput(decoder.push(&[], true))),
    )
    .await
    .map_err(|_| anyhow::anyhow!("shell output delivery timed out"))?
    .map_err(|_| anyhow::anyhow!("UI closed"))?;
    Ok(serde_json::json!({"exit_status":shell.wait().await?}))
}

async fn camera_worker(
    camera: CameraSession,
    registry: Registry,
    policy: Policy,
    mut work: tokio::sync::mpsc::UnboundedReceiver<Effect>,
    events: tokio::sync::mpsc::Sender<Event>,
    mut cancelled: tokio::sync::oneshot::Receiver<()>,
) {
    let mut client = None;
    loop {
        let effect = tokio::select! {
            biased;
            _ = &mut cancelled => return,
            effect = work.recv() => match effect { Some(effect) => effect, None => return },
        };
        if matches!(effect, Effect::Reconnect) {
            client = None;
        } // Drop the old socket before opening another.
        if client.is_none() {
            if events.send(Event::Connection("connecting".into())).await.is_err() {
                return;
            }
            match camera.connect().await {
                Ok(c) => {
                    client = Some(c);
                    if events.send(Event::Connection("connected".into())).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    if events
                        .send(Event::Connection("disconnected".into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if events
                        .send(Event::Completed(Err(format!("{e:#}"))))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
            }
        }
        match effect {
            Effect::Invoke(call) => {
                let is_shell = is_shell(&registry, &call.command);
                let result = until_shutdown(is_shell, &mut cancelled, async {
                    if is_shell {
                        stream_shell(
                            client.as_mut().expect("connected"),
                            &registry,
                            policy,
                            &call,
                            &events,
                        )
                        .await
                        .map_err(|e| e.to_string())
                    } else {
                        invoke(client.as_mut().expect("connected"), &registry, policy, call).await
                    }
                })
                .await;
                let Some(result) = result else {
                    return;
                };
                tracing::info!(target: "hblctl::op", shell = is_shell, result = ?result, "TUI operation completed");
                // After any failed exchange, drop the stream: its framing/cookie
                // state may no longer be safe to reuse.
                if result.is_err() {
                    client = None;
                    if events
                        .send(Event::Connection("disconnected (operation failed)".into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                if events
                    .send(if is_shell {
                        Event::ShellCompleted(result)
                    } else {
                        Event::Completed(result)
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Effect::Reconnect => {
                if events
                    .send(Event::Completed(Ok(
                        serde_json::json!({"connection":"connected"}),
                    )))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

pub(crate) fn run(
    camera: CameraSession,
    registry: Registry,
    policy: Policy,
    logs: &Receiver<String>,
) -> Result<()> {
    let mut app = App::new(registry.clone(), policy);
    let (requests, work) = tokio::sync::mpsc::unbounded_channel();
    let (shutdown, cancelled) = tokio::sync::oneshot::channel::<()>();
    let (finished, done) = mpsc::channel();
    let (events, mut updates) = event_channel();
    let runtime = tokio::runtime::Runtime::new()?;
    let worker = std::thread::spawn(move || {
        runtime.block_on(async move {
            camera_worker(camera, registry, policy, work, events, cancelled).await;
        });
        runtime.shutdown_timeout(Duration::from_secs(1));
        let _ = finished.send(());
    });
    let result = terminal_loop(&mut app, &requests, &mut updates, logs);
    let _ = shutdown.send(());
    drop(requests);
    drop(updates);
    done.recv().context("camera worker did not stop")?;
    let joined = worker
        .join()
        .map_err(|_| anyhow::anyhow!("camera worker panicked"));
    result.and(joined)
}

fn terminal_loop(
    app: &mut App,
    requests: &tokio::sync::mpsc::UnboundedSender<Effect>,
    updates: &mut tokio::sync::mpsc::Receiver<Event>,
    logs: &Receiver<String>,
) -> Result<()> {
    let mut terminal = ratatui::try_init().context("initializing terminal")?;
    let result = (|| {
        loop {
            let mut received_output = false;
            while let Ok(event) = updates.try_recv() {
                received_output = true;
                app.update(event);
            }
            for line in logs.try_iter() {
                app.update(Event::Log(line));
            }
            terminal.draw(|frame| view::draw(frame, app))?;
            if !event::poll(Duration::from_millis(if received_output { 0 } else { 50 }))? {
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

    #[tokio::test]
    async fn shutdown_waits_for_ordinary_operation() {
        let (shutdown, cancelled) = tokio::sync::oneshot::channel();
        let (finished, observed) = tokio::sync::oneshot::channel();
        shutdown.send(()).unwrap();
        until_shutdown(false, cancelled, async move {
            tokio::task::yield_now().await;
            finished.send(()).unwrap();
        })
        .await;
        observed.await.unwrap();
    }

    #[test]
    fn scroll_reaches_completion_after_seventy_thousand_lines() {
        let mut app = App::new(hblctl_core::registry::builtin(), Policy::default());
        app.update(Event::ShellOutput("x\n".repeat(70_000)));
        app.update(Event::ShellCompleted(Ok(serde_json::json!({"exit_status":0}))));
        for _ in 0..14_100 {
            app.update(Event::Scroll(5));
        }
        assert!(app.scroll > 65_535);
        assert!(view::result_window(&app.result, app.scroll).contains("exit_status: 0"));
        let wrapped = format!("{}\nexit_status: 0\n", "x".repeat(70_000));
        assert_eq!(view::result_window(&wrapped, usize::MAX), "exit_status: 0\n");
    }

    #[tokio::test]
    async fn shutdown_drops_quiet_work_promptly() {
        let (shutdown, cancelled) = tokio::sync::oneshot::channel();
        let (dropped, observed) = tokio::sync::oneshot::channel::<()>();
        let worker = tokio::spawn(until_shutdown(true, cancelled, async move {
            let _guard = dropped;
            std::future::pending::<()>().await;
        }));
        tokio::task::yield_now().await;
        shutdown.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(observed.await.is_err());
    }

    #[tokio::test]
    async fn full_capture_delivers_within_default_deadline() {
        let (tx, mut rx) = event_channel();
        let producer = async move {
            for _ in 0..hblctl_core::shell::OUTPUT_LIMIT.div_ceil(232) {
                tx.send(Event::ShellOutput("x".repeat(232))).await.unwrap();
            }
        };
        let consumer = async move {
            let mut bytes = 0;
            while let Some(event) = rx.recv().await {
                if let Event::ShellOutput(chunk) = event {
                    bytes += chunk.len();
                }
            }
            assert!(bytes >= hblctl_core::shell::OUTPUT_LIMIT);
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(producer, consumer);
        })
        .await
        .unwrap();
    }

    #[test]
    fn shell_routing_uses_wire_id() {
        let mut registry = hblctl_core::registry::builtin();
        let shell = registry.commands.iter_mut().find(|c| c.id == 65).unwrap();
        shell.name = "renamed-shell".into();
        assert!(is_shell(&registry, "renamed-shell"));
        let other = registry.commands.iter_mut().find(|c| c.id != 65).unwrap();
        other.name = "hbl-shell".into();
        assert!(!is_shell(&registry, "hbl-shell"));
        assert!(is_shell(&registry, "65"));
    }

    #[tokio::test]
    async fn full_output_queue_observes_deadline_and_closed_receiver() {
        let (tx, rx) = event_channel();
        for _ in 0..64 {
            tx.try_send(Event::ShellOutput("x".into())).unwrap();
        }
        assert!(
            tokio::time::timeout(
                Duration::from_millis(10),
                tx.send(Event::ShellOutput("tail".into()))
            )
            .await
            .is_err()
        );
        drop(rx);
        assert!(tx.send(Event::ShellOutput("tail".into())).await.is_err());
    }

    #[test]
    fn invocation_resets_previous_scroll() {
        let mut app = App::new(hblctl_core::registry::builtin(), Policy::default());
        app.scroll = 200;
        assert!(app.update(Event::Run).is_some());
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn output_queue_is_bounded_and_chunks_stay_incremental() {
        let (tx, mut rx) = event_channel();
        let chunk = "x".repeat(232);
        for _ in 0..64 {
            tx.try_send(Event::ShellOutput(chunk.clone())).unwrap();
        }
        assert!(matches!(
            tx.try_send(Event::ShellOutput(chunk.clone())),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));
        let mut app = App::new(hblctl_core::registry::builtin(), Policy::default());
        while let Ok(event) = rx.try_recv() {
            app.update(event);
        }
        assert_eq!(app.result, chunk.repeat(64));
        tx.try_send(Event::ShellOutput("tail".into())).unwrap();
        app.update(rx.try_recv().unwrap());
        assert!(app.result.ends_with("tail"));
    }

    #[test]
    fn shell_chunks_decode_utf8_incrementally() {
        let mut decoder = ShellDecoder::default();
        assert_eq!(decoder.push(&[b'a', 0xc3], false), "a");
        assert_eq!(decoder.push(&[0xa9, 0xff, 0xe2], false), "é�");
        assert_eq!(decoder.push(&[], true), "�");
        assert_eq!(decoder.pending.len(), 0);
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
