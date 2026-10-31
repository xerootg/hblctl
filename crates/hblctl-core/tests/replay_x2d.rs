// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Replays sessions recorded against a real camera. Each fixture is a `capture.hblcap` from a session
//! directory, so a run that misbehaves in the field can be dropped in here and becomes a regression test.

use std::time::Duration;

use hblctl_core::dispatch::invoke;
use hblctl_core::registry;
use hblctl_core::safety::Policy;
use hblctl_core::{Client, Status};
use hblctl_transport::capture::{CaptureReader, Direction};
use hblctl_transport::replay::ReplayTransport;
use serde_json::{Map, json};

/// First real exchange with an original X2D 100C on firmware 4.2.0 (2026-10-31, over the USB network link):
/// `firmware-version build-version` answered with the DJI build string.
const X2D_FIRMWARE_VERSION: &[u8] = include_bytes!("fixtures/x2d-4.2.0-firmware-version.hblcap");

fn cookie_of_first_request(records: &[hblctl_transport::capture::Record]) -> u32 {
    let req = records
        .iter()
        .find(|r| r.direction == Direction::Sent)
        .expect("a sent frame");
    // bus header (5) then the record: cmd, func, cookie
    u32::from_le_bytes(req.bytes[5 + 8..5 + 12].try_into().unwrap())
}

#[tokio::test]
async fn x2d_4_2_0_firmware_version_replays_byte_exact() {
    let records = CaptureReader::new(X2D_FIRMWARE_VERSION)
        .unwrap()
        .records()
        .unwrap();
    assert_eq!(records.len(), 2, "one request, one reply");
    assert_eq!(records[0].bytes.len(), 257, "request is one bare frame");
    assert_eq!(
        records[1].bytes.len(),
        1024,
        "the camera answers with a 1024-byte message"
    );
    let cookie = cookie_of_first_request(&records);

    let transport = ReplayTransport::new(records, true);
    let mut client = Client::new(transport, Duration::from_secs(1), cookie);
    let reg = registry::builtin();
    let out = invoke(
        &mut client,
        &reg,
        Policy::default(),
        "firmware-version",
        "build-version",
        &Map::new(),
    )
    .await
    .unwrap();
    assert_eq!(out["status"], json!("ok"));
    assert_eq!(out["fields"]["unicode_codepoints"], json!("10.00.25.21"));
    assert_eq!(out["stray_frames"], json!(0));
    assert!(client.transport().finished(), "every recorded byte was consumed");
}

#[test]
fn x2d_reply_status_word_is_ok() {
    let records = CaptureReader::new(X2D_FIRMWARE_VERSION)
        .unwrap()
        .records()
        .unwrap();
    let reply = &records[1].bytes;
    assert_eq!(
        &reply[..5],
        &[0x09, 0x00, 0x05, 0x09, 0xfc],
        "signal 9 from node 5 to node 9, type 0xfc"
    );
    let record = hblctl_proto::Sutest::decode(&reply[5..5 + 252]).unwrap();
    assert_eq!((record.cmd, record.func), (30, 0));
    assert_eq!(Status::of(&record), Status::Ok);
}
