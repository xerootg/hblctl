// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Shell CLI policy, streaming output, and remote status regression tests.

use hblctl_proto::{
    BusFrame, Sutest,
    bus::{BusHeader, NODE_EAGLE, NODE_TCP_HOST, SIGNAL_TEST_TX, TYPE_SUTEST},
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
};

#[test]
fn remote_failure_is_process_exit_status_and_output_streams() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 257];
        socket.read_exact(&mut request).unwrap();
        let (request, _) = BusFrame::decode(&request).unwrap();
        assert_eq!((request.record.cmd, request.record.func), (65, 1));
        for (func, text, status) in [
            (4, b"hello\n".as_slice(), 0),
            (4, b"".as_slice(), 0),
            (1, b"".as_slice(), 1),
        ] {
            let mut r = Sutest::new(65, func, 999);
            r.data[..4].copy_from_slice(&u32::to_le_bytes(status));
            r.data[4..4 + text.len()].copy_from_slice(text);
            socket
                .write_all(
                    &BusFrame {
                        header: BusHeader {
                            signal: SIGNAL_TEST_TX,
                            origin: NODE_EAGLE,
                            dest: NODE_TCP_HOST,
                            kind: TYPE_SUTEST,
                        },
                        record: r,
                    }
                    .encode_reply_message(),
                )
                .unwrap();
        }
    });
    let dir = tempfile::tempdir_in(".").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_hblctl"))
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--sessions",
            dir.path().to_str().unwrap(),
            "--allow",
            "destructive",
            "shell",
            "--",
            "exit 7",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"hello\n");
}
#[test]
fn policy_refusal_precedes_connection() {
    let output = Command::new(env!("CARGO_BIN_EXE_hblctl"))
        .args(["shell", "--", "true"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("destructive"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("connecting"));
}

#[test]
fn interactive_completion_does_not_wait_for_local_stdin() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = [0u8; 257];
        socket.read_exact(&mut request).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let record = Sutest::new(65, 1, 99);
        socket
            .write_all(
                &BusFrame {
                    header: BusHeader {
                        signal: SIGNAL_TEST_TX,
                        origin: NODE_EAGLE,
                        dest: NODE_TCP_HOST,
                        kind: TYPE_SUTEST,
                    },
                    record,
                }
                .encode_reply_message(),
            )
            .unwrap();
    });
    let dir = tempfile::tempdir_in(".").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_hblctl"))
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--sessions",
            dir.path().to_str().unwrap(),
            "--allow",
            "destructive",
            "shell",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let held_stdin = child.stdin.take().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    drop(held_stdin);
    server.join().unwrap();
    assert!(
        status.is_some_and(|s| s.success()),
        "shell waited for local input after remote completion"
    );
}
