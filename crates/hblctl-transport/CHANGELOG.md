# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/xerootg/hblctl/releases/tag/hblctl-transport-v0.3.0) - 2026-10-31

### Added

- *(core)* named registry tables and typed production-config fields ([#11](https://github.com/xerootg/hblctl/pull/11))
- *(tui)* terminal UI generated from the command registry
- *(transport)* byte transports, framer and binary session capture

### Fixed

- *(transport)* [**breaking**] read camera replies as 1024-byte messages

### Other

- *(release)* v0.2.0 ([#7](https://github.com/xerootg/hblctl/pull/7))
- release v0.1.0 ([#5](https://github.com/xerootg/hblctl/pull/5))
- use assert_eq for emptiness checks
- remove unbuilt crate scaffolds
- [**breaking**] relicense to LGPL-3.0-or-later and set up project tooling

## [0.1.0](https://github.com/xerootg/hblctl/releases/tag/hblctl-transport-v0.1.0) - 2026-10-30

### Added

- *(tui)* terminal UI generated from the command registry

### Fixed

- *(transport)* [**breaking**] read camera replies as 1024-byte messages

### Other

- use assert_eq for emptiness checks
