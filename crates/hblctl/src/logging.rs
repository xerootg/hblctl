// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Logging: a complete JSON event log for the session file and a filtered human log on stderr.
//!
//! The file layer records every `hblctl*` event at TRACE (wire hex, decoded frames, results, skipped bytes)
//! regardless of the console verbosity, so a quiet run still leaves everything needed to diagnose it.

use std::io::Write;
use std::sync::{Arc, Mutex};

use tracing::Subscriber;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

/// Filter for the session file: all of hblctl at TRACE, everything else at INFO.
pub(crate) const FILE_FILTER: &str = "info,hblctl=trace,hblctl_core=trace,hblctl_transport=trace";

/// A `MakeWriter` over a shared writer, so one file can back the JSON layer.
#[derive(Clone)]
pub(crate) struct Shared<W>(pub(crate) Arc<Mutex<W>>);

/// Writer handle returned by [`Shared`].
pub(crate) struct SharedGuard<W>(Arc<Mutex<W>>);

impl<W: Write> Write for SharedGuard<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log writer poisoned"))?
            .write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log writer poisoned"))?
            .flush()
    }
}

impl<'a, W: Write + 'a> MakeWriter<'a> for Shared<W> {
    type Writer = SharedGuard<W>;
    fn make_writer(&'a self) -> Self::Writer {
        SharedGuard(self.0.clone())
    }
}

/// Build the subscriber: JSON to `file`, human-readable to stderr filtered by `console`
/// (an `EnvFilter` directive such as `"warn"` or `"hblctl=debug"`).
pub(crate) fn subscriber<W: Write + Send + 'static>(
    file: Shared<W>,
    console: &str,
) -> impl Subscriber + Send + Sync {
    let file_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(false)
        .with_target(true)
        .with_writer(file)
        .with_filter(EnvFilter::new(FILE_FILTER));
    let console_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_filter(EnvFilter::new(console));
    tracing_subscriber::registry()
        .with(file_layer)
        .with(console_layer)
}

use tracing_subscriber::Layer as _;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_layer_records_trace_events_as_json() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sub = subscriber(Shared(buf.clone()), "off");
        tracing::subscriber::with_default(sub, || {
            tracing::trace!(target: "hblctl::wire", dir = "tx", hex = "0500", "bytes");
            tracing::debug!(target: "other_crate", "filtered out");
        });
        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 1, "{text}");
        assert_eq!(lines[0]["target"], "hblctl::wire");
        assert_eq!(lines[0]["fields"]["hex"], "0500");
        assert_eq!(lines[0]["level"], "TRACE");
    }
}
