// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! Streaming shell command line frontend.
use anyhow::{Context, Result};
use clap::{Arg, ArgMatches, Command};
use hblctl_core::{Policy, Safety};
use std::io::{BufRead, Write};

pub(crate) fn command() -> Command {
    Command::new("shell")
        .about("Stream a root shell (requires --allow destructive)")
        .arg(
            Arg::new("script")
                .long("script")
                .value_name("FILE")
                .conflicts_with("command"),
        )
        .arg(
            Arg::new("command")
                .num_args(1..)
                .trailing_var_arg(true)
                .allow_hyphen_values(true),
        )
}
pub(crate) fn run(m: &ArgMatches, sm: &ArgMatches, argv: &[String]) -> Result<u8> {
    let policy = Policy::up_to(crate::cli::allowed(m));
    policy.check("shell.run", Safety::Destructive)?;
    let script = sm
        .get_one::<String>("script")
        .map(std::fs::read_to_string)
        .transpose()
        .context("reading shell script")?;
    if let Some(script) = &script {
        anyhow::ensure!(!script.contains('\0'), "script contains NUL");
    }
    let command = sm
        .get_many::<String>("command")
        .map(|v| v.cloned().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    // Read all script lines before evaluation: an early `exit` must never turn
    // remaining stdin records into new shell starts after process completion.
    let script = script.map(prepare_script);
    let camera = crate::camera::CameraSession::open(m, argv, None)?;
    tokio::runtime::Runtime::new()?.block_on(async {
        let mut client = camera.connect().await?;
        let mut shell = client.start_shell(policy, script.as_ref().map_or(command.as_str(), |(start, _)| start.as_str())).await?;
        let mut stdin_open = script.is_none() && command.is_empty();
        let (tx, mut input) = tokio::sync::mpsc::unbounded_channel();
        if stdin_open {
            // Tokio stdin uses a blocking task that can prevent runtime shutdown.
            // A detached reader lets remote completion exit even with local stdin open.
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    if tx.send(line).is_err() { break; }
                }
            });
        } else { drop(tx); }
        if let Some((_, script)) = script {
            shell.write_stdin(&script).await?;
        }
        loop {
            tokio::select! {
                biased;
                bytes = shell.next_output() => match bytes? {
                    Some(bytes) => { let mut out = std::io::stdout().lock(); out.write_all(&bytes)?; out.flush()?; },
                    None => break,
                },
                line = input.recv(), if stdin_open => {
                    if let Some(line) = line {
                        shell.write_stdin(&(line? + "\n")).await?;
                    } else {
                        stdin_open = false;
                        shell.write_stdin("exit\n").await?;
                    }
                },
                signal = tokio::signal::ctrl_c() => { signal?; shell.interrupt().await?; },
            }
        }
        Ok(u8::from(shell.wait().await? != 0))
    })
}

fn prepare_script(mut text: String) -> (String, String) {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let lines = text.bytes().filter(|b| *b == b'\n').count();
    let start = format!(
        "_h=; _n=0; while [ \"$_n\" -lt {lines} ]; do IFS= read -r _l || exit 1; _h=\"$_h$_l\n\"; _n=$((_n+1)); done; eval \"$_h\""
    );
    (start, text)
}

#[cfg(all(test, unix))] // the only test runs the prepared script through the local /bin/sh
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn scripts_are_read_before_execution_and_keep_exit_status() {
        let (start, text) = prepare_script("printf '%s\\n' 'é'; exit 7\nignored\n".into());
        assert!(start.len() <= 231);
        let mut child = Command::new("/bin/sh")
            .args(["-c", &start])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.stdout, "é\n".as_bytes());
        assert_eq!(output.status.code(), Some(7));
    }
}
