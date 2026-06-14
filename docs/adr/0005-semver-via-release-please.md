# 0005. Project-level SemVer via release-please (simple release-type)

Date: 2026-06-14
Status: Accepted
Deciders: opossum-magnolia-taiga

## Context

Sonde is now a public repository and needs a disciplined, low-friction way to cut versioned releases with a maintained changelog, rather than hand-curating versions and release notes. Its sibling Tuxlink (private) solved this with [release-please](https://github.com/googleapis/release-please) driving a project-level version from Conventional Commit messages.

Sonde differs from Tuxlink in one way that matters here: it is a **cargo workspace of seven crates** with independent `Cargo.toml` versions (`sonde-phy` at 0.1.0, the rest at 0.0.1), **none yet published to crates.io**. Two questions follow: (1) should release automation manage the per-crate manifest versions, and (2) what is "the version" of Sonde as a project?

release-please offers a `rust` release-type that bumps each crate's `Cargo.toml` and maintains per-crate changelogs. Adopting it now would start rewriting all seven manifests on every release and tie the release cadence to crate-publish mechanics that do not yet exist (nothing is published; crate names are only reserved). That is premature noise.

## Decision

Adopt release-please with the **`simple` release-type**, a single package at the repo root, mirroring Tuxlink's configuration:

- A **project-level SemVer** is tracked in `version.txt` (seeded at `0.0.0`) and in `.github/.release-please-manifest.json`. The first release-please run proposes `0.1.0` from the accumulated `feat:` history (`bump-minor-pre-major: true`).
- `CHANGELOG.md` is maintained automatically from Conventional Commit types; `feat`/`fix`/`perf`/`refactor` are shown, the rest hidden.
- Releases are tagged `vX.Y.Z` (`include-v-in-tag: true`).
- The `.github/workflows/release-please.yml` workflow opens/updates a release PR on each push to `main`; merging it cuts the release.
- This project version is **decoupled from the per-crate `Cargo.toml` versions**. Those are bumped only when (and if) a crate is published to crates.io — a separate, future effort (the names are reserved per the extraction plan's Op B7). When that happens, per-crate versioning gets its own ADR (it may supersede this one's "simple" choice for the published crates).

Versioning policy (pre-1.0 semantics, how commit types map to bumps) is documented in `VERSIONING.md`; this ADR is the canonical decision record it points to.

## Consequences

- Releasing is one action: merge the release PR release-please maintains. Changelog and tag are generated; no hand-curation.
- The project gains a clean, citable SemVer line immediately, without touching the crate manifests or fighting the not-yet-published crate versions.
- There is a deliberate gap: the project version and the crate versions can diverge (e.g. project `v0.3.0` while `sonde-phy` is still `0.1.0`). This is intended pre-publish and is documented in VERSIONING.md so it does not read as drift.
- When crates.io publishing arrives, this decision must be revisited — likely a `rust`-type or per-crate scheme for the published crates, recorded as a new ADR.

## Alternatives considered

- **`rust` release-type, per-crate versions + changelogs.** More faithful to a publishable workspace, but premature: it rewrites seven manifests per release and couples release cadence to crate-publish that does not exist yet. Deferred to the crates.io-publish effort.
- **Manual versioning** (hand-edit `version.txt`/CHANGELOG, tag by hand). Rejected — exactly the error-prone, drift-prone process release-please exists to remove, and inconsistent with the "process rigor" ethos.
- **No project version, rely on crate versions only.** Rejected — leaves the repo with no single citable release line and no automated changelog while the crates remain unpublished and individually versioned.
