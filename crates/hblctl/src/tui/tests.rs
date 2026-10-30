// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

use super::app::{App, Effect, Event, Focus};
use hblctl_core::registry::Registry;
use hblctl_core::safety::{Policy, Safety};
use serde_json::json;

fn registry() -> Registry {
    Registry::from_json(
        r#"{"target":"test","commands":[{"id":1,"name":"camera","funcs":[
        {"func":0,"name":"read","safety":"read","verified":true,"request":[
            {"name":"number","offset":20,"type":"u8","default":3},
            {"name":"text","offset":24,"type":"cstr","len":8}]},
        {"func":1,"name":"write","safety":"persistent"},
        {"func":2,"name":"erase","safety":"destructive"}]}]}"#,
    )
    .unwrap()
}

#[test]
fn schema_form_editing_validation_and_navigation() {
    let mut a = App::new(registry(), Policy::default());
    assert_eq!(a.fields[0].value, "3");
    assert_eq!(a.fields[0].schema["maximum"], 255);
    assert!(a.fields[1].required);
    assert!(a.update(Event::Run).is_none());
    assert!(a.result.contains("text"));
    a.update(Event::Tab);
    assert_eq!(a.focus, Focus::Form);
    a.update(Event::Down);
    a.update(Event::Text('x'));
    assert!(matches!(a.update(Event::Run), Some(Effect::Invoke(_))));
    assert!(a.busy);
    assert!(a.update(Event::Run).is_none());
    a.update(Event::Completed(Ok(json!({"status":"ok"}))));
    assert!(a.result.contains("\"status\": \"ok\""));
    a.update(Event::Completed(Err("closed".into())));
    assert!(a.result.contains("closed"));
    a.fields[0].value = "0xff".into();
    assert!(matches!(a.update(Event::Run), Some(Effect::Invoke(_))));
    a.update(Event::Completed(Ok(json!({}))));
    a.fields[0].value = "256".into();
    assert!(a.update(Event::Run).is_none());
    assert!(a.result.contains("outside"));
    a.update(Event::Tab);
    a.update(Event::Down);
    assert_eq!(a.selected, 1);
    assert!(a.update(Event::Run).is_none());
    assert!(a.result.contains("current limit"));
    a.update(Event::Text('.'));
    a.update(Event::Text('e'));
    a.update(Event::Text('r'));
    a.update(Event::Text('a'));
    assert_eq!(a.visible().len(), 1);
    assert_eq!(a.operation().unwrap().name, "camera.erase");
    a.update(Event::Backspace);
    a.update(Event::Escape);
    assert_eq!(a.visible().len(), 3);
    a.update(Event::Up);
    assert_eq!(a.selected, 0);
}

#[test]
fn destructive_requires_exact_name_and_can_cancel() {
    let mut a = App::new(registry(), Policy::up_to(Safety::Destructive));
    a.update(Event::Down);
    a.update(Event::Down);
    assert!(a.update(Event::Run).is_none());
    assert_eq!(a.confirmation.as_deref(), Some(""));
    a.update(Event::Text('x'));
    assert!(a.update(Event::Run).is_none());
    a.update(Event::Escape);
    assert!(a.confirmation.is_none());
    a.update(Event::Run);
    for c in "camera.erase".chars() {
        a.update(Event::Text(c));
    }
    assert!(matches!(a.update(Event::Run), Some(Effect::Invoke(_))));
    a.update(Event::Completed(Ok(json!({}))));
    assert!(matches!(a.update(Event::Reconnect), Some(Effect::Reconnect)));
    a.update(Event::Connection("connected".into()));
    assert_eq!(a.connection, "connected");
}

#[test]
fn renders_main_and_confirmation() {
    use ratatui::{Terminal, backend::TestBackend};
    let mut a = App::new(registry(), Policy::up_to(Safety::Destructive));
    let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
    t.draw(|f| super::view::draw(f, &a)).unwrap();
    let screen = |t: &Terminal<TestBackend>| {
        t.backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    };
    let s = screen(&t);
    for expected in [
        "camera",
        "read",
        "unverified layout",
        "number",
        "255",
        "Result",
        "Log",
        "disconnected",
    ] {
        assert!(s.contains(expected), "missing {expected}: {s}");
    }
    a.update(Event::Down);
    a.update(Event::Down);
    a.update(Event::Run);
    t.draw(|f| super::view::draw(f, &a)).unwrap();
    assert!(screen(&t).contains("camera.erase"));
    assert!(screen(&t).contains("Confirm destructive operation"));
}

#[tokio::test]
async fn effects_run_through_mock_transport() {
    use hblctl_core::Client;
    use hblctl_proto::bus::{BusHeader, NODE_EAGLE, NODE_TCP_HOST, SIGNAL_TEST_TX, TYPE_SUTEST};
    use hblctl_proto::{BusFrame, Sutest};
    use hblctl_transport::mock::{MockTransport, Step};
    let mut a = App::new(registry(), Policy::default());
    a.fields[1].value = "x".into();
    let Some(Effect::Invoke(call)) = a.update(Event::Run) else {
        panic!("invoke");
    };
    let mut req = Sutest::new(1, 0, 1);
    req.put(20, &[3]).unwrap();
    req.put(24, b"x\0").unwrap();
    let reply = BusFrame {
        header: BusHeader {
            signal: SIGNAL_TEST_TX,
            origin: NODE_EAGLE,
            dest: NODE_TCP_HOST,
            kind: TYPE_SUTEST,
        },
        record: Sutest::new(1, 0, 1),
    };
    let mut client = Client::new(
        MockTransport::new([
            Step::Expect(BusFrame::command(req).encode()),
            Step::Reply(reply.encode_reply_message()),
        ]),
        std::time::Duration::from_secs(1),
        1,
    );
    let result = super::run::invoke(&mut client, &a.registry, a.policy, call).await;
    a.update(Event::Completed(result));
    assert!(a.result.contains("\"status\": \"ok\""));
    assert!(client.transport().finished());
}

#[test]
fn every_builtin_form_comes_from_its_schema() {
    let registry = hblctl_core::registry::builtin();
    let mut app = App::new(registry.clone(), Policy::default());
    for (_, spec) in registry.operations() {
        let schema = hblctl_core::registry::input_schema(spec);
        assert_eq!(app.fields.len(), spec.request.len());
        for field in &app.fields {
            assert_eq!(field.schema, schema["properties"][&field.name]);
            assert_eq!(
                field.required,
                schema["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!(field.name))
            );
            if let Some(default) = field.schema.get("default") {
                assert_eq!(
                    field.value,
                    default
                        .as_str()
                        .map_or_else(|| default.to_string(), str::to_owned)
                );
            }
        }
        app.update(Event::Down);
    }
}

#[test]
fn text_and_byte_lengths_and_integer_boundaries_are_validated() {
    let mut app = App::new(registry(), Policy::default());
    app.fields[0].value = "0x100".into();
    app.fields[1].value = "ok".into();
    assert!(app.update(Event::Run).is_none());
    assert!(app.result.contains("outside"));
    app.fields[0].value = "0".into();
    app.fields[1].value = "12345678".into();
    assert!(app.update(Event::Run).is_none());
    assert!(app.result.contains("NUL"));
    app.fields[1].value = "éééé".into();
    assert!(app.update(Event::Run).is_none());
    app.fields[1].value = "ok".into();
    app.update(Event::Tab);
    app.update(Event::Down);
    app.update(Event::Backspace);
    assert_eq!(app.fields[1].value, "o");
    app.update(Event::Up);
    assert_eq!(app.field, 0);
    for i in 0..510 {
        app.update(Event::Log(i.to_string()));
    }
    assert_eq!(app.logs.len(), 500);
    assert_eq!(app.logs.front().unwrap(), "10");
}

#[test]
fn empty_registry_and_no_filter_matches_are_safe() {
    let mut app = App::new(
        Registry {
            target: "empty".into(),
            commands: vec![],
        },
        Policy::default(),
    );
    for event in [
        Event::Down,
        Event::Up,
        Event::Run,
        Event::Text('x'),
        Event::Tab,
        Event::Backspace,
        Event::Run,
    ] {
        assert!(app.update(event).is_none());
    }
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(20, 8)).unwrap();
    terminal.draw(|f| super::view::draw(f, &app)).unwrap();
}

#[tokio::test]
async fn mock_connection_error_is_displayed() {
    use hblctl_core::Client;
    use hblctl_proto::{BusFrame, Sutest};
    use hblctl_transport::mock::{MockTransport, Step};
    let mut app = App::new(registry(), Policy::default());
    app.fields[1].value = "x".into();
    let Some(Effect::Invoke(call)) = app.update(Event::Run) else {
        panic!("invoke");
    };
    let mut request = Sutest::new(1, 0, 1);
    request.put(20, &[3]).unwrap();
    request.put(24, b"x\0").unwrap();
    let mut client = Client::new(
        MockTransport::new([Step::Expect(BusFrame::command(request).encode()), Step::Close]),
        std::time::Duration::from_secs(1),
        1,
    );
    let result = super::run::invoke(&mut client, &app.registry, app.policy, call).await;
    assert!(result.is_err());
    app.update(Event::Completed(result));
    assert!(app.result.starts_with("Error:"));
    assert!(!app.busy);
    assert!(client.transport().finished());
}

#[test]
fn byte_forms_validate_hex_and_length() {
    let mut reg = registry();
    let field = &mut reg.commands[0].funcs[0].request[1];
    field.ty = hblctl_core::registry::FieldType::Bytes;
    field.len = Some(2);
    let mut app = App::new(reg, Policy::default());
    assert_eq!(app.fields[1].schema["maxLength"], 4);
    for invalid in ["a", "zz", "aabbcc"] {
        app.fields[1].value = invalid.into();
        assert!(app.update(Event::Run).is_none());
    }
    app.fields[1].value = "aabb".into();
    assert!(matches!(app.update(Event::Run), Some(Effect::Invoke(_))));
}

#[test]
fn blocked_rows_are_grey_and_explain_the_ceiling() {
    use ratatui::{Terminal, backend::TestBackend, style::Color};
    let app = App::new(registry(), Policy::default());
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|f| super::view::draw(f, &app)).unwrap();
    let buffer = terminal.backend().buffer();
    let text = buffer
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(text.contains("above --allow read"));
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == "w" && cell.fg == Color::DarkGray)
    );
}
