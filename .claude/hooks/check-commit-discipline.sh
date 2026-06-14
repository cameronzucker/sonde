#!/bin/bash
# check-commit-discipline.sh — PreToolUse Bash hook
#
# Enforces commit discipline (CLAUDE.md §"Commit and release discipline",
# ADR 0002):
#   1. No <SESSION-MONIKER> placeholder leaks (catches unsubstituted plan templates).
#   2. Every commit has an Agent: <moniker> trailer (CLAUDE.md §Agent identity).
#   3. No direct commits to main unless ALLOW_INTEGRATION_COMMIT=1 prefix is present.
#
# Sonde uses main as its integration branch (ADR 0002: "main-as-integration").
# Ordinary task work happens on per-task branches (sonde-<bd-id>/<slug>) and
# lands on main via `gh pr merge --merge --delete-branch` (server-side, which
# bypasses this hook). The ALLOW_INTEGRATION_COMMIT=1 carve-out exists only for
# the rare legitimate LOCAL merge-commit (no-ff); squash-merge is banned.
#
# Input:  JSON on stdin with .tool_input.command
# Output: JSON deny on stdout if a check fails; nothing if clean.
# Exit:   0 always.
#
# Ported from tuxlink/.claude/hooks/check-commit-discipline.sh (adapted:
# Sonde blocks `main` directly since main is the integration branch).

set -u

# Resolve repo root from this script's filesystem location.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"

input=$(cat)
cmd=$(printf '%s' "$input" | jq -r '.tool_input.command // ""')

# Only act on `git commit` invocations. `git commit --amend` is blocked by
# block-destructive-git.sh; we don't double-up here.
if ! printf '%s' "$cmd" | grep -qE '\bgit[[:space:]]+commit\b'; then
    exit 0
fi

deny() {
    local reason="$1"
    jq -n --arg reason "$reason" '{
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": $reason
        }
    }'
    exit 0
}

# Check 1: <SESSION-MONIKER> placeholder must be substituted before commit
if printf '%s' "$cmd" | grep -q '<SESSION-MONIKER>'; then
    deny "Commit message still contains the literal '<SESSION-MONIKER>' placeholder from the plan template. Substitute with your actual session moniker (e.g., 'Agent: opossum-magnolia-taiga') in the heredoc before committing."
fi

# Check 2: require Agent: <moniker> trailer
if ! printf '%s' "$cmd" | grep -qE 'Agent:[[:space:]]+[a-z0-9_-]+'; then
    deny "Commit message lacks the required 'Agent: <moniker>' trailer per CLAUDE.md. Add 'Agent: <your-session-moniker>' on its own line above 'Co-Authored-By:'. Generate a moniker with 'python3 .claude/scripts/get_agent_moniker.py'."
fi

# Check 3: branch protection — main is blocked unless ALLOW_INTEGRATION_COMMIT=1
#
# Resolve the branch from the commit's WORKING DIRECTORY (the payload .cwd,
# constrained under $REPO), NOT from $REPO. The main checkout sits on `main`
# (ADR 0002 main-as-integration), so resolving from $REPO falsely classified
# EVERY worktree commit as a `main` commit and denied it. Mirrors the
# resolution block-main-checkout-race.sh already uses (its lines ~62-74).
payload_cwd=$(printf '%s' "$input" | jq -r '.cwd // ""')
cwd_for_git="$REPO"
if [[ -n "$payload_cwd" && -d "$payload_cwd" ]]; then
    resolved_cwd=$(cd "$payload_cwd" 2>/dev/null && pwd)
    if [[ "$resolved_cwd" == "$REPO" || "$resolved_cwd" == "$REPO"/* ]]; then
        cwd_for_git="$resolved_cwd"
    fi
fi
branch=$(git -C "$cwd_for_git" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")

case "$branch" in
    main)
        # Allow only if the command has ALLOW_INTEGRATION_COMMIT=1 set as an
        # env-var prefix. Match at command start OR after a shell separator.
        if ! printf '%s' "$cmd" | grep -qE '(^|[[:space:]&;|])[[:space:]]*ALLOW_INTEGRATION_COMMIT=1[[:space:]]+git'; then
            deny "Direct commits to 'main' are blocked. Sonde uses main as its integration branch (ADR 0002): ordinary work happens on a per-task branch and lands via 'gh pr merge --merge --delete-branch'. Branch off first: 'git checkout -b sonde-<bd-id>/<slug>'. For the rare legitimate LOCAL merge-commit (no-ff), prefix the command with 'ALLOW_INTEGRATION_COMMIT=1 git commit ...'."
        fi
        ;;
esac

exit 0
