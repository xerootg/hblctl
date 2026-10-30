# hblctl

A client for the factory diagnostic ("sutest") protocol of Hasselblad X-system cameras, starting with the
original **X2D 100C on firmware 4.2.0**. One command registry drives three frontends: a CLI, a TUI, and an MCP
server for AI agents, so every command is available everywhere with no per-frontend code.

> **Status: under construction.** The protocol specification is being derived from static analysis of the 4.2.0
> firmware. Nothing here has been run against a camera yet.

## What the protocol is

The camera runs a root factory-test service (`camera-test`). `camera-gui` exposes it on **TCP port 30303** over the
camera's Wi-Fi hotspot and its USB network link, relaying fixed 252-byte records over DJI's internal message bus.
Commands include reading and writing factory configuration, running shell commands as root, file transfer and
calibration. **There is no authentication.** Anyone who can reach the port can run commands as root.

## Safety

Some commands can permanently damage a camera (factory configuration, calibration data, camera type, firmware
slots). `hblctl` classifies every command as `read`, `volatile`, `persistent` or `destructive` and refuses anything
above `read` unless you opt in explicitly. Use it only on a camera you own, and keep backups.

## Logging

Every session records the raw socket bytes (binary capture) and a protocol-level event log, so a failed run can be
inspected, replayed, and turned into a regression test.

## Licence

Dual-licensed under MIT or Apache-2.0, at your option. Not affiliated with Hasselblad or DJI. This repository
contains no vendor firmware or code; protocol facts were derived independently.

Prior public work: [WeiCheng97/Hasselblad-X2d-series-5g-unlock](https://github.com/WeiCheng97/Hasselblad-X2d-series-5g-unlock)
(diagnostic channel and frame format), [derrickyau9/Hasselblad-X2DII-ADB-Enable](https://github.com/derrickyau9/Hasselblad-X2DII-ADB-Enable)
(HblShell route on X2D II).
