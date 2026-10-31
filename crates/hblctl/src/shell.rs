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
        let deadline = shell.deadline();
        let (output, chunks) = std::sync::mpsc::sync_channel::<OutputWrite>(1);
        // Keep blocking stdout I/O off the runtime. This detached thread owns at most
        // one wire chunk; a stalled pipe cannot prevent timeout or Ctrl-C handling.
        std::thread::spawn(move || {
            let mut out = std::io::stdout().lock();
            while let Ok((bytes, done)) = chunks.recv() {
                let result = out.write_all(&bytes).and_then(|()| out.flush());
                let failed = result.is_err();
                let _ = done.send(result);
                if failed {
                    break;
                }
            }
        });
        let stream = async {
            while let Some(bytes) = shell.next_output().await? {
                let (done, written) = tokio::sync::oneshot::channel();
                output.try_send((bytes, done)).context("stdout worker stopped")?;
                written.await.context("stdout worker stopped")??;
            }
            anyhow::Ok(())
        };
        until_deadline_or_signal(stream, tokio::signal::ctrl_c(), deadline).await?;

        Ok(u8::from(shell.wait().await? != 0))
    })
}

type OutputWrite = (Vec<u8>, tokio::sync::oneshot::Sender<std::io::Result<()>>);

async fn until_deadline_or_signal(
    work: impl std::future::Future<Output = Result<()>>,
    signal: impl std::future::Future<Output = std::io::Result<()>>,
    deadline: tokio::time::Instant,
) -> Result<()> {
    tokio::select! {
        biased;
        signal = signal => {
            signal?;
            anyhow::bail!("interrupted; shell controls unavailable; reconnect required");
        }
        () = tokio::time::sleep_until(deadline) => anyhow::bail!("shell session timed out; reconnect required"),
        result = work => result,
    }
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
    #[tokio::test]
    async fn signal_wins_over_ready_output_and_stalled_sink_times_out() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(10);
        let result = until_deadline_or_signal(async { Ok(()) }, async { Ok(()) }, deadline).await;
        assert!(result.unwrap_err().to_string().contains("interrupted"));
        let (mut sink, _reader) = tokio::io::duplex(1);
        let work = async {
            tokio::io::AsyncWriteExt::write_all(&mut sink, b"blocked output").await?;
            Ok(())
        };
        let result = until_deadline_or_signal(work, std::future::pending(), deadline).await;
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}
