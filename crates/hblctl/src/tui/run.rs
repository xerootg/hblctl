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

pub(crate) fn run(
    camera: CameraSession,
    registry: Registry,
    policy: Policy,
    logs: &Receiver<String>,
) -> Result<()> {
    let mut app = App::new(registry.clone(), policy);
    let (requests, work) = mpsc::channel();
    let (events, updates) = mpsc::channel();
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
                        let result = if call.command == "hbl-shell" {
                            let result: anyhow::Result<Value> = async {
                                let command = call.args.get("command_utf8_z").and_then(Value::as_str).unwrap_or("");
                                let spec = registry.command(&call.command).and_then(|c| c.func(&call.func)).ok_or_else(|| anyhow::anyhow!("unknown shell operation"))?;
                                let data = hblctl_core::codec::encode(spec, &call.args)?;
                                let delay = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                                let mut shell = client.as_mut().expect("connected").start_shell_with_delay(policy, command, delay).await?;
                                let mut output = Vec::new();
                                while let Some(bytes) = shell.next_output().await? {
                                    output.extend(bytes);
                                    let _ = events.send(Event::ShellOutput(String::from_utf8_lossy(&output).into_owned()));
                                }
                                Ok(serde_json::json!({"output":String::from_utf8_lossy(&output),"exit_status":shell.wait().await?}))
                            }.await;
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
                        let _ = events.send(Event::Completed(result));
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
            for event in updates.try_iter() {
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
