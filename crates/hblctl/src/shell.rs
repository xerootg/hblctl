// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Streaming shell command line frontend.
use anyhow::{Context, Result};
use clap::{Arg, ArgMatches, Command};
use hblctl_core::{Policy, Safety};
use std::io::Write;

pub(crate) fn command() -> Command {
    Command::new("shell")
        .about("Stream a root shell (requires --allow destructive)")
        .arg(
            Arg::new("script")
                .long("script")
                .value_name("FILE")
                .conflicts_with("command"),
        )
        .arg(Arg::new("command").num_args(1).allow_hyphen_values(true))
}
pub(crate) fn run(m: &ArgMatches, sm: &ArgMatches, argv: &[String]) -> Result<u8> {
    let policy = Policy::up_to(crate::cli::allowed(m));
    policy.check("shell.run", Safety::Destructive)?;
    let script = sm
        .get_one::<String>("script")
        .map(std::fs::read_to_string)
        .transpose()
        .context("reading shell script")?;
    let command = script.unwrap_or_else(|| sm.get_one::<String>("command").cloned().unwrap_or_default());
    validate_command(&command)?;
    let camera = crate::camera::CameraSession::open(m, argv, None)?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut client = camera.connect().await?;
        let mut shell = client.start_shell(policy, &command).await?;
        loop {
            tokio::select! {
                biased;
                bytes = shell.next_output() => match bytes? {
                    Some(bytes) => { let mut out = std::io::stdout().lock(); out.write_all(&bytes)?; out.flush()?; },
                    None => break,
                },
                signal = tokio::signal::ctrl_c() => { signal?; shell.interrupt().await?; },
            }
        }
        Ok(u8::from(shell.wait().await? != 0))
    })
}

fn validate_command(command: &str) -> Result<()> {
    anyhow::ensure!(
        !command.is_empty(),
        "interactive stdin and EOF forwarding are unsupported; provide one complete command string"
    );
    anyhow::ensure!(
        command.len() <= 231,
        "command/script exceeds 231 UTF-8 bytes; stdin uploads are unsupported by the bridge"
    );
    anyhow::ensure!(
        !command.contains(['\0', '\x03']),
        "command/script contains NUL or ETX"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_must_fit_one_initial_record() {
        for text in ["", &"x".repeat(232), "exit\n\x03", "x\0"] {
            assert!(validate_command(text).is_err());
        }
        assert!(validate_command("exit\nreboot\n").is_ok());
        assert!(validate_command("cat > file").is_ok());
    }

    #[test]
    fn command_keeps_argument_boundaries() {
        assert!(
            command()
                .try_get_matches_from(["shell", "--", "rm", "file name"])
                .is_err()
        );
        let m = command()
            .try_get_matches_from(["shell", "--", "rm 'file name'"])
            .unwrap();
        assert_eq!(m.get_one::<String>("command").unwrap(), "rm 'file name'");
    }
}
