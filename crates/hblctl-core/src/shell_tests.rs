// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later
use super::*;
use hblctl_proto::bus::{BusHeader, NODE_EAGLE, NODE_TCP_HOST, SIGNAL_TEST_TX, TYPE_SUTEST};
use hblctl_transport::mock::{MockTransport, Step};
use std::time::Duration;
fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
#[tokio::test]
async fn exact_example_split_reads() {
    let v: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/x2d-4.2.0-shell-pass4-example.json"
    ))
    .unwrap();
    let mut wire = Vec::new();
    for r in v["one_valid_reply_sequence"].as_array().unwrap() {
        let mut b = unhex(r["known_tcp_prefix_260_bytes_hex"].as_str().unwrap());
        b.resize(1024, 0xa5);
        wire.extend(b);
    }
    let mut steps = vec![Step::Expect(unhex(v["request_tcp_hex"].as_str().unwrap()))];
    steps.extend(wire.chunks(137).map(|b| Step::Reply(b.to_vec())));
    let mut c = Client::new(MockTransport::new(steps), Duration::from_secs(1), 0x1234_5678);
    let mut s = c
        .start_shell(
            Policy::up_to(Safety::Destructive),
            "sha256sum /system/bin/camera-test",
        )
        .await
        .unwrap();
    let mut output = Vec::new();
    while let Some(b) = s.next_output().await.unwrap() {
        output.extend(b);
    }
    assert_eq!(output, v["expected_stdout_ascii"].as_str().unwrap().as_bytes());
    assert_eq!(s.wait().await.unwrap(), 0);
    drop(s);
    assert!(c.transport().finished());
}
#[tokio::test]
async fn policy_refuses_before_send() {
    let mut c = Client::new(MockTransport::new([]), Duration::from_secs(1), 1);
    assert!(c.start_shell(Policy::default(), "").await.is_err());
    assert!(c.transport().finished());
}

fn req(func: u32, cookie: u32, text: &[u8]) -> Vec<u8> {
    let mut r = hblctl_proto::Sutest::new(65, func, cookie);
    r.data[4..4 + text.len()].copy_from_slice(text);
    hblctl_proto::BusFrame::command(r).encode()
}
fn reply(func: u32, cookie: u32, text: &[u8], status: u32) -> Vec<u8> {
    let mut r = hblctl_proto::Sutest::new(65, func, cookie);
    r.data[..4].copy_from_slice(&status.to_le_bytes());
    r.data[4..4 + text.len()].copy_from_slice(text);
    hblctl_proto::BusFrame {
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
#[tokio::test]
async fn stdin_uploads_and_etx_are_refused_without_any_record() {
    let mut c = Client::new(
        MockTransport::new([Step::Expect(req(1, 1, b"cat")), Step::Reply(reply(1, 9, b"", 0))]),
        Duration::from_secs(1),
        1,
    );
    let mut s = c
        .start_shell(Policy::up_to(Safety::Destructive), "cat")
        .await
        .unwrap();
    for text in [
        "exit\nreboot\n".to_owned(),
        format!("{}\x03", "x".repeat(231)),
        "x\0".to_owned(),
    ] {
        assert!(matches!(
            s.write_stdin(&text).await,
            Err(client::ClientError::ShellInput(_))
        ));
    }
    assert_eq!(s.wait().await.unwrap(), 0);
    assert!(s.write_stdin("reboot\n").await.is_err());
    assert!(s.interrupt().await.is_err());
    assert!(s.kill().await.is_err());
    drop(s);
    assert!(c.transport().finished());
}
#[tokio::test]
async fn unfinished_shell_blocks_client_and_rejects_invalid_input() {
    let mut c = Client::new(
        MockTransport::new([Step::Expect(req(1, 1, b""))]),
        Duration::from_secs(1),
        1,
    );
    assert!(
        c.start_shell(Policy::up_to(Safety::Destructive), &"a".repeat(232))
            .await
            .is_err()
    );
    let s = c
        .start_shell(Policy::up_to(Safety::Destructive), "")
        .await
        .unwrap();
    drop(s);
    assert!(matches!(
        c.call(52, 1, &[]).await,
        Err(client::ClientError::ShellActive)
    ));
}

#[tokio::test]
async fn thin_process_operations_use_verified_layouts() {
    use serde_json::json;
    let registry = registry::builtin();
    for (id, name, func, args, bytes) in [
        (
            52,
            "os-system-command",
            "run-path",
            json!({"path_utf8_z":"/bin/true"}),
            b"/bin/true\0".to_vec(),
        ),
        (
            73,
            "run-script",
            "run-staged-script",
            json!({"output_limit_bytes":-1}),
            (-1i32).to_le_bytes().to_vec(),
        ),
    ] {
        let mut request = hblctl_proto::Sutest::new(id, 1, 1);
        request.data[4..4 + bytes.len()].copy_from_slice(&bytes);
        let mut response = hblctl_proto::Sutest::new(id, 1, 1);
        if id == 52 {
            response.data[4..].fill(255);
        }

        let wire = hblctl_proto::BusFrame {
            header: BusHeader {
                signal: SIGNAL_TEST_TX,
                origin: NODE_EAGLE,
                dest: NODE_TCP_HOST,
                kind: TYPE_SUTEST,
            },
            record: response,
        }
        .encode_reply_message();
        let mut client = Client::new(
            MockTransport::new([
                Step::Expect(hblctl_proto::BusFrame::command(request).encode()),
                Step::Reply(wire),
            ]),
            Duration::from_secs(1),
            1,
        );
        let out = dispatch::invoke(
            &mut client,
            &registry,
            Policy::up_to(Safety::Destructive),
            name,
            func,
            args.as_object().unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(out["verified_layout"], true);
        assert_eq!(out["status"], "ok");
    }
}

#[tokio::test]
async fn full_notification_and_final_drain_are_preserved() {
    let t = MockTransport::new([
        Step::Expect(req(1, 1, b"true")),
        Step::Reply(
            [
                reply(4, 2, &[b'x'; 232], 0),
                reply(4, 3, b"tail", 0),
                reply(1, 4, b"", 0),
            ]
            .concat(),
        ),
    ]);
    let mut c = Client::new(t, Duration::from_secs(1), 1);
    let mut s = c
        .start_shell(Policy::up_to(Safety::Destructive), "true")
        .await
        .unwrap();
    assert_eq!(s.next_output().await.unwrap().unwrap(), vec![b'x'; 232]);
    assert_eq!(s.next_output().await.unwrap().unwrap(), b"tail");
    assert_eq!(s.wait().await.unwrap(), 0);
}

#[tokio::test]
async fn completion_timeout_leaves_connection_blocked() {
    struct Silent;
    impl hblctl_transport::Transport for Silent {
        fn send(
            &mut self,
            _: &[u8],
        ) -> impl std::future::Future<Output = Result<(), hblctl_transport::Error>> + Send {
            std::future::ready(Ok(()))
        }
        async fn recv(&mut self) -> Result<Option<Vec<u8>>, hblctl_transport::Error> {
            std::future::pending().await
        }
    }
    let mut c = Client::new(Silent, Duration::from_millis(1), 1);
    let mut s = c
        .start_shell(Policy::up_to(Safety::Destructive), "")
        .await
        .unwrap();
    assert!(matches!(s.wait().await, Err(client::ClientError::Timeout { .. })));
    drop(s);
    assert!(matches!(
        c.call(52, 1, &[]).await,
        Err(client::ClientError::ShellActive)
    ));
}

#[tokio::test]
async fn client_can_be_reused_only_after_completion() {
    use hblctl_proto::{BusFrame, Sutest};
    let next = Sutest::new(30, 0, 2);
    let response = BusFrame {
        header: BusHeader {
            signal: SIGNAL_TEST_TX,
            origin: NODE_EAGLE,
            dest: NODE_TCP_HOST,
            kind: TYPE_SUTEST,
        },
        record: next.clone(),
    }
    .encode_reply_message();
    let mut c = Client::new(
        MockTransport::new([
            Step::Expect(req(1, 1, b"true")),
            Step::Reply(reply(1, 55, b"", 0)),
            Step::Expect(BusFrame::command(next).encode()),
            Step::Reply(response),
        ]),
        Duration::from_secs(1),
        1,
    );
    {
        let mut s = c
            .start_shell(Policy::up_to(Safety::Destructive), "true")
            .await
            .unwrap();
        assert_eq!(s.wait().await.unwrap(), 0);
    }
    assert_eq!(c.call(30, 0, &[]).await.unwrap().status, Status::Ok);
    assert!(c.transport().finished());
}

#[tokio::test]
async fn dispatcher_refuses_empty_shell_before_send() {
    let mut c = Client::new(MockTransport::new([]), Duration::from_secs(1), 1);
    let result = dispatch::invoke(
        &mut c,
        &registry::builtin(),
        Policy::up_to(Safety::Destructive),
        "hbl-shell",
        "start-shell",
        serde_json::json!({"command_utf8_z":""}).as_object().unwrap(),
    )
    .await;
    assert!(matches!(
        result,
        Err(dispatch::DispatchError::Client(client::ClientError::ShellInput(
            _
        )))
    ));
    assert!(c.transport().finished());
}

#[tokio::test]
async fn short_script_is_one_complete_initial_record_with_no_eof_injection() {
    let script = "exit\nreboot\n";
    let mut c = Client::new(
        MockTransport::new([
            Step::Expect(req(1, 1, script.as_bytes())),
            Step::Reply(reply(1, 77, b"", 0)),
        ]),
        Duration::from_secs(1),
        1,
    );
    let mut shell = c
        .start_shell(Policy::up_to(Safety::Destructive), script)
        .await
        .unwrap();
    assert_eq!(shell.wait().await.unwrap(), 0);
    drop(shell);
    assert!(c.transport().finished());
}

#[tokio::test]
async fn explicit_signals_keep_byte_exact_records() {
    let mut c = Client::new(
        MockTransport::new([
            Step::Expect(req(1, 1, b"sleep 60")),
            Step::Expect(req(1, 2, &[3])),
            Step::Expect(req(5, 3, b"")),
            Step::Reply(reply(1, 100, b"", 1)),
        ]),
        Duration::from_secs(1),
        1,
    );
    let mut shell = c
        .start_shell(Policy::up_to(Safety::Destructive), "sleep 60")
        .await
        .unwrap();
    shell.interrupt().await.unwrap();
    shell.kill().await.unwrap();
    assert_eq!(shell.wait().await.unwrap(), 1);
    drop(shell);
    assert!(c.transport().finished());
}
