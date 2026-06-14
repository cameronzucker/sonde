#!/bin/bash
# check-commit-discipline.sh — PreToolUse Bash hook
#
# Enforces commit discipline (CLAUDE.md §"Commit and release discipline",
# ADR 0002):
#   1. No <SESSION-MONIKER> placeholder leaks (catches unsubstituted templates).
#   2. Every commit has an Agent: <moniker> trailer (CLAUDE.md §Agent identity).
#   3. No direct commits to main unless ALLOW_INTEGRATION_COMMIT=1 prefix.
#
# Sonde uses main as its integration branch (ADR 0002: "main-as-integration").
# Ordinary task work happens on per-task branches (sonde-<bd-id>/<slug>) in
# worktrees and lands on main via `gh pr merge --merge --delete-branch`
# (server-side, which bypasses this hook). The ALLOW_INTEGRATION_COMMIT=1
# carve-out exists only for the rare legitimate LOCAL no-ff merge-commit.
#
# Parsing model (sonde-ge9.1, Codex-reviewed). Robust shell parsing in a hook
# is infeasible, so the checks are split by severity and failure mode:
#
#  * SECURITY — the main-branch block — fails CLOSED. A commit is detected
#    BROADLY (anchored `git [globals] commit` at any command position across the
#    whole command, so no decoy token hides the real commit). The branch is
#    resolved from the directory the commit TARGETS (literal `git -C <path>`,
#    else a literal leading `cd <path>`, else the Bash `.cwd`, else $REPO). The
#    ALLOW_INTEGRATION_COMMIT carve-out must be AFFIRMATIVELY present in the
#    stripped command prefix — stripping can only remove tokens, so a commit
#    message body cannot inject the carve-out (no false-allow).
#
#  * DISCIPLINE — placeholder + Agent-trailer — may fail OPEN. They run only
#    when a commit is detected NARROWLY (on the stripped prefix). If a decoy
#    defeats narrow detection, the worst case is an un-enforced trailer (a
#    discipline lapse), never a main commit.
#
# Known fail-SAFE residual: a non-commit command whose heredoc/body merely
# MENTIONS `git commit`, run on the MAIN checkout, is denied by the security
# block. Worktree work is unaffected. Better a rare false-deny on main than a
# missed main commit.
#
# Input:  JSON on stdin with .tool_input.command and .cwd
# Output: JSON deny on stdout if a check fails; nothing if clean.
# Exit:   0 always.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"

input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // ""')

deny() {
    jq -n --arg reason "$1" '{
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": $reason
        }
    }'
    exit 0
}

# Anchored commit matcher: `git [-C/-c <arg> …] commit` where `git` sits at a
# command position — line start (^ is per-line in grep), or after a shell
# separator, optionally preceded by VAR=val env assignments and a `cd … &&`.
# Does not match `git log`, nor `git` inside a quoted string.
COMMIT_RE='(^|&&|\|\||[;|(])[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]+[[:space:]]+)*(cd[[:space:]]+[^[:space:]]+[[:space:]]+&&[[:space:]]+)?git[[:space:]]+((-C|-c)[[:space:]]+[^[:space:]]+[[:space:]]+|--git-dir([[:space:]]+|=)[^[:space:]]+[[:space:]]+|--work-tree([[:space:]]+|=)[^[:space:]]+[[:space:]]+)*commit([[:space:]]|$)'

# BROAD detection (whole command, all lines) — fail-closed for the main block.
broad_commit=false
if printf '%s' "$cmd" | grep -qE "$COMMIT_RE"; then
    broad_commit=true
fi

# Stripped command prefix: drop the heredoc body (`%%` spans newlines) and an
# inline -m/-F message argument. Used for NARROW detection, target-dir, and the
# ALLOW_INTEGRATION_COMMIT carve-out — all "must be affirmatively present"
# checks where stripping is safe (it can only remove, never inject).
prefix="${cmd%%<<*}"
prefix=$(printf '%s' "$prefix" | sed -E 's/[[:space:]](-m|--message|-F|--file)([[:space:]]|=).*$//')

# NARROW detection (stripped prefix) — gates the fail-open discipline checks.
narrow_commit=false
if printf '%s' "$prefix" | grep -qE "$COMMIT_RE"; then
    narrow_commit=true
fi

# Nothing commit-like anywhere → not our concern.
if [ "$broad_commit" != "true" ] && [ "$narrow_commit" != "true" ]; then
    exit 0
fi

# --- DISCIPLINE checks (fail-open: only for a narrowly-detected real commit). ---
if [ "$narrow_commit" = "true" ]; then
    if printf '%s' "$cmd" | grep -q '<SESSION-MONIKER>'; then
        deny "Commit message still contains the literal '<SESSION-MONIKER>' placeholder from the plan template. Substitute your actual session moniker (e.g., 'Agent: opossum-magnolia-taiga') before committing."
    fi
    if ! printf '%s' "$cmd" | grep -qE 'Agent:[[:space:]]+[a-z0-9_-]+'; then
        deny "Commit message lacks the required 'Agent: <moniker>' trailer per CLAUDE.md. Add 'Agent: <your-session-moniker>' on its own line above 'Co-Authored-By:'. Generate a moniker with 'python3 .claude/scripts/get_agent_moniker.py'."
    fi
fi

# --- SECURITY check: main-branch block (fail-closed on broad detection). ---
if [ "$broad_commit" != "true" ]; then
    exit 0
fi

# Git directory / work-tree overrides redirect the commit target in ways this
# hook does not parse (GIT_DIR=, GIT_WORK_TREE=, --git-dir, --work-tree). From a
# worktree cwd these could silently retarget `main`. Scanned over the WHOLE
# command (not just the prefix), so a decoy heredoc placed before the real
# commit cannot hide the override. Deny-as-ambiguous (fail-closed).
if printf '%s' "$cmd" | grep -qE '(^|[[:space:]])(GIT_DIR=|GIT_WORK_TREE=|--git-dir([[:space:]]|=)|--work-tree([[:space:]]|=))'; then
    deny "Commit-discipline hook cannot safely resolve the target branch: the command sets a git directory / work-tree override (GIT_DIR / GIT_WORK_TREE / --git-dir / --work-tree). Re-run with the Bash working directory set to the target worktree and without these overrides."
fi

# Resolve the commit-target directory from the stripped prefix only.
target=""
ambiguous=""
gitc=$(printf '%s' "$prefix" | grep -oE '[[:space:]]-C[[:space:]]+[^[:space:]]+' | head -1 | sed -E 's/^[[:space:]]-C[[:space:]]+//')
if [ -n "$gitc" ]; then
    case "$gitc" in
        *'$'* | '"'* | "'"* | *'`'*) ambiguous="git -C $gitc" ;;
        *) target="$gitc" ;;
    esac
fi
if [ -z "$target" ] && [ -z "$ambiguous" ]; then
    cdp=$(printf '%s' "$prefix" | grep -oE '(^|&&|[;|])[[:space:]]*cd[[:space:]]+[^[:space:]]+' | head -1 | sed -E 's/.*cd[[:space:]]+//')
    if [ -n "$cdp" ]; then
        case "$cdp" in
            *'$'* | '"'* | "'"* | *'`'*) ambiguous="cd $cdp" ;;
            *) target="$cdp" ;;
        esac
    fi
fi
if [ -n "$ambiguous" ]; then
    deny "Commit-discipline hook cannot safely resolve the target branch: the command uses a variable/quoted path ('$ambiguous'). Re-run the commit with the Bash tool's working directory set to the worktree (the hook reads the branch from cwd), or use a literal path in 'git -C <path> commit ...'."
fi
if [ -z "$target" ]; then
    payload_cwd=$(printf '%s' "$input" | jq -r '.cwd // ""')
    if [ -n "$payload_cwd" ]; then
        target="$payload_cwd"
    else
        target="$REPO"
    fi
fi

branch=$(cd "$target" 2>/dev/null && git rev-parse --abbrev-ref HEAD 2>/dev/null) || branch=""

case "$branch" in
    main)
        # ALLOW_INTEGRATION_COMMIT must be affirmatively present in the prefix.
        if ! printf '%s' "$prefix" | grep -qE '(^|&&|[;|])[[:space:]]*ALLOW_INTEGRATION_COMMIT=1[[:space:]]+([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]+[[:space:]]+)*git'; then
            deny "Direct commits to 'main' are blocked. Sonde uses main as its integration branch (ADR 0002): ordinary work happens on a per-task branch and lands via 'gh pr merge --merge --delete-branch'. Branch off first: 'git checkout -b sonde-<bd-id>/<slug>'. For the rare legitimate LOCAL merge-commit (no-ff), prefix the command with 'ALLOW_INTEGRATION_COMMIT=1 git commit ...'."
        fi
        ;;
esac

exit 0
