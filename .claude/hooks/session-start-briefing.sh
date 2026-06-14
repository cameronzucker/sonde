#!/bin/bash
# session-start-briefing.sh — SessionStart hook
#
# Emits a Sonde session briefing into the model's context when a fresh
# Claude Code session starts. Includes branch state, working-tree status,
# the most recent handoff filename, and the 5 most recent commits.
#
# Input:  JSON on stdin (unused)
# Output: JSON with hookSpecificOutput.additionalContext for context injection.
# Exit:   0 always (failure to gather any field is non-fatal).
#
# Ported from tuxlink/.claude/hooks/session-start-briefing.sh (adapted:
# Sonde uses main as the integration branch).

set -u

# Resolve repo root from this script's filesystem location.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO" 2>/dev/null || { echo '{}'; exit 0; }

branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "(unknown)")
ahead_behind=$(git for-each-ref --format='%(upstream:track)' "refs/heads/$branch" 2>/dev/null)
status_count=$(git status --short 2>/dev/null | wc -l | tr -d ' ')

last_handoff_line=""
if [[ -d dev/handoffs ]]; then
    last_handoff_file=$(find dev/handoffs -maxdepth 1 -name '20*-*.md' -type f -print 2>/dev/null \
        | sort -r | head -1)
    if [[ -n "$last_handoff_file" ]]; then
        last_handoff_line="$(basename "$last_handoff_file")"
    fi
fi

recent_commits=$(git log --oneline -5 2>/dev/null)

# Branch-protection reminder when on the integration branch.
branch_warning=""
case "$branch" in
    main)
        branch_warning=$'\n\n⚠️  You are on \x60main\x60 (Sonde\x27s integration branch). Direct commits are blocked by the commit-discipline hook. Branch off with \x60git checkout -b sonde-<bd-id>/<slug>\x60 before any work; land via \x60gh pr merge --merge --delete-branch\x60.'
        ;;
esac

briefing=$(cat <<EOF
## Sonde session briefing

- **Branch:** \`${branch}\`${ahead_behind:+ ${ahead_behind}}
- **Working tree:** ${status_count} uncommitted file(s)
- **Most recent handoff:** ${last_handoff_line:-none}

### Recent commits
\`\`\`
${recent_commits}
\`\`\`${branch_warning}

### Reminders
- Pick a session moniker via \`python3 .claude/scripts/get_agent_moniker.py\` (3-word hyphenated form, auto-pre-flighted against git history) and state it in your first message.
- Per-task branches: \`sonde-<bd-id>/<slug>\` (or \`agent-<moniker>/<slug>\` / \`task-NN-<slug>\`), off \`main\`.
- Commit-discipline hooks will reject: missing \`Agent:\` trailer, unsubstituted \`<SESSION-MONIKER>\` placeholder, direct commits to \`main\`.
- Worktrees are mandatory for write work when another live session is active (ADR 0002 / docs/git-strategy.md). Create one with \`python3 .claude/scripts/new_sonde_worktree.py --slug <slug> --issue <sonde-bd-id>\`.
EOF
)

jq -n --arg ctx "$briefing" '{
    "hookSpecificOutput": {
        "hookEventName": "SessionStart",
        "additionalContext": $ctx
    }
}'
