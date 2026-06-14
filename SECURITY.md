# Security Policy

## Reporting a vulnerability

Security issues affecting Sonde should be reported **privately**, not via public
GitHub issues.

Two private channels are accepted:

1. **GitHub private security advisory** (preferred) —
   <https://github.com/cameronzucker/sonde/security/advisories/new>. This creates
   a draft advisory visible only to the reporter and the maintainer until
   disclosure.
2. **Email** — <cameronzucker@gmail.com> with the subject prefix
   `[sonde security]`.

Please include:

- A clear description of the issue and its impact.
- Reproduction steps (crate + version, OS, and a **callsign-redacted** config or
  capture snippet if relevant — Sonde drives real radios; do not include
  identifying station data).
- Any proof-of-concept, redacted of sensitive content.

A response is provided within **7 calendar days** acknowledging receipt and
giving an initial assessment. Resolution timelines depend on severity but follow
industry norms (90 days from initial report for non-critical, fewer for
critical).

## Scope

Sonde is an HF data modem that **keys real transmitters** (`crates/sonde-tx`,
the rig/PTT crates). Reports that are especially in scope:

- Memory-safety or panic-to-DoS issues in the demodulation/decode path
  (attacker-controlled RF/audio input reaching `sonde-rx` / `sonde-phy`).
- Any path that could cause an **unintended or unattended transmission**, or
  that weakens the RADIO-1 consent gate (see
  [docs/pitfalls/implementation-pitfalls.md](docs/pitfalls/implementation-pitfalls.md)).
  Unlicensed/unattended transmission is a regulatory (Part 97) hazard, not just
  a software bug.
- Supply-chain issues in the build (CI workflow injection, dependency
  compromise).

## Supported versions

Sonde is pre-1.0 and under active development. Only the latest `main` /
most-recent release receives security fixes; there are no maintained release
branches yet.
