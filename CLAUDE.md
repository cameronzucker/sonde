# Sonde

## Project framing

Sonde is a clean-sheet **HF data modem** — a software waveform that transmits
and receives digital data over high-frequency (shortwave) radio. It is
**AGPLv3-only** and structured as a Rust **cargo workspace**.

Sonde is **subordinate to the Tuxlink "clean-sheet modem" program overview**
(`docs/superpowers/specs/2026-05-31-clean-sheet-modem-overview.md` in the
tuxlink repo); that program document is the parent intent, and Sonde implements
the modem subsystems under it. Per ADR 0014 (recorded in tuxlink), the design is
clean-sheet: no examination of VARA / ARDOP / FLDigi / Trimode / Pat / wl2k-go
internals. Conceptual primitives are drawn from open foundations documented in
`docs/research/modem-foundations.md`.

### Workspace structure

| Crate | Role |
|---|---|
| `crates/sonde-phy` | PHY waveform (modulation / demodulation) |
| `crates/sonde-fec` | LDPC forward error correction |
| `crates/sonde-rx` | Receive pipeline |
| `crates/sonde-tx` | Transmit pipeline — **keys a real radio** (see live-radio section) |
| `crates/sonde-rig-rts` | Serial-RTS PTT (push-to-talk) keying |
| `crates/sonde-rig-cm108` | USB-HID (CM108) PTT keying |
| `hf-channel-sim` | Vendored workspace member — HF channel simulator |

**Coming soon:** `crates/sonde-phy-runtime` — the `SondePhy` `PhyTransport`
adapter — tracked under bd epic `sonde-gmc`, with the implementation plan at
[`docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md`](docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md)
and architecture in [ADR 0003](docs/adr/0003-sonde-phy-runtime-adapter.md).

### Commands

```bash
cargo build --workspace                                  # build
cargo test  --workspace                                  # test
cargo clippy --workspace --all-targets -- -D warnings    # lint (warnings are errors)
cargo fmt --all --check                                   # format check
```

CI requires the system packages **`libasound2-dev`** and **`pkg-config`** to be
installed before any cargo step (cpal → alsa-sys links against ALSA). Install
them first on any fresh build host.

## Project ethos

Sonde is part of Cameron's learning-sandbox program for AI-assisted development
techniques — the same discipline as its sibling repo Tuxlink. The shipped modem
matters, but **professional-development outcomes are a first-class goal
alongside features.**

Implications:

- **Process rigor > raw velocity.** Do the right thing, not the fast thing.
- **Explain** new workflows (the what and the why) so Cameron builds
  transferable skill that carries to higher-stakes environments.
- Prefer patterns that generalize to multi-developer / multi-agent work.
- Signal professional polish — clean commits, honest CI, disciplined branches —
  because the surface area of the repo teaches what "good" looks like.

## Skill routing

When the user's request matches an available skill, ALWAYS invoke it using the
Skill tool as your FIRST action. Do NOT answer directly, do NOT use other tools
first. The skill has specialized workflows that produce better results than
ad-hoc answers.

Key routing rules:

- Product ideas, "is this worth building", brainstorming → invoke office-hours
- Bugs, errors, "why is this broken" → invoke investigate
- Ship, deploy, push, create PR → invoke ship
- Code review, check my diff → invoke review
- Update docs after shipping → invoke document-release
- Weekly retro → invoke retro
- Architecture review → invoke plan-eng-review
- Save progress, checkpoint, resume → invoke checkpoint
- Code quality, health check → invoke health

## Agent identity — pick a moniker at session start

**At the very start of every session** (after reading CLAUDE.md and the
most-recent handoff, before taking any action on the repo), generate a moniker
via the script and state it in your first user-facing message:

```bash
python3 .claude/scripts/get_agent_moniker.py
```

This draws 3 words without replacement from a pool of plant / animal /
geographic nouns and hyphen-joins them (e.g. `towhee-wren-aspen`). The script
pre-flights against `git log --all --grep="^Agent: <candidate>"` automatically;
on a detected collision it retries up to `--max-attempts` before giving up.

The moniker:

- Is the hyphen-joined three-word form (any single-word legacy monikers in
  commit history remain valid; the new format applies to forward commits).
- Is **ctrl+F-friendly** by construction (the pool excludes common code
  identifiers and human first names).
- Persists for the entire session — do not change it mid-session.
- Passes through to every subagent you dispatch: include
  `"You are agent <moniker>; use this in your commit trailers."` in each Agent
  tool prompt so subagent-authored commits are grep-discoverable too.

**Include the moniker in every git action as a commit trailer:**
`Agent: <moniker>` on its own line, alongside the `Co-Authored-By:` trailer.

```
<subject>

<body paragraphs>

Agent: juniper-finch-mesa
Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
```

The moniker also belongs in the branch name (`<bd-id>/<slug>` — see the
git workflow below) and in any PR title you open (`[<moniker>] <subject>`).

**Why:** triage + forensics. When a session goes sideways, Cameron needs to grep
the commit graph for "which agent did this" without reconstructing it from
timestamps. `git log --grep="^Agent: <moniker>"` returns the full trail for a
session; `git log --all --grep="^Agent:"` enumerates every agent that has
touched the repo.

**If you forget to set a moniker early in the session:** pick one now and apply
it to all forward commits. Do not retroactively amend earlier commits (amending
shared/pushed commits is banned — see below).

## Git workflow — `main` is the integration branch (ADR 0002 deviation)

**Sonde deviates from Tuxlink's branch topology.** Tuxlink integrates onto a
`feat/v0.0.1` branch; Sonde uses **`main` as its integration branch**, to match
the existing CI (which builds and tests on every push to `main`). This deviation
is recorded in [ADR 0002](docs/adr/0002-git-workflow-and-governance.md).

What this means in practice:

- **Direct commits to `main` are BLOCKED.** Do not commit work directly to
  `main`.
- **Feature work uses per-task branches** named `<bd-id>/<slug>` (the bd
  issue ID is the prefix), then opens a PR, then merges with:

  ```bash
  gh pr merge <#> --merge --delete-branch
  ```

  This is a **no-fast-forward merge-commit** — no squash, no rebase-onto-main
  flattening. The integration branch preserves every task-branch commit.
- **The only carve-out for a local commit on `main`** is the rare merge-commit
  itself, and it must be prefixed with the explicit acknowledgement env var:

  ```bash
  ALLOW_INTEGRATION_COMMIT=1 git commit ...
  ```

  Use this only for the integration merge-commit, never for feature work.

## Git workflow — worktrees mandatory under bd-issue ownership (ADR 0002)

Write work happens in **worktrees**, each bound to a bd issue. When the
`.claude/hooks/block-main-checkout-race.sh` hook denies a write op citing
"another live session is active," create a worktree per the QUICK FIX in the
deny message and re-run your op there. **The hook's determination is
authoritative;** agents do not re-decide it via `.claude/scripts/get_sonde_sessions.py`
or any other source. Read-only ops and `bd` commands stay free regardless.

**Create a worktree with the Sonde script:**

```bash
python3 .claude/scripts/new_sonde_worktree.py --slug <slug> --issue <sonde-bd-id>
```

Worktrees live at `worktrees/<name>/` at the repo root (`worktrees/` is
`.gitignore`d).

**Worktree ownership rule.** A worktree is permitted IFF:

1. A **bd issue** is `in_progress` and claims the worktree (`bd show <id>` is the
   canonical answer to "what is `worktrees/X` for?").
2. The branch follows the per-task convention `<bd-id>/<slug>`.
3. The worktree path is `worktrees/<bd-id-or-slug>/` at the repo root.
4. The session adheres to all other CLAUDE.md rules (moniker, commit discipline,
   destructive-git ban, session-end handoff).

A worktree without a bd-issue claim is an anti-pattern: either retroactively
claim it with a bd issue, or dispose of it via the disposal ritual.

**Pattern A (harness-spawned ephemeral worktrees** — the `Agent` tool's
`isolation: "worktree"` parameter) is uncontroversially permitted; the harness
manages create + dispose, no per-worktree bd issue required.

**Worktree disposal ritual + lease mechanism** (inventory → propagate-or-archive
→ physical remove → `git worktree prune`) and the session-lease mechanism are
documented in [`docs/git-strategy.md`](docs/git-strategy.md). `git worktree
remove` is banned (the destructive-git hook denies it); use the ritual. The full
rationale, per-task branch model, no-squash rule, destructive-git ban, and the
main-as-integration deviation all live in
[ADR 0002](docs/adr/0002-git-workflow-and-governance.md).

## Git workflow — destructive commands are BANNED (ADR 0002)

The [`.claude/hooks/block-destructive-git.sh`](.claude/hooks/block-destructive-git.sh)
hook denies destructive git operations at the harness layer. **The hook is the
canonical enforcement; do not work around it.** If a hook denial surprises you,
find a non-destructive alternative — never `--no-verify`, never an end-run.

Quick reference (the hook source is the authoritative list, not this list):

- `git reset --hard <ref>` — use `git revert <commit>` or restore named files.
- `git push --force` / `-f` / `--force-with-lease` — open a new PR or ask.
- `git checkout -- .` / `git restore .` / `git clean -f` — name files explicitly.
- `git branch -D` / `--delete --force` — use `-d`, which refuses unmerged.
- `git commit --amend` on pushed or other-authored commits — create a new commit.
- `git rebase -i` / `--interactive` — banned; use `git rebase <base>` for
  non-interactive linear replays of local un-pushed commits.
- `git worktree remove` — use the disposal ritual in `docs/git-strategy.md`.
- `git reflog expire --expire=now` / `git gc --prune=now` — strips the recovery
  safety net.
- `git filter-branch` / `git filter-repo` — mass history rewrite.
- `--no-verify` / `--no-gpg-sign` — bypasses the project's gates.

**Why hooks, not just prose:** prose-documented rules have been violated by
agents before (the cross-program record includes a subagent running `git reset
--hard` that wiped shipped commits, recovered only via reflog). Prose alone does
not prevent it; the hook layer does. **If you think you need a banned command:**
stop and surface the situation to the user with a proposed non-destructive
alternative. Full rationale: [ADR 0002](docs/adr/0002-git-workflow-and-governance.md).

## Live radio operations — READ BEFORE ANY TRANSMISSION

**Sonde literally transmits.** `crates/sonde-tx` keys a real radio over a real
antenna via the PTT crates (`sonde-rig-rts`, `sonde-rig-cm108`). This is not a
simulation-only project.

No automation, test, subagent, CI job, scheduled task, or AI agent initiates a
transmission under the station's amateur callsign without the **station licensee
giving explicit, scoped, per-invocation consent at the moment of the run.**
Cached credentials, stored env vars, repo secrets, and "the user said yes last
week" are NOT consent.

This is a **Part 97 regulatory requirement**, not a style guideline. The
canonical rule and consent-gate protocol live in the **RADIO-1** entry in
[docs/pitfalls/implementation-pitfalls.md](docs/pitfalls/implementation-pitfalls.md).

**Subagent rule:** if your task touches any code path that could transmit
(anything under `sonde-tx` or the rig/PTT crates), **refuse to run it in your
shell.** Write the code, commit it, and let the licensee run it manually. If
your task seems to require running a TX binary to verify completion, your task is
misspecified — STOP and escalate.

## Commit and release discipline

- Use conventional commit types: `feat:`, `fix:`, `docs:`, `refactor:`,
  `test:`, `chore:`, `perf:`, `ci:`, `build:`. Match the `type:` to the actual
  intent — never `fix:` for docs, never `feat:` for an internal refactor.
- Prefer scoped commits (`feat(sonde-phy): ...`, `fix(sonde-tx): ...`) when the
  change is localized to one crate.
- Breaking changes: add `!` and a `BREAKING CHANGE:` footer with a one-line
  user-facing explanation.
- **Polish before push.** Per [ADR 0002](docs/adr/0002-git-workflow-and-governance.md),
  squash-merge is banned, so the integration branch preserves every task-branch
  commit. Clean up WIP / fixup / "oops" commits via **non-interactive**
  `git rebase <base>` on **local un-pushed** commits before `git push`. Once
  pushed, commits are immutable (the destructive-git ban on `--amend` of pushed
  commits and on `git rebase -i` enforces this). The push gates the polish.

## Documentation propagation contract

For any project-policy claim — an ADR, a spec section, an operator decision — the
**canonical source is the ADR or spec itself.** CLAUDE.md, AGENTS.md, plan
templates, pitfalls docs, and memory entries are **pointers**, not parallel
statements.

**Maximum three propagation sites per ADR:**

1. The ADR itself (always).
2. The spec section it amends, if any.
3. One operational doc — CLAUDE.md OR plan template OR pitfalls — pick one.

Memory entries cite the ADR; they do not restate it. Narrowly-scoped operational
recipes that are inherently a how-to (e.g., the worktree-disposal ritual
step-by-step) MAY live where the operator will look for them. The rule is "don't
restate what the spec/ADR already says," not "don't write recipes."

**Why:** without this contract, ADRs and CLAUDE.md drift apart — the same rule
appears in three places with slightly different wording, one place is updated,
the others rot. The propagation contract makes the ADR/spec the single canonical
source.

## Parity with `AGENTS.md`

[AGENTS.md](AGENTS.md) is a deliberate **summary with links** to this file's
sections, intended for non-Claude agent harnesses (Codex CLI and any tooling
that picks up the standard `AGENTS.md` convention) where pulling the whole
CLAUDE.md inline would be wasteful. It is NOT a full mirror; the substantive
rules live here and AGENTS.md points to them.

**Upkeep discipline.** Every PR that changes a rule in CLAUDE.md MUST also do the
AGENTS.md parity check, in the same PR:

1. Locate the AGENTS.md section that summarizes the CLAUDE.md section you changed.
2. If the change is purely additive (clarification, expanded example, new link)
   AND the AGENTS.md summary line is still accurate, no AGENTS.md update needed.
3. If the change adds, removes, or renames a CLAUDE.md section, OR alters the
   load-bearing summary AGENTS.md was providing, update AGENTS.md in the same PR.
4. If a CLAUDE.md change introduces a load-bearing rule for non-Claude agents and
   no AGENTS.md section summarizes it, add one.

Drift between CLAUDE.md and AGENTS.md is a defect — it violates the propagation
contract above. **When in doubt, ship the AGENTS.md update alongside the
CLAUDE.md change.**

## Tool referee — which tool owns which job

This project uses both Claude Code's built-in primitives (TodoWrite,
auto-memory) and `bd` (Beads). They serve overlapping but **non-redundant**
roles. When `bd`'s auto-managed section below (`<!-- BEGIN BEADS INTEGRATION -->`)
prescribes a rule that conflicts with the table here, **the table wins.**

| Concern | Owns it | Notes |
|---|---|---|
| Cross-session task tracking with deps | `bd` | Primary. Use `bd ready` / `bd update --claim` / `bd close`. |
| In-turn micro-progress within one session | TodoWrite | Claude Code primitive; ephemeral; correct for "read X, edit Y, run Z" lists. |
| User profile + cross-cutting feedback | Auto-memory at `~/.claude/projects/<slug>/memory/` | Harness-native, auto-loaded each session via `MEMORY.md` index. Do not migrate to bd. |
| Issue-adjacent factoids discovered during a task | `bd remember` | Knowledge linked to a specific issue. Cross-project user/feedback stays in auto-memory. |
| Branch model | Per-task branch + merge-commit (no-ff) onto `main` | See [ADR 0002](docs/adr/0002-git-workflow-and-governance.md). |

**Specific overrides of bd's BEADS INTEGRATION block:**

- bd says *"do NOT use TodoWrite, TaskCreate, or markdown TODO lists"* →
  **Override:** TodoWrite is the right primitive for in-turn working memory; bd
  is the right primitive for cross-session work units. Use both, for their
  respective layers.
- bd says *"Use `bd remember` for persistent knowledge — do NOT use MEMORY.md
  files"* → **Override:** the Claude Code auto-memory directory at
  `~/.claude/projects/<slug>/memory/` is harness-native and remains canonical
  for user / feedback / project memory. Use `bd remember` for
  issue-tracker-adjacent factoids only.
- bd says *"Work is NOT complete until `git push` succeeds … YOU must push"* →
  **Not overridden.** Push is mandatory at session end per
  [§Session Completion](#session-completion); bd agrees with project policy here.

**If you discover a fourth bd directive that conflicts with project
commitments:** extend the table above and surface it. Do NOT silently soften an
override.

## Session Completion

Work is not complete until `git push` succeeds AND a session-end handoff document
exists.

**Required steps before ending any session:**

1. File issues for remaining work discovered during the session (`bd create ...`).
2. Run quality gates if code changed (`cargo build/test/clippy/fmt` —
   warnings are errors).
3. Update issue tracker status (`bd close <id>` / `bd update <id>`).
4. **`git push`** — mandatory. If push fails, resolve the failure and retry
   until it succeeds. Do NOT stop before pushing.
5. Clean up: clear stashes; ensure remote task branches are deleted
   (`gh pr merge --delete-branch` handles this for landed PRs; manual
   `git push origin --delete <branch>` for branches that didn't reach merge).
6. Write a session-end handoff document to
   `dev/handoffs/<YYYY-MM-DD>-<short-slug>.md` enumerating: branch state,
   working-tree state, in-flight worktrees + their untracked + gitignored-
   stateful content, what was completed, what is in-progress, what is pending
   decision.
7. **Surface the operator's next-session starting prompt** as your final
   user-facing message of the session, AFTER step 6's handoff is committed. It is
   a concise (~10-line) paste-ready code block the operator copies into a fresh
   session's first message. Include:
   - One sentence framing what happened this session.
   - A pointer to the canonical handoff doc by path.
   - The **critical first action or gate** the next session must not skip.

   Format: a single fenced markdown code-block the operator can copy whole.

**Never say "ready to push when you are."** Push is the session's responsibility,
not the operator's. The handoff document closes the context loop so the next
session — possibly on a different machine — can continue without manual
reconstruction from `git log`.

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
