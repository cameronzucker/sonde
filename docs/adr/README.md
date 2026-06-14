# Architecture Decision Records

This directory holds **Architecture Decision Records (ADRs)** — short, dated documents capturing significant architectural decisions made on Sonde, why they were made, and what the consequences are.

ADRs are not a replacement for design documents, specs, or the plans under `docs/superpowers/plans/`. They are the **record** of what was decided, written when the decision is fresh, so future contributors (and future AI agents) can reconstruct the reasoning without spelunking through commit messages or chat logs.

Sonde is a sibling of [Tuxlink](https://github.com/cameronzucker/tuxlink) and adopts its engineering discipline; this ADR log mirrors Tuxlink's `docs/adr/` conventions. Where Sonde deliberately deviates from Tuxlink (notably the integration-branch choice — see [ADR 0002](0002-git-workflow-and-governance.md)), the deviation is recorded as an explicit decision rather than left as an undocumented difference.

## When to write an ADR

- A choice between two or more viable architectures or technologies.
- A constraint accepted that limits future options (e.g., "no async runtime in the PHY runtime; `std::thread` only").
- A workflow / process commitment that the project will be held to (e.g., "main is the integration branch; per-task branches; no squash-merge").
- A reversal of a prior decision — supersede the old ADR, write a new one explaining the change.

Routine implementation choices and minor refactors do NOT need ADRs. The bar is "would a contributor six months from now reasonably ask 'why is it this way' and benefit from a paragraph of context?"

## Format

Sonde uses the [Nygard ADR format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) — short, structured Markdown. Each ADR has these sections:

```markdown
# NNNN. Title (decision in present tense — "Adopt X" / "Use Y" / "Ban Z")

Date: YYYY-MM-DD
Status: Accepted | Superseded by NNNN | Deprecated
Deciders: <names or session monikers of people involved>

## Context

<The problem or situation that prompted the decision. ~3 paragraphs.>

## Decision

<What was decided, in present tense. Be concrete.>

## Consequences

<What follows — both the positive consequences (this is now possible) and the negative ones (we now have to live with this constraint). Include reversal cost if non-trivial.>

## Alternatives considered

<Brief list of options NOT chosen, and why. Don't bury this — it's the most useful section for future readers.>
```

## File naming

`NNNN-<short-slug>.md`, zero-padded to 4 digits. Numbers are assigned in chronological order; once an ADR has a number, it never changes.

## Lifecycle

- An ADR is `Accepted` when merged.
- If a later ADR overrides it, the original's status changes to `Superseded by NNNN` (and the superseding ADR's `Context` references the original). The original's content stays — it's the historical record.
- **An ADR is never deleted; superseded ADRs remain for the audit trail.** History is preserved, never rewritten — the same discipline the destructive-git ban applies to commits applies to the decision log.

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions | Accepted |
| [0002](0002-git-workflow-and-governance.md) | Git workflow and governance (main-as-integration, per-task branches, no-squash, worktrees, destructive-git ban) | Accepted |
| [0003](0003-sonde-phy-runtime-adapter.md) | SondePhy runtime adapter architecture | Accepted |

## References

- [ADR Tools (Nygard)](https://github.com/npryce/adr-tools) — `adr-tools` CLI; not used in Sonde, but the format inspires this directory's structure.
- [Cognitect blog post on ADRs](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).
- [MADR — Markdown Architectural Decision Records](https://adr.github.io/madr/) — a more structured alternative if Sonde outgrows Nygard format.
- [`docs/git-strategy.md`](../git-strategy.md) — the operational git playbook that ADR 0002 governs.
