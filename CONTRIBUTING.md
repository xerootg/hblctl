# Contributing to hblctl

## Commits, versions and releases

- Commit messages and PR titles follow [Conventional Commits](https://www.conventionalcommits.org/)
  (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `ci:`, `chore:`; `!` or a `BREAKING CHANGE:` footer for breaking
  changes). CI checks PR titles.
- Versions follow [SemVer](https://semver.org/). [release-plz](https://release-plz.dev/) derives the next version from
  the commit history, keeps a release PR with the changelog, and tags the release when that PR is merged. Public API
  changes are checked with `cargo-semver-checks` on every PR.

## Development

Tests come first: add a failing test, then the code. Units are tested in isolation with mocks/stubs; transports are
tested against recorded sessions.

Before pushing, run what CI runs:

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo deny check
reuse lint
typos
```

## Licensing of contributions

hblctl is licensed under the **GNU Lesser General Public License v3.0 or later** (`LGPL-3.0-or-later`). Every source
file carries an SPDX header; the repository is [REUSE](https://reuse.software/)-compliant.

By submitting a contribution you agree that:

1. **You have the right to submit it.** You certify the
   [Developer Certificate of Origin 1.1](https://developercertificate.org/) and add a `Signed-off-by:` line to each
   commit (`git commit -s`).
2. **Inbound = outbound.** Your contribution is licensed under `LGPL-3.0-or-later`.
3. **Relicensing grant.** You additionally grant the project maintainer (Haley Grandle) a perpetual, worldwide,
   non-exclusive, royalty-free, irrevocable licence to use, modify and distribute your contribution under any other
   licence approved by the [Open Source Initiative](https://opensource.org/licenses) or the
   [Free Software Foundation](https://www.gnu.org/licenses/license-list.html). You keep your copyright. This lets the
   project change or add licences later (for example to a newer GNU licence, or a dual licence) without having to
   track down every contributor.

Dependencies must be compatible with distributing hblctl under `LGPL-3.0-or-later`; `cargo deny` enforces an allow
list (permissive licences, MPL-2.0 and LGPL-3.0-or-later). GPL-only dependencies are not accepted.

## Firmware and vendor material

Do not add vendor firmware, binaries, keys, disassembly listings or other copyrighted vendor material. Protocol facts
(layouts, numbers, behaviour) described in your own words are welcome.
