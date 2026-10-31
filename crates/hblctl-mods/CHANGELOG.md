# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/xerootg/hblctl/releases/tag/hblctl-mods-v0.3.0) - 2026-10-31

### Added

- *(core)* named registry tables and typed production-config fields ([#11](https://github.com/xerootg/hblctl/pull/11))
- *(mods)* bundle engine with journaled apply, rollback and revert

### Fixed

- *(android)* publish release APKs reliably and sign them with a stable key
- *(mods)* drop the now-unused File import
- *(mods)* no unused import on non-Unix targets
- *(mods)* directory fsync is Unix-only
- *(mods)* ship the bundle schema in the crate; make preview-bundle tests optional

### Other

- *(release)* v0.2.0 ([#7](https://github.com/xerootg/hblctl/pull/7))
- release v0.1.0 ([#5](https://github.com/xerootg/hblctl/pull/5))

## [0.1.0](https://github.com/xerootg/hblctl/releases/tag/hblctl-mods-v0.1.0) - 2026-10-30

### Fixed

- *(mods)* drop the now-unused File import
- *(mods)* no unused import on non-Unix targets
- *(mods)* directory fsync is Unix-only
- *(mods)* ship the bundle schema in the crate; make preview-bundle tests optional
