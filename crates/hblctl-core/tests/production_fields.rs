// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Production field tables, typed replies and wire replay regression tests.

use hblctl_core::{codec, registry};
use hblctl_proto::Sutest;
use serde_json::json;

#[test]
fn names_schema_and_typed_values() {
    let r = registry::builtin();
    let f = r.command("prod-config").unwrap().func("get").unwrap();
    for name in [json!(13), json!("13"), json!("wifiRegion"), json!("wifi-region")] {
        let data =
            codec::encode_with_tables(&r, f, json!({"field_index": name}).as_object().unwrap()).unwrap();
        assert_eq!(&data[4..8], &13i32.to_le_bytes());
    }
    let schema = registry::input_schema_with_tables(&r, f);
    assert!(
        schema["properties"]["field_index"]["anyOf"][1]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("wifi-region"))
    );
    let mut rec = Sutest::new(49, 2, 1);
    rec.put(24, &[0xa5; 228]).unwrap();
    let text: Vec<u8> = "XT99001234\0"
        .chars()
        .flat_map(|c| u32::from(c).to_le_bytes())
        .collect();
    rec.put(24, &text).unwrap();
    let out = codec::decode_with_tables(&r, f, &rec);
    assert_eq!(out["values_decoded"]["field"], "suSerial");
    assert_eq!(out["values_decoded"]["value"], "XT99001234");
    rec.put(20, &27i32.to_le_bytes()).unwrap();
    rec.put(24, &(-42i32).to_le_bytes()).unwrap();
    assert_eq!(
        codec::decode_with_tables(&r, f, &rec)["values_decoded"]["value"],
        -42
    );
    rec.put(20, &25i32.to_le_bytes()).unwrap();
    assert_eq!(
        codec::decode_with_tables(&r, f, &rec)["values_decoded"]["value"],
        codec::decode(f, &rec)["values"]
    );
}

#[test]
fn tables_are_strictly_validated() {
    let original: serde_json::Value = serde_json::from_str(registry::X2D_4_2_0).unwrap();
    for (key, value) in [
        ("name", json!("suSerial")),
        ("value", json!(0)),
        ("value", json!(i64::MAX)),
    ] {
        let mut v = original.clone();
        v["tables"]["production-fields"][1][key] = value;
        assert!(registry::Registry::from_json(&v.to_string()).is_err());
    }
    let mut v = original;
    let command = v["commands"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|c| c["id"] == 49)
        .unwrap();
    command["funcs"][0]["request"][0]["table"] = json!("missing");
    assert!(registry::Registry::from_json(&v.to_string()).is_err());
}

#[tokio::test]
async fn get_by_name_replays_exact_wire_exchange() {
    use hblctl_core::{Client, dispatch, safety::Policy};
    use hblctl_proto::{BusFrame, bus::BusHeader};
    use hblctl_transport::mock::{MockTransport, Step};
    use std::time::Duration;
    let mut request = Sutest::new(49, 2, 1);
    request.put(20, &13i32.to_le_bytes()).unwrap();
    let mut response = request.clone();
    response.put(24, &6u32.to_le_bytes()).unwrap();
    let raw = hblctl_core::hex(&response.encode());
    let reply = BusFrame {
        header: BusHeader {
            signal: 9,
            origin: 5,
            dest: 9,
            kind: 0xfc,
        },
        record: response,
    };
    let transport = MockTransport::new([
        Step::Expect(BusFrame::command(request).encode()),
        Step::Reply(reply.encode_reply_message()),
    ]);
    let mut client = Client::new(transport, Duration::from_secs(1), 1);
    let result = dispatch::invoke(
        &mut client,
        &registry::builtin(),
        Policy::default(),
        "prod-config",
        "get",
        json!({"field_index": "wifi-region"}).as_object().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(result["status"], "ok");
    assert_eq!(result["fields"]["values_decoded"]["field"], "wifiRegion");
    assert_eq!(result["fields"]["values_decoded"]["value"], 6);
    assert_eq!(result["raw_result"], raw);
    assert!(client.transport().finished());
}

#[test]
fn all_declared_fields_decode_without_using_the_tail() {
    let r = registry::builtin();
    let f = r.command("prod-config").unwrap().func("get").unwrap();
    let entries = &r.tables["production-fields"];
    assert_eq!(entries.len(), 31);
    for e in entries {
        let mut rec = Sutest::new(49, 2, 1);
        rec.put(20, &i32::try_from(e.value).unwrap().to_le_bytes())
            .unwrap();
        rec.put(24, &[0xff; 228]).unwrap();
        if e.value == 0 {
            rec.put(24, &[0x80, 0xf6, 1, 0, 0, 0, 0, 0]).unwrap();
        }
        let out = codec::decode_with_tables(&r, f, &rec);
        assert_eq!(out["values_decoded"]["field"], e.name);
        match e.value {
            0 => assert_eq!(out["values_decoded"]["value"], "🚀"),
            1 | 2 | 5..=9 | 14..=17 | 23 | 24 => assert_eq!(out["values_decoded"]["value"], 255),
            3 | 13 | 18..=20 | 30 => assert_eq!(out["values_decoded"]["value"], u32::MAX),
            27 => assert_eq!(out["values_decoded"]["value"], -1),
            4 => assert_eq!(out["values_decoded"]["value"], json!(vec![-1; 6])),
            10..=12 => assert_eq!(out["values_decoded"]["value"], json!(vec![-1; 5])),
            21 => assert_eq!(out["values_decoded"]["value"], json!(vec![-1; 10])),
            22 => assert_eq!(out["values_decoded"]["value"], json!(vec![-1; 2])),
            26 => assert_eq!(out["values_decoded"]["value"], json!(vec![-1; 18])),
            25 | 28 | 29 => assert_eq!(out["values_decoded"]["value"], "ff".repeat(228)),
            _ => panic!("unexpected production field"),
        }
    }
    let mut rec = Sutest::new(49, 2, 1);
    rec.put(20, &99i32.to_le_bytes()).unwrap();
    let out = codec::decode_with_tables(&r, f, &rec);
    assert!(out["values_decoded"].get("field").is_none());
    assert_eq!(out["values_decoded"]["value"], "00".repeat(228));
    for n in [json!(-1), json!(31), json!("missing")] {
        assert!(codec::encode_with_tables(&r, f, json!({"field_index": n}).as_object().unwrap()).is_err());
    }
}

fn selected_registry() -> registry::Registry {
    let mut r = registry::builtin();
    let f = &mut r.commands.iter_mut().find(|c| c.id == 49).unwrap().funcs[2];
    f.response[0].name = "field".into();
    f.response[1].selector = Some("field".into());
    f.response[1].len = Some(4);
    let mut ordinary = f.response[0].clone();
    ordinary.name = "value".into();
    ordinary.offset = 28;
    f.response.push(ordinary);
    r
}

#[test]
fn selected_decode_preserves_ordinary_fields_and_raw_buffer() {
    let r = selected_registry();
    r.validate().unwrap();
    let f = r.command("prod-config").unwrap().func("get").unwrap();
    let mut rec = Sutest::new(49, 2, 1);
    rec.put(20, &13i32.to_le_bytes()).unwrap();
    rec.put(24, &6u32.to_le_bytes()).unwrap();
    rec.put(28, &42i32.to_le_bytes()).unwrap();
    let out = codec::decode_with_tables(&r, f, &rec);
    assert_eq!(out["field"], 13);
    assert_eq!(out["value"], 42);
    assert_eq!(out["values"], "06000000");
    assert_eq!(out["values_decoded"], json!({"field": "wifiRegion", "value": 6}));
}

#[test]
fn rejects_request_side_selector() {
    let mut r = selected_registry();
    let f = &mut r.commands.iter_mut().find(|c| c.id == 49).unwrap().funcs[2];
    f.request = f.response.clone();
    assert!(r.validate().unwrap_err().0.contains("response-only"));
}

#[test]
fn rejects_multiple_selected_response_buffers() {
    let mut r = selected_registry();
    let f = &mut r.commands.iter_mut().find(|c| c.id == 49).unwrap().funcs[2];
    let mut second = f.response[1].clone();
    second.name = "other".into();
    second.offset = 32;
    f.response.push(second);
    assert!(r.validate().unwrap_err().0.contains("multiple"));
}

#[test]
fn rejects_derived_name_collisions() {
    let mut r = selected_registry();
    let f = &mut r.commands.iter_mut().find(|c| c.id == 49).unwrap().funcs[2];
    f.response[2].name = "values_decoded".into();
    assert!(r.validate().unwrap_err().0.contains("collides"));
}
