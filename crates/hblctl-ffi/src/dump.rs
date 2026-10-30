// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! `hblctl capture dump`: a protocol-level reading of a binary capture.
//!
//! Each captured chunk is listed with its time, direction and size; the bytes are fed through a separate
//! framer per direction, so frames split across chunks are reassembled exactly as the client saw them.

use std::fmt::Write as _;

use hblctl_core::client::Status;
use hblctl_transport::capture::{Direction, Record};
use hblctl_transport::framer::{Event, Framer};

/// Render records as text lines. With `hex`, each chunk's bytes are printed too.
#[must_use]
pub(super) fn render(records: &[Record], hex: bool) -> Vec<String> {
    let mut tx = Framer::requests();
    let mut rx = Framer::replies();
    let mut out = Vec::new();
    for r in records {
        #[allow(clippy::cast_precision_loss)] // display only
        let t = r.t_ns as f64 / 1e9;
        let (arrow, framer) = match r.direction {
            Direction::Sent => ("->", &mut tx),
            Direction::Received => ("<-", &mut rx),
        };
        let mut line = format!("{t:>12.6}s {arrow} {:>5} bytes", r.bytes.len());
        if hex {
            let _ = write!(line, "  {}", hblctl_core::hex(&r.bytes));
        }
        out.push(line);
        for ev in framer.push(&r.bytes) {
            out.push(match ev {
                Event::Frame(f) => {
                    let s = &f.record;
                    let status = if r.direction == Direction::Received {
                        format!(" status={:?}", Status::of(s))
                    } else {
                        String::new()
                    };
                    format!(
                        "               frame signal=0x{:02x} {}->{} cmd={} func={} cookie=0x{:08x}{status}",
                        f.header.signal, f.header.origin, f.header.dest, s.cmd, s.func, s.cookie
                    )
                }
                Event::Skipped { byte, reason } => format!("               skipped 0x{byte:02x}: {reason}"),
            });
        }
    }
    for (name, f) in [("sent", &tx), ("received", &rx)] {
        if f.pending() > 0 {
            out.push(format!(
                "               {} {name} bytes left over (incomplete frame)",
                f.pending()
            ));
        }
    }
    out
}
