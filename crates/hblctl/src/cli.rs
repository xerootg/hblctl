// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! The command line, generated from the registry at runtime.
//!
//! Every registry operation becomes `hblctl <command> <func> [--field VALUE ...]`, with help text, types and
//! defaults taken from the registry. Nothing here is specific to any one command.

use clap::{Arg, ArgAction, ArgMatches, Command};
use hblctl_core::registry::{FieldType, Registry};
use hblctl_core::safety::Safety;
use serde_json::{Map, Value};

/// Global options shared by every operation.
pub(crate) fn globals(cmd: Command) -> Command {
    cmd.arg(
        Arg::new("host")
            .long("host")
            .global(true)
            .default_value(hblctl_transport::tcp::DEFAULT_HOST)
            .help("Camera address"),
    )
    .arg(
        Arg::new("port")
            .long("port")
            .global(true)
            .value_parser(clap::value_parser!(u16))
            .default_value("30303")
            .help("Diagnostic port"),
    )
    .arg(
        Arg::new("timeout")
            .long("timeout")
            .global(true)
            .value_parser(clap::value_parser!(u64))
            .default_value("10")
            .help("Seconds to wait for a result"),
    )
    .arg(
        Arg::new("allow")
            .long("allow")
            .global(true)
            .value_parser(["read", "volatile", "persistent", "destructive"])
            .default_value("read")
            .help("Highest safety class to allow; anything above it is refused before sending"),
    )
    .arg(
        Arg::new("verbose")
            .short('v')
            .long("verbose")
            .global(true)
            .action(ArgAction::Count)
            .help("More console output (the session log always records everything)"),
    )
}

/// One subcommand per registry command, one sub-subcommand per function, one flag per request field.
pub(crate) fn operations(registry: &Registry) -> Vec<Command> {
    registry
        .commands
        .iter()
        .map(|c| {
            let mut cmd = Command::new(c.name.clone())
                .about(if c.doc.is_empty() {
                    format!("Command {}", c.id)
                } else {
                    format!("{} (command {})", c.doc, c.id)
                })
                .subcommand_required(true);
            for f in &c.funcs {
                let mut about = format!(
                    "[{}{}] {}",
                    f.safety,
                    if f.verified { "" } else { ", unverified layout" },
                    f.doc
                );
                if about.ends_with(' ') {
                    about.pop();
                }
                let mut sub = Command::new(f.name.clone()).about(about);
                for fld in &f.request {
                    let mut a = Arg::new(fld.name.clone()).long(fld.name.replace('_', "-")).help({
                        let mut help = field_help(fld.ty, fld.len, &fld.doc);
                        if let Some(entries) = fld.table.as_ref().and_then(|t| registry.tables.get(t)) {
                            help.push_str("; names: ");
                            help.push_str(
                                &entries
                                    .iter()
                                    .map(|e| format!("{} ({})", e.alias(), e.name))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            );
                        }
                        help
                    });
                    match &fld.default {
                        Some(d) => a = a.default_value(json_to_arg(d)),
                        None => a = a.required(true),
                    }
                    sub = sub.arg(a);
                }
                cmd = cmd.subcommand(sub);
            }
            cmd
        })
        .collect()
}

fn field_help(ty: FieldType, len: Option<usize>, doc: &str) -> String {
    let t = match ty {
        FieldType::Bytes => format!("hex, up to {} bytes", len.unwrap_or(0)),
        FieldType::Cstr => format!("text, up to {} bytes", len.unwrap_or(0).saturating_sub(1)),
        other => format!("{other:?}").to_lowercase(),
    };
    if doc.is_empty() {
        format!("<{t}>")
    } else {
        format!("{doc} <{t}>")
    }
}

fn json_to_arg(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A parsed operation call.
#[derive(Debug, PartialEq)]
pub(crate) struct Call {
    /// Registry command name.
    pub(crate) command: String,
    /// Function name.
    pub(crate) func: String,
    /// Arguments, as JSON values the codec understands.
    pub(crate) args: Map<String, Value>,
}

/// If `m` selected a registry operation, extract it. Integer fields stay strings here; the codec parses
/// decimal and 0x-hex strings, so the CLI and MCP paths share one validator.
pub(crate) fn call(registry: &Registry, m: &ArgMatches) -> Option<Call> {
    let (cname, cm) = m.subcommand()?;
    let c = registry.command(cname)?;
    let (fname, fm) = cm.subcommand()?;
    let f = c.func(fname)?;
    let mut args = Map::new();
    for fld in &f.request {
        if let Some(v) = fm.get_one::<String>(&fld.name) {
            args.insert(fld.name.clone(), Value::String(v.clone()));
        }
    }
    Some(Call {
        command: c.name.clone(),
        func: f.name.clone(),
        args,
    })
}

/// The `--allow` value.
pub(crate) fn allowed(m: &ArgMatches) -> Safety {
    match m.get_one::<String>("allow").map(String::as_str) {
        Some("volatile") => Safety::Volatile,
        Some("persistent") => Safety::Persistent,
        Some("destructive") => Safety::Destructive,
        _ => Safety::Read,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REG: &str = r#"{"target": "t", "commands": [
      {"id": 49, "name": "prod-config", "doc": "Factory parameters", "funcs": [
        {"func": 2, "name": "get", "safety": "read", "verified": true,
         "request": [{"name": "field_id", "offset": 20, "type": "u32"}]},
        {"func": 9, "name": "probe", "safety": "read",
         "request": [{"name": "which", "offset": 20, "type": "u8", "default": 3}]}]}]}"#;

    fn app(reg: &Registry) -> Command {
        globals(Command::new("hblctl")).subcommands(operations(reg))
    }

    #[test]
    fn production_help_and_names() {
        let reg = hblctl_core::registry::builtin();
        for name in ["wifi-region", "13"] {
            let m = app(&reg)
                .try_get_matches_from(["hblctl", "prod-config", "get", "--field-index", name])
                .unwrap();
            let c = call(&reg, &m).unwrap();
            let f = reg.command(&c.command).unwrap().func(&c.func).unwrap();
            assert!(hblctl_core::codec::encode_with_tables(&reg, f, &c.args).is_ok());
        }
        let mut cmd = app(&reg);
        let help = cmd
            .find_subcommand_mut("prod-config")
            .unwrap()
            .find_subcommand_mut("get")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("wifi-region"));
        assert!(help.contains("suSerial"));
    }

    #[test]
    fn generated_subcommands_parse_into_calls() {
        let reg = Registry::from_json(REG).unwrap();
        let m = app(&reg)
            .try_get_matches_from(["hblctl", "prod-config", "get", "--field-id", "0x0d"])
            .unwrap();
        let c = call(&reg, &m).unwrap();
        assert_eq!(c.command, "prod-config");
        assert_eq!(c.func, "get");
        assert_eq!(c.args["field_id"], Value::String("0x0d".into()));
        assert_eq!(allowed(&m), Safety::Read);
    }

    #[test]
    fn defaults_required_flags_and_allow() {
        let reg = Registry::from_json(REG).unwrap();
        let m = app(&reg)
            .try_get_matches_from(["hblctl", "--allow", "persistent", "prod-config", "probe"])
            .unwrap();
        assert_eq!(call(&reg, &m).unwrap().args["which"], Value::String("3".into()));
        assert_eq!(allowed(&m), Safety::Persistent);
        assert!(
            app(&reg)
                .try_get_matches_from(["hblctl", "prod-config", "get"])
                .is_err(),
            "required field"
        );
        assert!(
            app(&reg)
                .try_get_matches_from(["hblctl", "--allow", "everything", "prod-config", "probe"])
                .is_err()
        );
    }

    #[test]
    fn help_shows_safety_and_verification() {
        let reg = Registry::from_json(REG).unwrap();
        let mut a = app(&reg);
        let help = a
            .find_subcommand_mut("prod-config")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(
            help.lines()
                .any(|l| l.trim_start().starts_with("get") && l.trim_end().ends_with("[read]")),
            "{help}"
        );
        assert!(help.contains("[read, unverified layout]"), "{help}");
        app(&reg).debug_assert();
    }
}
