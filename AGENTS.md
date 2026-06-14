# Sonde

> **Note:** This file is a **summary with links** into [CLAUDE.md](CLAUDE.md)
> for non-Claude agent harnesses (Codex, etc.). The substantive rules live in
> CLAUDE.md; this file points at them. When CLAUDE.md changes, update the
> summary line here only if the change is something a non-Claude agent reading
> just this file needs to see.

## Project framing

Sonde is a clean-sheet **HF data modem**, **AGPLv3-only**, structured as a Rust
**cargo workspace**. It is subordinate to the Tuxlink "clean-sheet modem"
program overview. Crates: `sonde-phy` (PHY waveform), `sonde-fec` (LDPC FEC),
`sonde-rx`, `sonde-tx` (**keys a real radio**), `sonde-rig-rts` (serial-RTS PTT),
`sonde-rig-cm108` (USB-HID PTT), plus the vendored `hf-channel-sim`. Coming soon:
`crates/sonde-phy-runtime` (the `SondePhy` `PhyTransport` adapter; bd epic
`sonde-gmc`; [ADR 0003](docs/adr/0003-sonde-phy-runtime-adapter.md)).
See [CLAUDE.md](CLAUDE.md#project-framing) for the full crate table.

## Commands

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

CI requires system packages **`libasound2-dev`** + **`pkg-config`** installed
before any cargo step (cpal → alsa-sys links against ALSA).

## Project ethos

Sonde is part of Cameron's learning-sandbox program: process rigor > raw
velocity; explain new workflows so the skill transfers; prefer patterns that
generalize to multi-developer / higher-stakes work. See
[CLAUDE.md](CLAUDE.md#project-ethos).

## Skill routing

When a request matches an available skill, invoke it via the Skill tool as your
FIRST action — don't answer directly. See [CLAUDE.md](CLAUDE.md#skill-routing).

## Agent identity

Generate a moniker via `python3 .claude/scripts/get_agent_moniker.py` at session
start (3-word hyphenated form, auto-pre-flighted against git history). Include
`Agent: <moniker>` as a commit trailer on every commit, in branch names
(`<bd-id>/<slug>`), and in PR titles. Pass the moniker through to every
subagent you dispatch. The env var is `SONDE_AGENT_MONIKER`. See
[CLAUDE.md](CLAUDE.md#agent-identity--pick-a-moniker-at-session-start).

## Git workflow: `main` is the integration branch (ADR 0002 deviation)

**Sonde deviates from Tuxlink:** Tuxlink integrates onto `feat/v0.0.1`; Sonde
uses **`main` as its integration branch** to match its existing CI (build/test on
push to `main`). Direct commits to `main` are **BLOCKED**. Feature work uses
per-task branches `<bd-id>/<slug>` → PR → `gh pr merge <#> --merge
--delete-branch` (no-ff merge-commit; no squash). The only carve-out for a local
commit on `main` is the rare integration merge-commit, prefixed
`ALLOW_INTEGRATION_COMMIT=1 git commit ...`. See
[CLAUDE.md](CLAUDE.md#git-workflow--main-is-the-integration-branch-adr-0002-deviation)
and [ADR 0002](docs/adr/0002-git-workflow-and-governance.md).

## Git workflow: worktrees mandatory under bd-issue ownership; destructive commands BANNED (ADR 0002)

Write work happens in worktrees, each bound to a bd issue. Create one with
`python3 .claude/scripts/new_sonde_worktree.py --slug <slug> --issue <sonde-bd-id>`;
worktrees live at `worktrees/<name>/` (gitignored). When
`.claude/hooks/block-main-checkout-race.sh` denies a write op citing "another
live session is active," create a worktree and re-run there — the hook is
authoritative; do not re-decide via `.claude/scripts/get_sonde_sessions.py`. The
disposal ritual + lease mechanism are in
[docs/git-strategy.md](docs/git-strategy.md). Destructive git commands are banned
regardless of worktree topology: no `reset --hard`, no force push, no `--amend`
on pushed commits, no `--no-verify`, no `git worktree remove`, no `git rebase
-i`. If you think you need a banned command, stop and ask. See
[CLAUDE.md](CLAUDE.md#git-workflow--worktrees-mandatory-under-bd-issue-ownership-adr-0002),
[CLAUDE.md](CLAUDE.md#git-workflow--destructive-commands-are-banned-adr-0002),
and [ADR 0002](docs/adr/0002-git-workflow-and-governance.md).

## Live radio operations: READ BEFORE ANY TRANSMISSION

**Sonde literally transmits** — `sonde-tx` keys a real radio via the PTT crates.
No automation, test, subagent, CI job, scheduled task, or AI agent initiates a
transmission under the station callsign without the licensee's **explicit,
scoped, per-invocation consent** at the moment of the run. This is **Part 97**
regulatory compliance, not a style rule. **If your task touches any TX code path
(`sonde-tx`, rig/PTT crates), do NOT run it — write the code, commit it, let the
licensee run it manually; if completion seems to require running a TX binary,
STOP and escalate.** Canonical rule: the **RADIO-1** entry in
[docs/pitfalls/implementation-pitfalls.md](docs/pitfalls/implementation-pitfalls.md).
See [CLAUDE.md](CLAUDE.md#live-radio-operations--read-before-any-transmission).

## Commit and release discipline

Conventional commit types (`feat:`, `fix:`, `docs:`, etc.); scope to a crate when
localized (`feat(sonde-phy): ...`). Breaking changes get `!` + `BREAKING CHANGE:`
footer. **Squash-merge is banned** ([ADR 0002](docs/adr/0002-git-workflow-and-governance.md));
PRs merge as no-ff merge-commits via `gh pr merge <#> --merge --delete-branch`.
**Polish before push:** clean up WIP commits via non-interactive `git rebase
<base>` on local un-pushed commits; once pushed, commits are immutable. See
[CLAUDE.md](CLAUDE.md#commit-and-release-discipline).

## Documentation propagation + AGENTS parity

Canonical policy/spec claims live in their ADR or spec; CLAUDE.md / AGENTS.md /
templates / pitfalls docs are pointers or operational recipes. Every PR that
changes a CLAUDE.md rule must perform the AGENTS.md parity check in the same PR;
update AGENTS.md when the non-Claude summary becomes inaccurate or a new
load-bearing non-Claude rule appears. See
[CLAUDE.md](CLAUDE.md#documentation-propagation-contract) and
[CLAUDE.md](CLAUDE.md#parity-with-agentsmd).

## Tool referee (overrides bd's CLAUDE.md defaults)

This project uses bd (Beads) AND harness-native in-turn planning primitives. They
are NOT substitutes. When bd's BEADS INTEGRATION section conflicts with project
commitments, the `## Tool referee` table in
[CLAUDE.md](CLAUDE.md#tool-referee--which-tool-owns-which-job) wins: the
harness-native in-turn planner handles micro-progress; bd handles cross-session
work; auto-memory at `~/.claude/projects/<slug>/memory/` is canonical for
user/feedback memory. Push at session end is mandatory (bd agrees).

## Session Completion

Work is not complete until `git push` succeeds AND a session-end handoff exists.
Required steps: (1) file issues for remaining work; (2) run quality gates if code
changed (`cargo build/test/clippy/fmt`); (3) update bd status; (4) `git push` —
mandatory, retry until it succeeds; (5) clean up stashes + delete remote task
branches; (6) write a handoff at `dev/handoffs/<YYYY-MM-DD>-<short-slug>.md`
enumerating branch + working-tree + worktree state; (7) surface the operator's
next-session starting prompt as your final message (~10-line paste-ready block:
one-sentence summary, handoff doc pointer, critical-first-action emphasis). See
[CLAUDE.md §Session Completion](CLAUDE.md#session-completion).

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
