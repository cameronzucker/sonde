# Contributing to Sonde

Thanks for your interest in Sonde, a clean-sheet HF data modem (AGPLv3-only).

## Before you start

- Sonde **keys real radios** (`crates/sonde-tx`, the rig/PTT crates). Read the
  **live-radio / RADIO-1** section in [CLAUDE.md](CLAUDE.md) and
  [docs/pitfalls/implementation-pitfalls.md](docs/pitfalls/implementation-pitfalls.md):
  no automation, test, CI job, or agent initiates a transmission without the
  station licensee's explicit, per-invocation consent (a Part 97 requirement).
  Contributions must never run a transmit path automatically — write the code,
  let the licensee run it.
- The project's engineering discipline lives in [CLAUDE.md](CLAUDE.md) and
  [docs/git-strategy.md](docs/git-strategy.md), with decisions recorded under
  [docs/adr/](docs/adr/).

## Development

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings    # warnings are errors
cargo fmt --all --check
```

CI requires the system packages **`libasound2-dev`** and **`pkg-config`**
(cpal → alsa-sys links against ALSA). Install them before any cargo step on a
fresh host.

## Workflow

- **`main` is the integration branch** and is protected: changes land via a
  **pull request** with green CI, merged as a **no-fast-forward merge commit**
  (squash and rebase merges are disabled). Direct pushes to `main` are blocked.
  See [ADR 0002](docs/adr/0002-git-workflow-and-governance.md).
- Branch per unit of work; open a PR against `main`.
- All four gates above (build, test, clippy `-D warnings`, fmt) must pass.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org):
  `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `perf:`, `ci:`,
  `build:`. Scope to a crate where it helps (`feat(sonde-phy): …`).
- These messages drive the changelog and version bumps via release-please —
  see [VERSIONING.md](VERSIONING.md) and
  [ADR 0005](docs/adr/0005-semver-via-release-please.md). Match the `type:` to
  the real intent (the changelog depends on it).
- Breaking changes: add `!` and a `BREAKING CHANGE:` footer.

## Releases

Releases are automated. release-please opens a release PR from the accumulated
Conventional Commits; merging it cuts the tag, updates `CHANGELOG.md` and
`version.txt`. Do not hand-edit released changelog sections.

## License

By contributing, you agree your contributions are licensed under the project's
**AGPL-3.0-only** license.
