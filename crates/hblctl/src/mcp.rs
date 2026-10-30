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
use hblctl_core::registry::{CommandSpec, FuncSpec, Registry, input_schema};
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
    registry
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
            let Value::Object(schema) = input_schema(f) else {
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
        .collect()
}

/// Split a tool name back into (command, func).
#[must_use]
pub(crate) fn route(name: &str) -> Option<(&str, &str)> {
    name.split_once(SEP)
}

/// The server: one client connection shared by all tool calls (calls are serialised).
pub(crate) struct McpServer<T> {
    client: Mutex<Client<T>>,
    registry: Registry,
    policy: Policy,
}

impl<T> McpServer<T> {
    /// Serve `registry` over `client`, exposing operations up to `policy`.
    pub(crate) fn new(client: Client<T>, registry: Registry, policy: Policy) -> Self {
        Self {
            client: Mutex::new(client),
            registry,
            policy,
        }
    }
}

impl<T: Transport + Send + Sync + 'static> McpServer<T> {
    /// Run one tool call; errors become tool errors the agent can read.
    pub(crate) async fn call(&self, name: &str, args: Map<String, Value>) -> CallToolResult {
        let Some((c, f)) = route(name) else {
            return CallToolResult::error(vec![Content::text(format!("unknown tool {name}"))]);
        };
        let mut client = self.client.lock().await;
        match invoke(&mut client, &self.registry, self.policy, c, f, &args).await {
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

    const REG: &str = r#"{"target": "test", "commands": [
      {"id": 49, "name": "prod-config", "doc": "Factory parameters", "funcs": [
        {"func": 2, "name": "get", "safety": "read", "verified": true,
         "request": [{"name": "field", "offset": 20, "type": "u32"}],
         "response": [{"name": "value", "offset": 24, "type": "u32"}]},
        {"func": 1, "name": "set", "safety": "persistent",
         "request": [{"name": "field", "offset": 20, "type": "u32"}, {"name": "value", "offset": 24, "type": "u32"}]}]}]}"#;

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
            Step::Reply(reply.encode()),
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
