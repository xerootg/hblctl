// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Shared session logging and connection setup for every camera frontend.
use crate::{logging, session};
use anyhow::{Context, Result};
use clap::ArgMatches;
use hblctl_core::Client;
use hblctl_transport::{capture::CaptureWriter, recording::Recording, tcp::TcpTransport};
use std::{
    fs::File,
    io::{BufWriter, Write},
    sync::{Arc, Mutex, mpsc::Sender},
    time::Duration,
};

type Capture = Arc<Mutex<CaptureWriter<BufWriter<File>>>>;
pub(crate) type CameraClient = Client<Recording<TcpTransport, BufWriter<File>>>;

pub(crate) struct CameraSession {
    host: String,
    port: u16,
    timeout: Duration,
    capture: Capture,
}

impl CameraSession {
    pub(crate) fn open(m: &ArgMatches, argv: &[String], logs: Option<Sender<String>>) -> Result<Self> {
        let session = session::Session::create(&crate::sessions_root(m), argv)?;
        let file = File::create(session.events_path()).context("creating events.jsonl")?;
        let tui = logs.is_some();
        let writer = EventFile {
            file,
            logs,
            pending: Vec::new(),
        };
        let console = if tui {
            "off"
        } else {
            match m.get_count("verbose") {
                0 => "warn",
                1 => "info",
                2 => "debug",
                _ => "trace",
            }
        };
        tracing::subscriber::set_global_default(logging::subscriber(
            logging::Shared(Arc::new(Mutex::new(writer))),
            console,
        ))
        .context("installing logger")?;
        tracing::info!(target: "hblctl::session", dir = %session.dir.display(), "session started");
        eprintln!("session: {}", session.dir.display());
        Ok(Self {
            host: m.get_one::<String>("host").expect("default host").clone(),
            port: *m.get_one::<u16>("port").expect("default port"),
            timeout: Duration::from_secs(*m.get_one::<u64>("timeout").expect("default timeout")),
            capture: Arc::new(Mutex::new(session.capture()?)),
        })
    }

    pub(crate) async fn connect(&self) -> Result<CameraClient> {
        let tcp = TcpTransport::connect(
            &format!("{}:{}", self.host, self.port),
            self.timeout,
            self.timeout,
        )
        .await
        .with_context(|| format!("connecting to {}:{}", self.host, self.port))?;
        Ok(Client::new(
            Recording::with_shared_capture(tcp, self.capture.clone()),
            self.timeout,
            crate::initial_cookie(),
        ))
    }
}

// The JSON layer writes in chunks. Mirror complete protocol events into the UI,
// while preserving every event (including wire hex) in the session file.
struct EventFile {
    file: File,
    logs: Option<Sender<String>>,
    pending: Vec<u8>,
}

impl Write for EventFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(bytes)?;
        if let Some(tx) = &self.logs {
            self.pending.extend_from_slice(bytes);
            while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let line = self.pending.drain(..=end).collect::<Vec<_>>();
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&line)
                    && (v["target"] == "hblctl::proto"
                        || v["target"] == "hblctl::op"
                        || v["target"] == "hblctl::transport")
                {
                    let _ = tx.send(format!(
                        "{} {} {}",
                        v["level"].as_str().unwrap_or(""),
                        v["fields"]["message"].as_str().unwrap_or(""),
                        v["fields"]
                    ));
                }
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_file_mirrors_complete_protocol_lines_and_keeps_all_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let (tx, rx) = std::sync::mpsc::channel();
        let mut writer = EventFile {
            file: File::create(&path).unwrap(),
            logs: Some(tx),
            pending: vec![],
        };
        let text = "{\"target\":\"hblctl::proto\",\"level\":\"DEBUG\",\"fields\":{\"message\":\"send\"}}\n{\"target\":\"hblctl::wire\",\"fields\":{}}\n";
        writer.write_all(&text.as_bytes()[..12]).unwrap();
        assert!(rx.try_recv().is_err());
        writer.write_all(&text.as_bytes()[12..]).unwrap();
        assert!(rx.try_recv().unwrap().contains("send"));
        assert!(rx.try_recv().is_err());
        writer.flush().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
    }
}
