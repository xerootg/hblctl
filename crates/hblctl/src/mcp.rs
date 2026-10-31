// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! MCP server: every registry operation within the safety ceiling becomes a tool.
//!
//! Tools above the ceiling are not listed at all, so an agent cannot even attempt them; raising the ceiling
//! is a decision made by whoever starts the server (`hblctl --allow ... mcp`). Tool names are
//! `<command>__<func>`; input schemas come from [`hblctl_core::registry::input_schema`], so the agent sees
//! exactly the arguments, types and ranges the CLI accepts.

use std::sync::Arc;

use hblctl_core::Client;
use hblctl_core::dispatch::invoke;
use hblctl_core::registry::{CommandSpec, FuncSpec, Registry, input_schema_with_tables};
use hblctl_core::safety::{Policy, Safety};
use hblctl_transport::Transport;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock as Content, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

const SEP: &str = "__";

/// Tool name for an operation.
#[must_use]
pub(crate) fn tool_name(c: &CommandSpec, f: &FuncSpec) -> String {
    format!("{}{SEP}{}", c.name, f.name)
}

/// Every tool visible under `policy`.
#[must_use]
pub(crate) fn tools(registry: &Registry, policy: Policy) -> Vec<Tool> {
    let mut result: Vec<Tool> = registry
        .operations()
        .filter(|(_, f)| policy.check("", f.safety).is_ok())
        .map(|(c, f)| {
            let doc = [c.doc.as_str(), f.doc.as_str()]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(": ");
            let desc = format!(
                "{doc} [command {} func {}; safety {}{}]",
                c.id,
                f.func,
                f.safety,
                if f.verified {
                    ""
                } else {
                    "; layout not fully verified, check raw_result"
                }
            );
            let Value::Object(schema) = input_schema_with_tables(registry, f) else {
                unreachable!("input_schema returns an object")
            };
            Tool::new(tool_name(c, f), desc, Arc::new(schema)).with_annotations(
                ToolAnnotations::new()
                    .read_only(f.safety == Safety::Read)
                    .destructive(f.safety >= Safety::Persistent)
                    .idempotent(f.safety == Safety::Read)
                    .open_world(false),
            )
        })
        .collect();
    if policy.max == Safety::Destructive {
        let schema = serde_json::json!({"type":"object","properties":{"command":{"type":"string","minLength":1,"maxLength":231}},"required":["command"],"additionalProperties":false});
        result.push(Tool::new("shell__run", "Run a root shell command; returns output and wire success/failure (not numeric child exit code)", Arc::new(schema.as_object().unwrap().clone())).with_annotations(ToolAnnotations::new().read_only(false).destructive(true).idempotent(false)));
    }
    result
}

/// Split a tool name back into (command, func).
#[must_use]
pub(crate) fn route(name: &str) -> Option<(&str, &str)> {
    name.split_once(SEP)
}

/// The server: one client connection shared by all tool calls (calls are serialised).
pub(crate) struct McpServer<T> {
    client: Mutex<Option<Client<T>>>,
    reconnect: Option<Reconnect<T>>,
    registry: Registry,
    policy: Policy,
}

type Reconnect<T> =
    Box<dyn Fn() -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<Client<T>>> + Send>> + Send + Sync>;

impl<T> McpServer<T> {
    /// Serve `registry` over `client`, exposing operations up to `policy`.
    pub(crate) fn new(client: Client<T>, registry: Registry, policy: Policy) -> Self {
        Self {
            client: Mutex::new(Some(client)),
            reconnect: None,
            registry,
            policy,
        }
    }
}

impl<T: Transport + Send + Sync + 'static> McpServer<T> {
    pub(crate) fn with_reconnect(mut self, reconnect: Reconnect<T>) -> Self {
        self.reconnect = Some(reconnect);
        self
    }

    /// Run one tool call; errors become tool errors the agent can read.
    pub(crate) async fn call(&self, name: &str, args: Map<String, Value>) -> CallToolResult {
        let mut slot = self.client.lock().await;
        if slot.as_ref().is_some_and(Client::shell_active) {
            *slot = None;
        }
        let result: anyhow::Result<Value> = async {
            if slot.is_none() {
                let reconnect = self.reconnect.as_ref().ok_or_else(|| anyhow::anyhow!("connection discarded; reconnect required"))?;
                *slot = Some(reconnect().await?);
            }
            let client = slot.as_mut().expect("connected");
            if name == "shell__run" {
                self.policy.check("shell.run", Safety::Destructive)?;
                anyhow::ensure!(args.len() == 1, "expected only command");
                let command = args.get("command").and_then(Value::as_str).ok_or_else(|| anyhow::anyhow!("expected command string"))?;
                anyhow::ensure!(!command.is_empty(), "non-interactive shell command must not be empty");
                let mut session = client.start_shell(self.policy, command).await?;
                let mut output = Vec::new();
                while let Some(bytes) = session.next_output().await? { output.extend(bytes); }
                return Ok(serde_json::json!({"output": String::from_utf8_lossy(&output), "output_hex": hblctl_core::hex(&output), "exit_status": session.wait().await?}));
            }
            let (c, f) = route(name).ok_or_else(|| anyhow::anyhow!("unknown tool {name}"))?;
            Ok(invoke(client, &self.registry, self.policy, c, f, &args).await?)
        }.await;
        // A failed exchange may leave a live shell or unread replies. Discard the
        // connection and open a fresh recorded stream on the next tool call.
        if result.is_err() && slot.as_ref().is_some_and(Client::shell_active) {
            *slot = None;
        }
        match result {
            Ok(v) => CallToolResult::success(vec![Content::text(
                serde_json::to_string_pretty(&v).unwrap_or_default(),
            )]),
            Err(e) => CallToolResult::error(vec![Content::text(e.to_string())]),
        }
    }
}

impl<T: Transport + Send + Sync + 'static> ServerHandler for McpServer<T> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("hblctl", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!(
                "Factory diagnostic protocol of a Hasselblad camera ({}). Operations above safety class `{}` are \
                 not exposed. Every call is logged with its raw bytes.",
                self.registry.target, self.policy.max
            ))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(tools(
            &self.registry,
            self.policy,
        ))))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        Ok(self
            .call(&request.name, request.arguments.unwrap_or_default())
            .await
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hblctl_proto::bus::{BusHeader, NODE_EAGLE, NODE_TCP_HOST, SIGNAL_TEST_TX, TYPE_SUTEST};
    use hblctl_proto::{BusFrame, Sutest};
    use hblctl_transport::mock::{MockTransport, Step};
    use std::time::Duration;

    struct DelayedMock {
        wire: MockTransport,
        delay: Duration,
    }
    impl Transport for DelayedMock {
        async fn send(&mut self, bytes: &[u8]) -> Result<(), hblctl_transport::Error> {
            self.wire.send(bytes).await
        }
        async fn recv(&mut self) -> Result<Option<Vec<u8>>, hblctl_transport::Error> {
            tokio::time::sleep(self.delay).await;
            self.wire.recv().await
        }
    }

    const REG: &str = r#"{"target": "test", "commands": [
      {"id": 49, "name": "prod-config", "doc": "Factory parameters", "funcs": [
        {"func": 2, "name": "get", "safety": "read", "verified": true,
         "request": [{"name": "field", "offset": 20, "type": "u32"}],
         "response": [{"name": "value", "offset": 24, "type": "u32"}]},
        {"func": 1, "name": "set", "safety": "persistent",
         "request": [{"name": "field", "offset": 20, "type": "u32"}, {"name": "value", "offset": 24, "type": "u32"}]}]}]}"#;

    #[test]
    fn production_tool_schema_has_names_and_integers() {
        let list = tools(&hblctl_core::registry::builtin(), Policy::default());
        let tool = list.iter().find(|t| t.name == "prod-config__get").unwrap();
        let choices = &tool.input_schema["properties"]["field_index"]["anyOf"];
        assert_eq!(choices[0]["type"], "integer");
        assert!(
            choices[1]["enum"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("wifi-region"))
        );
    }

    #[test]
    fn tools_respect_the_ceiling_and_carry_hints() {
        let reg = Registry::from_json(REG).unwrap();
        let ro = tools(&reg, Policy::default());
        assert_eq!(
            ro.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(),
            vec!["prod-config__get"]
        );
        let a = ro[0].annotations.as_ref().unwrap();
        assert_eq!((a.read_only_hint, a.destructive_hint), (Some(true), Some(false)));
        assert_eq!(ro[0].input_schema["required"], serde_json::json!(["field"]));
        let rw = tools(&reg, Policy::up_to(Safety::Persistent));
        assert_eq!(rw.len(), 2);
        assert_eq!(rw[1].annotations.as_ref().unwrap().destructive_hint, Some(true));
        assert_eq!(route("prod-config__get"), Some(("prod-config", "get")));
    }

    #[tokio::test]
    async fn shell_tool_requires_destructive_ceiling() {
        let reg = Registry::from_json(REG).unwrap();
        assert!(
            !tools(&reg, Policy::default())
                .iter()
                .any(|t| t.name == "shell__run")
        );
        assert!(
            tools(&reg, Policy::up_to(Safety::Destructive))
                .iter()
                .any(|t| t.name == "shell__run")
        );
        let server = McpServer::new(
            Client::new(MockTransport::new([]), Duration::from_secs(1), 1),
            reg,
            Policy::default(),
        );
        assert_eq!(
            server
                .call(
                    "shell__run",
                    serde_json::json!({"command":"true"}).as_object().unwrap().clone()
                )
                .await
                .is_error,
            Some(true)
        );
    }

    #[tokio::test]
    async fn unfinished_shell_reconnects_for_both_routes() {
        fn request() -> Vec<u8> {
            let mut r = Sutest::new(65, 1, 1);
            r.data[4..8].copy_from_slice(b"true");
            BusFrame::command(r).encode()
        }
        for delay in [Duration::ZERO, Duration::from_millis(20)] {
            for (name, args) in [
                ("shell__run", serde_json::json!({"command":"true"})),
                (
                    "hbl-shell__start-shell",
                    serde_json::json!({"command_utf8_z":"true"}),
                ),
            ] {
                let first = Client::new(
                    DelayedMock {
                        wire: MockTransport::new([Step::Expect(request()), Step::Close]),
                        delay,
                    },
                    Duration::from_millis(10),
                    1,
                );
                let server = McpServer::new(
                    first,
                    hblctl_core::registry::builtin(),
                    Policy::up_to(Safety::Destructive),
                )
                .with_reconnect(Box::new(|| {
                    Box::pin(async {
                        let reply = BusFrame {
                            header: BusHeader {
                                signal: SIGNAL_TEST_TX,
                                origin: NODE_EAGLE,
                                dest: NODE_TCP_HOST,
                                kind: TYPE_SUTEST,
                            },
                            record: Sutest::new(65, 1, 99),
                        };
                        Ok(Client::new(
                            DelayedMock {
                                wire: MockTransport::new([
                                    Step::Expect(request()),
                                    Step::Reply(reply.encode_reply_message()),
                                ]),
                                delay: Duration::ZERO,
                            },
                            Duration::from_secs(1),
                            1,
                        ))
                    })
                }));
                assert_eq!(
                    server
                        .call(name, args.as_object().unwrap().clone())
                        .await
                        .is_error,
                    Some(true)
                );
                assert!(server.client.lock().await.is_none());
                assert_eq!(
                    server
                        .call(name, args.as_object().unwrap().clone())
                        .await
                        .is_error,
                    Some(false)
                );
                assert!(
                    server
                        .client
                        .lock()
                        .await
                        .as_ref()
                        .unwrap()
                        .transport()
                        .wire
                        .finished()
                );
            }
        }
    }
    #[tokio::test]
    async fn empty_shell_is_refused_without_sending() {
        let server = McpServer::new(
            Client::new(MockTransport::new([]), Duration::from_secs(1), 1),
            hblctl_core::registry::builtin(),
            Policy::up_to(Safety::Destructive),
        );
        for (name, args) in [
            ("shell__run", serde_json::json!({"command":""})),
            ("hbl-shell__start-shell", serde_json::json!({"command_utf8_z":""})),
        ] {
            assert_eq!(
                server
                    .call(name, args.as_object().unwrap().clone())
                    .await
                    .is_error,
                Some(true)
            );
        }
        assert!(
            server
                .client
                .lock()
                .await
                .as_ref()
                .unwrap()
                .transport()
                .finished()
        );
    }

    #[tokio::test]
    async fn call_runs_through_the_dispatcher_and_refusals_are_tool_errors() {
        let reg = Registry::from_json(REG).unwrap();
        let mut req = Sutest::new(49, 2, 1);
        req.put(20, &13u32.to_le_bytes()).unwrap();
        let mut res = Sutest::new(49, 2, 1);
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
        let server = McpServer::new(Client::new(t, Duration::from_secs(1), 1), reg, Policy::default());
        let mut args = Map::new();
        args.insert("field".into(), 13.into());
        let ok = server.call("prod-config__get", args.clone()).await;
        assert_eq!(ok.is_error, Some(false));
        assert!(
            format!("{:?}", ok.content).contains("\\\"value\\\": 6"),
            "{:?}",
            ok.content
        );
        args.insert("value".into(), 1.into());
        let refused = server.call("prod-config__set", args).await;
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(server.call("nonsense", Map::new()).await.is_error, Some(true));
    }
}
