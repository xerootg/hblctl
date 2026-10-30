// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! The single entry point every frontend calls: look up an operation in the registry, check it against
//! the safety policy, encode the arguments, run it, and return a JSON result.

use hblctl_transport::Transport;
use serde_json::{Map, Value, json};

use crate::client::{Client, ClientError, Status};
use crate::codec::{self, ArgError};
use crate::registry::Registry;
use crate::safety::{Policy, Refused};

/// Why an operation did not run or did not complete.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// No such command or function in the registry.
    #[error("unknown operation `{0}`")]
    Unknown(String),
    /// Refused by the safety policy; nothing was sent.
    #[error(transparent)]
    Refused(#[from] Refused),
    /// Bad arguments; nothing was sent.
    #[error(transparent)]
    Args(#[from] ArgError),
    /// The exchange with the camera failed.
    #[error(transparent)]
    Client(#[from] ClientError),
}

/// Run `command`/`func` with `args`. The JSON result has `status`, decoded response fields, progress values,
/// raw notifications and the raw result record, so nothing the camera returned is lost to the caller.
pub async fn invoke<T: Transport>(
    client: &mut Client<T>,
    registry: &Registry,
    policy: Policy,
    command: &str,
    func: &str,
    args: &Map<String, Value>,
) -> Result<Value, DispatchError> {
    let c = registry
        .command(command)
        .ok_or_else(|| DispatchError::Unknown(command.to_owned()))?;
    let f = c
        .func(func)
        .ok_or_else(|| DispatchError::Unknown(format!("{}.{func}", c.name)))?;
    let what = format!("{}.{}", c.name, f.name);
    policy.check(&what, f.safety)?;
    let data = codec::encode(f, args)?;
    let args_json = Value::Object(args.clone());
    tracing::info!(target: "hblctl::op", op = %what, cmd = c.id, func = f.func, args = %args_json, "invoke");
    let out = client.call(c.id, f.func, &data).await?;
    let status = match &out.status {
        Status::Ok => json!("ok"),
        Status::Error { code, message } => json!({"error": {"code": code, "message": message}}),
        Status::Other(x) => json!({"other": x}),
    };
    let result = json!({
        "operation": what,
        "status": status,
        "fields": codec::decode(f, &out.result),
        "progress": out.progress,
        "notifications": out.notifications.iter().map(|n| crate::hex(&n.encode())).collect::<Vec<_>>(),
        "stray_frames": out.stray.len(),
        "raw_result": crate::hex(&out.result.encode()),
        "verified_layout": f.verified,
    });
    tracing::info!(target: "hblctl::op", op = %what, result = %result, "done");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::sample;
    use crate::safety::Safety;
    use hblctl_proto::bus::{BusHeader, NODE_EAGLE, NODE_TCP_HOST, SIGNAL_TEST_TX, TYPE_SUTEST};
    use hblctl_proto::{BusFrame, Sutest};
    use hblctl_transport::mock::{MockTransport, Step};
    use std::time::Duration;

    fn args(v: &Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn runs_a_read_operation_end_to_end() {
        let mut req = Sutest::new(49, 2, 1);
        req.put(20, &13u32.to_le_bytes()).unwrap();
        let mut res = Sutest::new(49, 2, 1);
        res.put(20, &13u32.to_le_bytes()).unwrap();
        res.put(24, &6u32.to_le_bytes()).unwrap();
        let reply = BusFrame {
            header: BusHeader {
                signal: SIGNAL_TEST_TX,
                origin: NODE_EAGLE,
                dest: NODE_TCP_HOST,
                kind: TYPE_SUTEST,
            },
            record: res,
        };
        let t = MockTransport::new([
            Step::Expect(BusFrame::command(req).encode()),
            Step::Reply(reply.encode_reply_message()),
        ]);
        let mut c = Client::new(t, Duration::from_secs(1), 1);
        let v = invoke(
            &mut c,
            &sample(),
            Policy::default(),
            "prod-config",
            "get",
            &args(&json!({"field": 13})),
        )
        .await
        .unwrap();
        assert_eq!(v["status"], json!("ok"));
        assert_eq!(v["fields"], json!({"field": 13, "value": 6}));
        assert_eq!(v["verified_layout"], json!(true));
    }

    #[tokio::test]
    async fn refuses_before_sending() {
        // An empty script: any send would fail with Exhausted, so success here proves nothing was sent.
        let mut c = Client::new(MockTransport::new([]), Duration::from_secs(1), 1);
        let r = invoke(
            &mut c,
            &sample(),
            Policy::default(),
            "prod-config",
            "set",
            &args(&json!({"field": 1, "value": 1})),
        )
        .await;
        assert!(matches!(
            r,
            Err(DispatchError::Refused(Refused {
                needed: Safety::Persistent,
                ..
            }))
        ));
        let r = invoke(
            &mut c,
            &sample(),
            Policy::up_to(Safety::Persistent),
            "prod-config",
            "set",
            &args(&json!({"field": 1})),
        )
        .await;
        assert!(matches!(r, Err(DispatchError::Args(ArgError::Missing(_)))));
        let r = invoke(&mut c, &sample(), Policy::default(), "nope", "get", &Map::new()).await;
        assert!(matches!(r, Err(DispatchError::Unknown(_))));
        assert!(c.transport().finished());
    }
}
