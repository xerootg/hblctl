// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Explicit ADB mods frontend with session evidence and typed destructive confirmation.
use crate::{cli, logging, session};
use anyhow::{Context, Result, bail};
use clap::{Arg, ArgAction, ArgMatches, Command};
use hblctl_core::safety::{Policy, Safety};
use hblctl_mods::{bundle::Bundle, camera::AdbFs, engine::Engine};
use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub(crate) fn command() -> Command {
    let mut cmd = Command::new("mods")
        .about("Inspect and transact passive camera feature bundles")
        .subcommand_required(true);
    for name in ["load", "status", "plan", "apply", "revert"] {
        let mut sub = Command::new(name).arg(
            Arg::new("bundle")
                .required(true)
                .help("Bundle .tar.zst (requires .sha256 sidecar)"),
        );
        if name != "load" {
            sub = sub
                .arg(Arg::new("via").long("via").value_parser(["adb"]).required(true))
                .arg(
                    Arg::new("recovery-session")
                        .long("recovery-session")
                        .help("Existing session containing recovery journal and backups"),
                );
        }
        if matches!(name, "plan" | "apply") {
            sub = sub.arg(Arg::new("feature").required(true).num_args(1..));
        }
        if name == "apply" || name == "revert" {
            sub = sub.arg(
                Arg::new("go")
                    .long("go")
                    .action(ArgAction::SetTrue)
                    .help("Execute after planning (default: dry run)"),
            );
        }
        if name == "revert" {
            sub = sub
                .arg(
                    Arg::new("feature")
                        .required_unless_present("all")
                        .conflicts_with("all"),
                )
                .arg(Arg::new("all").long("all").action(ArgAction::SetTrue));
        }
        cmd = cmd.subcommand(sub);
    }
    cmd
}
pub(crate) fn run(global: &ArgMatches, m: &ArgMatches, invocation: &[String]) -> Result<()> {
    let (name, args) = m.subcommand().context("missing mods operation")?;
    let session = session::Session::create(&super::sessions_root(global), invocation)?;
    let log = session.events()?;
    let subscriber = logging::subscriber(logging::Shared(Arc::new(Mutex::new(log))), "warn");
    let _guard = tracing::subscriber::set_default(subscriber);
    let bundle = Bundle::load(&PathBuf::from(
        args.get_one::<String>("bundle").context("bundle required")?,
    ))?;
    tracing::info!(target:"hblctl::mods",operation=name,bundle=%bundle.digest(),"mods invocation");
    if name == "load" {
        println!("{}", serde_json::to_string_pretty(bundle.manifest())?);
        return Ok(());
    }
    let policy = Policy::up_to(cli::allowed(global));
    let recovery = args
        .get_one::<String>("recovery-session")
        .map_or_else(|| session.dir.clone(), PathBuf::from);
    let mut camera = AdbFs;
    let mut engine = Engine::new(&bundle, &mut camera, &recovery, policy);
    match name {
        "status" => {
            for feature in bundle.manifest()["features"].as_array().context("features")? {
                let id = feature["id"].as_str().context("feature ID")?;
                println!("{id}: {:?}", engine.status(id)?);
            }
        }
        "plan" | "apply" => {
            let features: Vec<_> = args
                .get_many::<String>("feature")
                .context("features required")?
                .cloned()
                .collect();
            let plan = engine.plan(&features)?;
            println!("{}", serde_json::to_string_pretty(&plan.summary())?);
            if name == "apply" && args.get_flag("go") {
                confirm("mods.apply", policy)?;
                engine.apply(&plan)?;
                println!(
                    "Installed; reboot required. Recovery session: {}",
                    recovery.display()
                );
            } else {
                println!("Dry run; no camera changes.");
            }
        }
        "revert" => {
            let feature = args.get_one::<String>("feature").map(String::as_str);
            if args.get_flag("go") {
                confirm("mods.revert", policy)?;
                engine.revert(feature)?;
                println!("Restored; reboot required.");
            } else {
                println!(
                    "Dry run: restore {} using {}. Use --go to execute.",
                    feature.unwrap_or("all features"),
                    recovery.display()
                );
            }
        }
        _ => bail!("unknown mods operation"),
    }
    Ok(())
}
fn confirm(operation: &str, policy: Policy) -> Result<()> {
    policy.check(operation, Safety::Destructive)?;
    eprint!("Type {operation} to confirm: ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    if line.trim() != operation {
        bail!("confirmation did not match {operation}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backend_is_explicit_and_apply_defaults_to_dry_run() {
        let app = || Command::new("hblctl").subcommand(command());
        assert!(
            app()
                .try_get_matches_from(["hblctl", "mods", "apply", "b.tar.zst", "toy"])
                .is_err()
        );
        let m = app()
            .try_get_matches_from(["hblctl", "mods", "apply", "b.tar.zst", "--via", "adb", "toy"])
            .unwrap();
        let apply = m.subcommand().unwrap().1.subcommand().unwrap().1;
        assert!(!apply.get_flag("go"));
        assert!(
            app()
                .try_get_matches_from([
                    "hblctl",
                    "mods",
                    "revert",
                    "b.tar.zst",
                    "--via",
                    "adb",
                    "--all",
                    "toy"
                ])
                .is_err()
        );
    }
}
