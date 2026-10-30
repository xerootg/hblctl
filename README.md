# hblctl

A client for the factory diagnostic ("sutest") protocol of Hasselblad X-system cameras, starting with the
original **X2D 100C on firmware 4.2.0**. One command registry drives three frontends: a CLI, a TUI, and an MCP
server for AI agents, so every command is available everywhere with no per-frontend code.

> **Status: early.** The protocol was derived from static analysis of the 4.2.0 firmware and cross-checked against
> public tools. It has not yet been run against a camera by this project.

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
