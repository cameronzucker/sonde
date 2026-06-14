# Versioning

Sonde follows [Semantic Versioning 2.0.0](https://semver.org). Releases are cut
automatically by [release-please](https://github.com/googleapis/release-please)
from [Conventional Commit](https://www.conventionalcommits.org) messages. The
decision and rationale live in [ADR 0005](docs/adr/0005-semver-via-release-please.md);
this file is the operational policy.

## Two version namespaces (read this first)

Sonde tracks **two distinct kinds of version**, deliberately decoupled:

1. **Project version** — a single SemVer for the repository as a whole, stored in
   [`version.txt`](version.txt) and tagged `vX.Y.Z`. This is what release-please
   maintains and what [`CHANGELOG.md`](CHANGELOG.md) describes.
2. **Per-crate versions** — each crate's own `version` in its `Cargo.toml`
   (`sonde-phy`, `sonde-fec`, …). These are **not** managed by release-please and
   are bumped only when a crate is published to crates.io (a separate, future
   effort; the names are reserved but nothing is published yet).

The two can diverge — the project may be at `v0.3.0` while `sonde-phy` is still
`0.1.0`. That is intended pre-publish, not drift. When crates.io publishing
arrives, per-crate versioning gets its own ADR.

## How the project version bumps

While the project is pre-1.0 (`0.y.z`), release-please is configured with
`bump-minor-pre-major: true`, so:

| Commit type | Pre-1.0 effect | Post-1.0 effect |
|---|---|---|
| `feat:` | minor (`0.y`→`0.(y+1)`) | minor |
| `fix:` / `perf:` | patch | patch |
| `feat!:` / `BREAKING CHANGE:` | **minor** (pre-1.0 convention — breaking changes are expected in 0.x) | major |
| `docs:` `test:` `ci:` `build:` `chore:` `refactor:` | no release on their own | no release |

Pre-1.0, a breaking change bumps the **minor**, not the major — standard 0.x
semantics. The `1.0.0` cut is a deliberate, manual decision (open a `feat!:` or
hand-set the release-please PR) made when the PHY/FEC interfaces are considered
stable.

## Cutting a release

1. Land work on `main` via PRs with Conventional Commit messages (see
   [CLAUDE.md](CLAUDE.md) commit discipline).
2. release-please opens/updates a **release PR** automatically on each push to
   `main`, accumulating the changelog and the next version.
3. Merge the release PR. That tags `vX.Y.Z`, updates `version.txt` +
   `CHANGELOG.md`, and creates the GitHub release.

Do not hand-edit released `CHANGELOG.md` sections or `version.txt` — release-please
owns them.
