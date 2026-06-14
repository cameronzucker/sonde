# 2. Git workflow and governance: main-as-integration, per-task branches, no-squash merge, mandatory worktrees, destructive-git ban

Date: 2026-06-14
Status: Accepted
Deciders: cameronzucker, sonde-ge9

## Context

Sonde is adopting the engineering discipline of its sibling Tuxlink (private sibling repo). Tuxlink arrived at its git governance over several ADRs and a two-week safety-stack sprint (LFST, May 2026): a per-task-branch model (its ADR 0004), worktrees mandated under bd-issue ownership (its ADR 0008), a worktree disposal ritual (its ADR 0009), and a no-squash-merge rule (its ADR 0010), all backed by harness-level hooks. The motivating incidents are real: the 2026-04-20 Geographica `git reset --hard` that wiped seven commits (recovered only via `git reflog` inside the 14-day `git gc` window), and the LFST `musing-bhabha` worktree-disposal incident that permanently lost untracked content via `git worktree remove`. Prose alone did not prevent these; the hook layer does.

Rather than spread these decisions across four ADRs as Tuxlink did, Sonde consolidates them into this single governance ADR — Sonde is a younger, smaller workspace and a single record is easier to onboard against. The substance is Tuxlink's; the packaging is consolidated, and the operational step-by-step lives in [`docs/git-strategy.md`](../git-strategy.md) so this ADR stays the *why* and the playbook stays the *how*.

There is one deliberate, material deviation from Tuxlink. Tuxlink uses a dedicated `feat/v0.0.1` integration branch off `main`, landing per-task PRs there and ff-merging to `main` only at release. Sonde's CI builds and tests on push to `main`; a separate integration branch would mean either duplicating that CI onto the integration branch or losing per-PR signal. So Sonde uses **`main` itself as the integration branch**. That choice changes which branch is protected and where the commit-discipline carve-out applies, but it does not change the per-task / no-squash / worktree / destructive-ban substance — those apply to `main` exactly as they applied to Tuxlink's `feat/v0.0.1`.

## Decision

### 1. `main` is the integration branch (Sonde's deviation from Tuxlink)

Sonde uses `main` as its integration branch, to match the existing CI that builds and tests on push to `main`. Per-task PRs merge into `main` directly; there is no separate `feat/vX` integration branch.

**Direct commits to `main` are blocked at the harness level** by the commit-discipline hook on `git commit`. The only carve-out is the rare *local merge-commit* on `main` (the integration merge that lands a reviewed branch when not done via the forge), which the operator authorizes per-invocation by prefixing the command with the one-shot environment variable `ALLOW_INTEGRATION_COMMIT=1`. The carve-out is per-shell-invocation, not persistent; it is a deliberate foot-gun guard, not a routine path.

### 2. Per-task branch model

One branch per unit of work, branched off `main`:

- **Naming:** `<bd-id>/<slug>` is preferred when a bd issue exists (e.g., `sonde-ge9/adopt-governance`). Otherwise `agent-<moniker>/<slug>` or `task-NN-<slug>`.
- The agent (or human) branches from `main`, implements the task on that branch, runs the quality gates, and opens a PR against `main`.
- After review, the branch merges into `main` (per §3) and is deleted.

This gives isolation (a botched task is a discardable branch, not a contaminated `main`), parallelism (Agent A on one branch and Agent B on another cannot collide), and review granularity (one task's diff per PR).

### 3. No-squash merge

PRs land on `main` as **merge commits with no fast-forward**, never squashed and never rebase-merged:

- **`gh` CLI:** `gh pr merge <#> --merge --delete-branch` (NOT `--squash`, NOT `--rebase`).
- **GitHub UI:** "Create a merge commit."
- **Local merge** (the rare `ALLOW_INTEGRATION_COMMIT=1` path): `git merge --no-ff <branch>`.

The merge commit's second-parent linkage preserves every per-task-branch commit on `main`, so `git bisect` and `git blame` work at any granularity and anyone cloning the repo sees the full per-step history — no dependency on the forge's PR-view UI for archaeology. Squash-merge is banned because it collapses task-internal commits into one opaque diff and, once the source branch is deleted, destroys the per-step history irrecoverably (a data-loss class, same family as the destructive-git ban).

**Because nothing is squashed, polish WIP commits locally before pushing.** Collapse `wip:` / `fixup!` / "oops" commits via a non-interactive `git rebase <base>` (or `git reset --soft <good-sha>` + a fresh commit) on **local, un-pushed** commits. Once pushed, commits are immutable — the destructive-git ban on `--amend` of pushed commits and on `git rebase -i` enforces this. The push gates the polish.

### 4. Worktrees mandatory under bd-issue ownership

When the main-checkout-race hook (`.claude/hooks/block-main-checkout-race.sh`) reports that another live session is active, any session not holding the main-checkout lease MUST move its write work to a **worktree**; the main checkout is reserved for the lease-holder. The hook's determination is authoritative — agents do not second-guess it via the sessions script. Read-only ops and `bd` commands stay free regardless. For solo-session work (no other live session), main-checkout writes are fine and worktrees are optional.

Every worktree binds to a bd issue:

1. A **bd issue** in `in_progress` claims the worktree, with its absolute path recorded in the issue body or via `bd remember`. `bd show <id>` is the canonical answer to "what is `worktrees/X` for?"
2. The branch follows the §2 naming convention.
3. The worktree path is `worktrees/<bd-id-or-slug>/` at the repo root; `worktrees/` is `.gitignore`d.
4. The session honours every other rule here (commit discipline, destructive-git ban, session-end handoff).

A worktree without a bd-issue claim is an anti-pattern: either retroactively claim it with a bd issue, or dispose of it per the disposal ritual. The harness-spawned ephemeral worktree (the `Agent` tool's `isolation: "worktree"`) is uncontroversially permitted and needs no per-worktree bd issue — the harness manages its lifecycle.

The one-line creation helper is `python3 .claude/scripts/new_sonde_worktree.py --slug <slug> --issue <sonde-bd-id>`; see [`docs/git-strategy.md`](../git-strategy.md) for the lease mechanism and session-inspection details.

### 5. Destructive-git operations are BANNED

The [`.claude/hooks/block-destructive-git.sh`](../../.claude/hooks/block-destructive-git.sh) hook is the **canonical enforcement** of the destructive-git ban — the hook source is the authoritative banned-list, not this prose. It denies, among others: `git reset --hard`, `git push --force` / `-f` / `--force-with-lease`, `git commit --amend`, `git rebase -i` / `--interactive`, `git branch -D`, `git checkout -- .` / `git restore .` / `git clean -f`, `git worktree remove`, history-rewrite tooling (`filter-branch` / `filter-repo`), recovery-net strippers (`reflog expire --expire=now`, `gc --prune=now`), and gate-bypass flags (`--no-verify`, `--no-gpg-sign`). If a hook denial surprises you, find a non-destructive alternative — never an end-run. If you believe you need a banned command, stop and surface the situation with a proposed safe alternative.

### 6. Worktree disposal ritual

Because `git worktree remove` is hook-banned (it silently destroys gitignored-but-stateful and untracked content — the `musing-bhabha` failure class), worktree disposal uses a four-step ritual: **inventory → propagate/archive → `rm -rf` → `git worktree prune`**. The full ritual, including why the `cd`-back-to-main step before archiving is load-bearing, lives in [`docs/git-strategy.md`](../git-strategy.md); it is not restated here.

## Consequences

**Positive:**

- Sonde reaches Tuxlink-parity governance in one onboarding-friendly record. The same safety substrate (per-task isolation, lease-coordinated worktrees, destructive-op refusal, history preservation) protects Sonde's repository.
- `main`-as-integration means the existing CI signal applies directly to every landed PR with no integration-branch duplication.
- No-squash + the destructive ban together make data loss structurally hard: history is preserved on merge, and the operations that could rewrite or destroy it are hook-denied.
- Per-task branches + lease-coordinated worktrees make concurrent-agent work safe by default, unblocking parallel implementation of Sonde's roadmap.

**Negative:**

- More PRs to review than a single-branch model, and a non-linear `main` graph (`git log --oneline --first-parent` collapses it to merge boundaries). Mitigated by small per-PR diffs and review subagents.
- The `ALLOW_INTEGRATION_COMMIT=1` carve-out on `main` is a foot-gun if used carelessly; it is per-invocation by design.
- Worktree creation is multi-step (claim/record the bd issue, then add the worktree); the `new_sonde_worktree.py` helper collapses this.
- Polish-before-push is a discipline the author must actually perform; WIP noise pushed to a branch cannot be retroactively cleaned without a banned operation.
- **Sonde deliberately does NOT, in this pass, adopt Tuxlink's `.githooks` branch-lifecycle state machine (Tuxlink's ADR 0017)** — the `active → pr-open → merged-dead` model that denies commits/pushes to a branch whose PR has merged. That is a possible future ADR for Sonde; for now the per-task + no-squash + destructive-ban + worktree-ownership stack is the adopted scope, and orphan-post-merge branches are managed by discipline rather than by a lifecycle hook.

## Alternatives considered

- **Keep Tuxlink's dedicated `feat/vX` integration branch verbatim.** Rejected for Sonde: Sonde's CI is wired to `main`, so an integration branch would either duplicate CI or lose per-PR signal. `main`-as-integration is the smaller deviation that preserves the existing pipeline. The per-task / no-squash / worktree / destructive substance is unchanged — only the protected branch's identity differs.
- **Spread the governance across four ADRs mirroring Tuxlink's 0004/0008/0009/0010.** Rejected. Sonde is younger and smaller; a single consolidated governance record is easier to onboard against and to keep internally consistent. The Tuxlink ADRs remain the upstream reference for full rationale and watched failure modes.
- **Squash-merge into `main`** (one commit per PR). Rejected — collapses per-step history and, with post-merge branch deletion, destroys it irrecoverably. This is the exact reversal Tuxlink's ADR 0010 made; Sonde adopts the conclusion directly rather than repeating the mistake first.
- **Rebase-merge.** Rejected — rewrites commit SHAs (breaking external references) and drops the informational merge commit that marks "this PR landed here." Tuxlink's experience favored merge-commit no-ff specifically.
- **Allow `git worktree remove` for "clean" worktrees.** Rejected — git's own clean-check does not surface gitignored-but-stateful content (the bd local-state class), which is exactly where the `musing-bhabha` data loss occurred. The ritual is the only sanctioned disposal path.
- **Worktrees mandatory even for solo-session work.** Rejected — setup overhead with no benefit when only one session touches the repo; the mandate applies only under detected concurrency, matching Tuxlink's source-of-truth posture (the hook is the authority).
- **Adopt the branch-lifecycle state machine now.** Deferred, not rejected — it is a coherent next step but out of scope for this governance-adoption pass; a future ADR can ratify it for Sonde.
- **Document the rules in prose only (no hooks).** Rejected — the Geographica `git reset --hard` incident happened while the rule was *correctly documented*. Prose did not prevent it; the hook layer does. Sonde adopts the hooks as the canonical enforcement.
