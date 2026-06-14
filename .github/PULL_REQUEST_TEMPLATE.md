<!-- PR title: use a Conventional Commit subject, e.g. `feat(sonde-phy): ...` -->

## Summary

<!-- What this PR does and why. Link the bd issue (e.g. sonde-xxx) if there is one. -->

## Type of change

- [ ] `feat` — new functionality
- [ ] `fix` — bug fix
- [ ] `docs` / `chore` / `ci` / `build` / `refactor` / `test` / `perf`
- [ ] Breaking change (`!` + `BREAKING CHANGE:` footer)

## Checklist

- [ ] `cargo build --workspace` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --all --check` clean
- [ ] Conventional Commit messages (they drive the changelog / release-please)
- [ ] If `CLAUDE.md` changed: `AGENTS.md` parity check done in this PR
- [ ] If a design decision was made: ADR added under `docs/adr/` (claim-next number per ADR 0004)
- [ ] Commits carry the `Agent: <moniker>` trailer (agent-authored work)

## Live-radio (RADIO-1)

- [ ] This PR does **not** add a path that transmits without explicit, per-invocation licensee consent, and no transmit path is exercised by automated tests/CI. (Tick if N/A.)
