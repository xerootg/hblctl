// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! `hblctl`: client for the Hasselblad X-system factory diagnostic protocol.
//!
//! The command line is generated from the registry (`hblctl <command> <func> --field VALUE`), next to a few
//! fixed commands: `ops`, `tui`, `mcp`, `capture dump` and `sessions`. Every run that talks to a camera gets a
//! session directory with a JSON event log and a binary capture of the socket.

mod camera;
mod cli;
mod dump;
mod logging;
mod mcp;
mod mods;
mod session;
mod tui;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Arg, ArgAction, ArgMatches, Command};
use hblctl_core::dispatch;
use hblctl_core::registry::{self, Registry};
use hblctl_core::safety::Policy;
use hblctl_transport::capture::CaptureReader;

fn app(reg: &Registry) -> Command {
    cli::globals(Command::new("hblctl").version(env!("CARGO_PKG_VERSION")).about(
        "Client for the Hasselblad X-system factory diagnostic protocol. Operations are refused above --allow \
         (default: read).",
    ))
    .arg(Arg::new("registry").long("registry").global(true).help("Use this registry JSON instead of the built-in one"))
    .arg(
        Arg::new("sessions")
            .long("sessions")
            .global(true)
            .help("Directory for session logs and captures (default: the platform state directory)"),
    )
    .subcommand_required(true)
    .subcommand(Command::new("tui").about("Interactive registry-generated camera console"))
    .subcommand(Command::new("ops").about("List every operation with its safety class"))
    .subcommand(Command::new("mcp").about("Serve the operations allowed by --allow as MCP tools on stdin/stdout"))
    .subcommand(
        Command::new("capture").about("Work with session captures").subcommand_required(true).subcommand(
            Command::new("dump")
                .about("Decode a capture into frames")
                .arg(Arg::new("file").required(true))
                .arg(Arg::new("hex").long("hex").action(ArgAction::SetTrue).help("Also print chunk bytes")),
        ),
    )
    .subcommand(mods::command())
    .subcommand(Command::new("sessions").about("Print where session logs and captures are written"))
    .subcommands(cli::operations(reg))
}

/// `--registry PATH` or `--registry=PATH`, read from argv before parsing because the CLI is generated from it.
fn registry_arg(args: &[String]) -> Option<String> {
    args.iter()
        .position(|a| a == "--registry")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix("--registry=").map(str::to_owned))
        })
}

fn load_registry(args: &[String]) -> Result<Registry> {
    match registry_arg(args) {
        Some(p) => {
            let text = std::fs::read_to_string(&p).with_context(|| format!("reading registry {p}"))?;
            Ok(Registry::from_json(&text)?)
        }
        None => Ok(registry::builtin()),
    }
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let reg = load_registry(&argv)?;
    let m = app(&reg).get_matches_from(&argv);
    match m.subcommand() {
        Some(("mods", mm)) => mods::run(&m, mm, &argv),
        Some(("ops", _)) => {
            for (c, f) in reg.operations() {
                println!(
                    "{:<12} {:<28} {:<32} cmd={:<3} func={:<2}{}",
                    f.safety.to_string(),
                    c.name,
                    f.name,
                    c.id,
                    f.func,
                    if f.verified { "" } else { "  (layout unverified)" }
                );
            }
            Ok(())
        }
        Some(("capture", cm)) => {
            let Some(("dump", dm)) = cm.subcommand() else {
                unreachable!("subcommand_required")
            };
            let file = PathBuf::from(dm.get_one::<String>("file").expect("required"));
            let f = std::fs::File::open(&file).with_context(|| format!("opening {}", file.display()))?;
            let records = CaptureReader::new(std::io::BufReader::new(f))?.records()?;
            for line in dump::render(&records, dm.get_flag("hex")) {
                println!("{line}");
            }
            Ok(())
        }
        Some(("sessions", _)) => {
            println!("{}", sessions_root(&m).display());
            Ok(())
        }
        Some(("tui", _)) => {
            let (tx, rx) = std::sync::mpsc::channel();
            let camera = camera::CameraSession::open(&m, &argv, Some(tx))?;
            tui::run(camera, reg, Policy::up_to(cli::allowed(&m)), &rx)
        }
        Some(("mcp", _)) => run_camera(&m, &argv, reg, true),
        Some(_) => run_camera(&m, &argv, reg, false),
        None => unreachable!("subcommand_required"),
    }
}

fn sessions_root(m: &ArgMatches) -> PathBuf {
    m.get_one::<String>("sessions")
        .map_or_else(session::default_root, PathBuf::from)
}

/// Open a session, connect (recording every byte), then run one operation or the MCP server.
fn run_camera(m: &ArgMatches, argv: &[String], reg: Registry, mcp_mode: bool) -> Result<()> {
    let camera = camera::CameraSession::open(m, argv, None)?;
    let policy = Policy::up_to(cli::allowed(m));
    let call = if mcp_mode {
        None
    } else {
        Some(cli::call(&reg, m).context("no operation selected")?)
    };

    tokio::runtime::Runtime::new()?.block_on(async move {
        let mut client = camera.connect().await?;
        match call {
            None => {
                use rmcp::ServiceExt as _;
                let server = mcp::McpServer::new(client, reg, policy);
                let running = server
                    .serve(rmcp::transport::stdio())
                    .await
                    .context("starting MCP server")?;
                running.waiting().await.context("MCP server")?;
            }
            Some(call) => {
                let result =
                    dispatch::invoke(&mut client, &reg, policy, &call.command, &call.func, &call.args)
                        .await?;
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
        }
        anyhow::Ok(())
    })
}

/// Cookies start from the clock so concurrent sessions are distinguishable in captures.
fn initial_cookie() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.subsec_nanos() | 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_flag_forms() {
        let v = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            registry_arg(&v(&["hblctl", "--registry", "r.json", "ops"])),
            Some("r.json".into())
        );
        assert_eq!(
            registry_arg(&v(&["hblctl", "--registry=r.json", "ops"])),
            Some("r.json".into())
        );
        assert_eq!(registry_arg(&v(&["hblctl", "ops"])), None);
    }

    #[test]
    fn builtin_registry_generates_a_valid_cli() {
        app(&registry::builtin()).debug_assert();
    }
}
