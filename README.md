# hblctl

A client for the factory diagnostic ("sutest") protocol of Hasselblad X-system cameras, starting with the
original **X2D 100C on firmware 4.2.0**. One command registry drives four frontends: a CLI, a TUI, an MCP
server for AI agents, and an Android app, so every command is available everywhere with no per-frontend code.

> **Status: early.** The protocol was derived from static analysis of the 4.2.0 firmware and cross-checked against
> public tools. Read-only commands have been verified against one original X2D 100C on 4.2.0 (see
> [Connecting to the camera](#connecting-to-the-camera)); everything else is still unverified.

## Use

```
hblctl ops                                    # every operation with its safety class
hblctl firmware-version build-version         # read-only operations run by default
hblctl --allow volatile wifi-power get        # anything above `read` must be allowed explicitly
hblctl tui                                    # interactive operation list, forms, results and protocol log
hblctl mcp                                    # MCP server on stdio (read-only tools unless --allow is given)
hblctl capture dump <session>/capture.hblcap  # decode a recorded session
```

## What the protocol is

The camera runs a root factory-test service (`camera-test`). The bridge `msg2dbus` listens on **TCP port 30303** on
every interface (the camera's Wi-Fi hotspot and its USB network link) and relays test messages to it over D-Bus.

- A request is one 257-byte message: `05 00 09 05 fc` (signal 5, from node 9 to node 5, type 0xFC) followed by a
  252-byte record: `u32 cmd | u32 func | u32 cookie | u32 CRC-16/XMODEM of bytes 0x10..0xFC | 236 bytes of data`.
- Every reply is a 1024-byte message: `09 00 05 09 fc`, the 252-byte result record, then filler.
- The camera treats each TCP read as one message, so each request goes out in a single write, one at a time.
- **One connection at a time:** a new connection replaces the previous one, so two hblctl instances (for example the
  CLI and an MCP server) disconnect each other.
- **There is no authentication.** Anything that can reach the port can use every command, including ones that run
  programs as root.

The command set (43 commands) is described as data in
[`crates/hblctl-core/registry/x2d-4.2.0.json`](crates/hblctl-core/registry/x2d-4.2.0.json); operations whose layouts
are not fully established are marked `verified: false` and show `(layout unverified)` in the CLI.

In the TUI, type to filter operations, use ↑/↓ to select, and Tab to switch to the generated form.
Type/backspace edits a field; Enter validates and runs. PgUp/PgDn scroll the result. Esc cancels confirmation
or clears the filter, Ctrl-R reconnects, and Ctrl-C exits. The connection opens on the first operation.
Destructive operations require typing their full `command.function` name after allowing that safety class.

## Android

The Jetpack Compose app renders operations and forms from the same registry through UniFFI.
It records sessions and supports capture viewing and ZIP export. See [android/README.md](android/README.md)
for prerequisites, native binding generation, and APK build/test commands.

## Connecting to the camera

Verified on an original X2D 100C, firmware 4.2.0, over USB (2026-10-31):

- Plug the camera into a **root USB port** of the computer (a Dell dock dropped the link).
- At the camera's connection prompt choose **tether**, not Skip. The camera then drops and re-enumerates the USB
  link once (about ten seconds after it first appears) and from then on holds it; with the camera setting *Power off
  while tethered* set to *Never* it stays up indefinitely. Skip, or leaving the prompt unanswered, ends the link.
  Nothing on the computer needs to speak Phocus.
- Wait for that second enumeration before connecting; a connection attempt during the drop fails with
  `connecting to 192.168.42.2:30303`.
- The camera serves DHCP on its USB network interface: the computer gets `192.168.42.1`, the camera is
  `192.168.42.2`, and the diagnostic port is `30303`, which is `hblctl`'s default. No mass-storage mode is needed.
- The camera's hotspot is the other transport the port is reachable over.

First exchange recorded as a replay test: `crates/hblctl-core/tests/fixtures/x2d-4.2.0-firmware-version.hblcap`.

## Safety

Some commands can permanently damage a camera (factory configuration, calibration data, camera type, firmware
slots). `hblctl` classifies every command as `read`, `volatile`, `persistent` or `destructive` and refuses anything
above `read` unless you opt in explicitly. Use it only on a camera you own, and keep backups.

## Logging

Every session records the raw socket bytes (binary capture) and a protocol-level event log, so a failed run can be
inspected, replayed, and turned into a regression test.

## Licence

[LGPL-3.0-or-later](COPYING.LESSER) (which incorporates the [GPL-3.0](COPYING)). Contributions are accepted under the
same licence plus a relicensing grant; see [CONTRIBUTING.md](CONTRIBUTING.md). Not affiliated with Hasselblad or DJI.
This repository contains no vendor firmware or code; protocol facts were derived independently.

Prior public work: [WeiCheng97/Hasselblad-X2d-series-5g-unlock](https://github.com/WeiCheng97/Hasselblad-X2d-series-5g-unlock)
(diagnostic channel and frame format), [derrickyau9/Hasselblad-X2DII-ADB-Enable](https://github.com/derrickyau9/Hasselblad-X2DII-ADB-Enable)
(HblShell route on X2D II).

## Mods

`hblctl-mods` consumes declarative bundles as passive data. It never executes bundled
source or downloads patch tools. A pinned JSON Schema and semantic checks validate
references, dependency cycles, ordered steps, literal paths, artifact hashes and
`byte-edits-v1` recipes. Loading requires a companion `<bundle>.sha256` sidecar and
an exact archive inventory. SHA-256 detects corruption; it does not authenticate a
publisher. The current toolkit preview bundle is marked blocked and cannot authorize changes;
its loading test pins the locally supplied fixture checksum.

```
hblctl mods load x2d-toolkit-bundle-<version>.tar.zst   # with its .sha256 sidecar beside it
hblctl --allow destructive mods plan bundle.tar.zst --via adb afc
hblctl --allow destructive mods apply bundle.tar.zst --via adb afc        # dry run
hblctl --allow destructive mods apply bundle.tar.zst --via adb afc --go
hblctl --allow destructive mods status bundle.tar.zst --via adb --recovery-session SESSION
hblctl --allow destructive mods revert bundle.tar.zst --via adb --all --recovery-session SESSION --go
```

The engine checks model, firmware, exact stock hashes, symlink ancestors, mount
identity, dependencies and conflicts before writing. Effective safety is the maximum
of the step effect, declared transport floor and backend floor. Rollback policy is
checked before installation too. ADB shell operations have a destructive floor,
even reads, and execution requires typing `mods.apply` or `mods.revert` after `--go`.
The diagnostic file-transfer commands are unverified; there is no diagnostic backend.

Original bytes and modes are recorded in a private, fsynced session journal with
separate host backups and verified on-camera toolkit backups. Journal evidence
precedes each step. Payloads precede startup hooks, writes are read back, and the
original read-only mount is checked before commit. Ordinary failures attempt
rollback; transport loss retains evidence and requires explicit recovery with the
original `--recovery-session`. Recovery accepts exact recorded bytes or a proven
interrupted prefix and refuses unknown changes. Shared files survive partial revert
while another selected feature owns them. Every step, hash, backup and journal write
is traced under `hblctl::mods` into the session event log. Reboot is required after
install and restore and remains an explicit owner action.

**Camera execution remains gated:** ADB file operations exist, but production runtime
withdrawal, service readiness and USB acceptance checks do not yet have reviewed
fixed commands. ADB apply/revert therefore refuse before mutation. MemoryFs exercises
the transaction engine with write failures and mid-write power loss. No physical
camera acceptance or post-reboot upgrade verification is claimed.
