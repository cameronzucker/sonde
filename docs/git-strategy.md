# Sonde git strategy

The operational git playbook for Sonde. This is the **how**; the **why** — the
decisions, alternatives, and watched failure modes — lives in
[ADR 0002](adr/0002-git-workflow-and-governance.md). Where this doc and the ADR
appear to disagree, the ADR is the source of truth and this doc gets a corrective
commit. Sonde derives this playbook from its sibling
Tuxlink (private sibling repo)'s `CLAUDE.md` git sections,
adapted for Sonde's `main`-as-integration model.

## The model in one breath

- **`main` is the integration branch.** Sonde's CI builds and tests on push to
  `main`, so `main` plays the role Tuxlink gives its `feat/vX` branch. There is
  no separate integration branch.
- **One branch per unit of work**, named `<bd-id>/<slug>` (preferred when a
  bd issue exists), or `agent-<moniker>/<slug>`, or `task-NN-<slug>`. Branch off
  `main`, open a PR against `main`.
- **PRs merge as merge-commits, no fast-forward, never squashed.** Polish WIP
  commits locally before pushing.
- **Direct commits to `main` are hook-blocked.** The only carve-out is the rare
  local integration merge-commit (see below).

## Per-task branch + PR lifecycle

```bash
# 1. Branch off main
git switch main
git switch -c <bd-id>/<slug>

# 2. ... implement, commit in coherent steps ...

# 3. Polish local, un-pushed WIP commits BEFORE pushing (no-squash means every
#    commit lands on main; clean them up while they are still local).
git rebase main                 # non-interactive linear replay; -i is BANNED
#    or, to collapse N noisy commits into one:
git reset --soft <good-sha>     # --soft only; --hard is BANNED
git commit -m "<clean message>"

# 4. Push and open the PR against main
git push -u origin <bd-id>/<slug>
gh pr create --base main

# 5. After review, land it as a merge commit (NOT --squash, NOT --rebase) and
#    delete the branch
gh pr merge <#> --merge --delete-branch
```

Once a commit is pushed it is immutable — the destructive-git ban on `--amend`
of pushed commits and on `git rebase -i` guarantees it. The push gates the
polish.

### The integration-merge carve-out

Direct commits to `main` are denied by the commit-discipline hook. The single
sanctioned exception is the rare **local integration merge-commit** — when a
reviewed branch is merged into `main` locally rather than through the forge. The
operator authorizes it per-invocation:

```bash
ALLOW_INTEGRATION_COMMIT=1 git merge --no-ff <branch>
```

The env var is one-shot (per-shell-invocation), never exported persistently. It
is a deliberate foot-gun guard, not a routine path — prefer
`gh pr merge --merge` for ordinary landings.

## Worktrees and the main-checkout lease

Sonde coordinates concurrent sessions with a **main-checkout lease**. The
`block-main-checkout-race.sh` hook writes per-session lease files under
`<git-common-dir>/session-leases/`, heartbeat-refreshed on each Bash tool call
with a **30-minute TTL**. When another live session exists, main-checkout *write*
operations are denied unless this session owns `main-checkout.json`. Read-only
ops and `bd` commands are always free.

**The hook's determination is authoritative.** When it denies a write op citing
another live session, the correct response is to move your write work to a
worktree and re-run there — not to second-guess the hook via the sessions script.

Inspect live sessions with:

```bash
python3 .claude/scripts/get_sonde_sessions.py
```

For solo-session work (no other live session), main-checkout writes are fine and
worktrees are optional.

### Creating a worktree

Every worktree binds to a bd issue (`bd show <id>` answers "what is this worktree
for?"), uses the per-task branch naming, and lives at
`worktrees/<bd-id-or-slug>/` (gitignored). The one-liner sets all of this up:

```bash
python3 .claude/scripts/new_sonde_worktree.py --slug <slug> --issue <sonde-bd-id>
```

A worktree without a bd-issue claim is an anti-pattern: either retroactively
claim it with a bd issue, or dispose of it via the ritual below. The
harness-spawned ephemeral worktree (the `Agent` tool's `isolation: "worktree"`)
is exempt — the harness manages its lifecycle.

## Destructive git is banned

The [`.claude/hooks/block-destructive-git.sh`](../.claude/hooks/block-destructive-git.sh)
hook is the **canonical enforcement** — its source is the authoritative
banned-list, not this prose. It denies (non-exhaustive quick reference):

| Banned | Use instead |
|---|---|
| `git reset --hard <ref>` | `git revert <commit>`, or restore named files |
| `git push --force` / `-f` / `--force-with-lease` | open a new PR, or ask |
| `git commit --amend` (pushed commits) | create a new commit |
| `git rebase -i` / `--interactive` | non-interactive `git rebase <base>` |
| `git branch -D` | `git branch -d` (refuses unmerged) |
| `git checkout -- .` / `git restore .` / `git clean -f` | name files explicitly |
| `git worktree remove` | the disposal ritual (below) |
| `git filter-branch` / `git filter-repo` | — (mass history rewrite) |
| `git reflog expire --expire=now` / `git gc --prune=now` | — (strips recovery net) |
| `--no-verify` / `--no-gpg-sign` | — (bypasses the gates) |

If a hook denial surprises you, find a non-destructive alternative — never an
end-run. If you believe you need a banned command, stop and surface the situation
with a proposed safe alternative. (The rule's grounding: the 2026-04-20
Geographica `git reset --hard` wiped seven commits while the rule was *correctly
documented in prose* — only the hook layer actually prevents it.)

## Worktree disposal ritual

`git worktree remove` is hook-banned because its own "is it clean?" check does
**not** surface gitignored-but-stateful content (e.g. a local bd database) or
untracked working-tree files — the LFST `musing-bhabha` incident permanently lost
untracked content this way. Disposal uses a four-step ritual instead. There is no
shortcut.

### Step 1 — Inventory (from inside the worktree being disposed)

```bash
git status --short                                  # tracked dirty
git ls-files --others --exclude-standard            # untracked
git ls-files --others --ignored --exclude-standard  # gitignored on disk (the dangerous class)
git stash list                                      # worktree-scoped stashes
```

These four cover the four at-risk categories: dirty-tracked, untracked,
gitignored-stateful, stashed. If any return non-empty, do Step 2 before Step 3.
If all are empty and `bd show <worktree-issue-id>` confirms the work is closed,
go straight to Step 3.

### Step 2 — Propagate or archive

Anything Step 1 surfaced that is not safe to lose gets either **propagated**
(commit + push to a topic branch) or **archived locally**. Critically, `cd` back
to the main repo *before* archiving:

```bash
cd <main-repo-path>          # CRITICAL: leave the worktree before archiving
tar czf .claude/worktree-archives/<worktree-name>-$(date -u +%Y%m%dT%H%M%SZ).tar.gz <full-worktree-path>
```

**Why the `cd` is load-bearing:** if you write the archive while still `cd`'d
*inside* the doomed worktree, the relative `.claude/worktree-archives/...` path
resolves to `<worktree>/.claude/worktree-archives/...` — and Step 3's
`rm -rf <worktree>` then deletes the archive along with the worktree, defeating
the entire safety step. `cd` back to the main repo first (or use an absolute path
for the archive destination). `.claude/worktree-archives/` is gitignored;
archives are per-machine safety nets, not project artifacts.

### Step 3 — Physical remove

```bash
rm -rf <worktree-path>
```

`rm -rf` is not hook-gated (gating it would block too much legitimate work); the
discipline lives in the ritual — Steps 1 and 2 are mandatory before this.

### Step 4 — Prune git's registry

```bash
git worktree prune
```

Cleans git's internal worktree registry of entries whose working trees no longer
exist on disk. Skipping it leaves ghost rows in `git worktree list`. Always run
after Step 3.

## See also

- [ADR 0002 — Git workflow and governance](adr/0002-git-workflow-and-governance.md):
  the decisions, full rationale, alternatives considered, and the deliberate
  non-adoption (this pass) of Tuxlink's branch-lifecycle state machine.
- [ADR 0001 — Record architecture decisions](adr/0001-record-architecture-decisions.md):
  why Sonde keeps an ADR log at all.
