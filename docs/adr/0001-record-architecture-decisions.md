# 1. Record architecture decisions

Date: 2026-06-14
Status: Accepted
Deciders: cameronzucker, sonde-ge9

## Context

Sonde is the modem workspace extracted from Tuxlink and is being brought up to its sibling's engineering discipline. A predictable failure mode in projects of this kind — especially AI-orchestrated ones — is the loss of architectural intent: decisions made in a chat session, a plan document, or a commit message body get diluted as the codebase grows, until "why is it this way" becomes archaeology.

Tuxlink surfaced this concretely and remediated it with an [Architecture Decision Record](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) log (its `docs/adr/`). The sister Geographica project demonstrated the failure mode that motivated it: substantive choices were captured only in chat history, commit prose, and pitfalls docs, and had to be reconstructed months later — error-prone and lossy. Sonde inherits both the lesson and the remedy.

The standard remediation, used by Kubernetes, CockroachDB, Tauri, and many other professional OSS projects, is the ADR: a short, dated Markdown file capturing one decision, its context, and its consequences, kept in version control alongside the code. As Sonde grows its own architecture (the PHY runtime, FEC wiring, the channel-sim gate) and adopts Tuxlink's governance, it needs the same structured record.

## Decision

Sonde adopts the [Nygard ADR format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions) for recording architectural decisions, stored in `docs/adr/` and indexed in `docs/adr/README.md`.

ADRs are written:

- **At decision time** — when a choice between viable alternatives is being made, not retroactively after the code lands.
- **By the agent or contributor making the decision** — moniker / handle is recorded in the `Deciders` line.
- **As part of the same PR** that enacts the decision in code (or in the governing docs), so reviewers can verify the ADR matches what was built.

ADRs use the format documented in `docs/adr/README.md`: Title, Date, Status, Deciders, Context, Decision, Consequences, Alternatives considered.

## Consequences

**Positive:**

- Future contributors (human or AI) can answer "why is it this way" in seconds without reconstructing chat history.
- Reversing a decision is auditable — the original ADR stays, a superseding ADR explains the change. The decision log preserves history the same way the no-squash-merge rule ([ADR 0002](0002-git-workflow-and-governance.md)) preserves commit history.
- The ADR record itself is a portfolio artifact demonstrating engineering discipline, consistent with Sonde's sibling Tuxlink.

**Negative:**

- ~10–30 minutes of overhead per significant decision to write the ADR. Acceptable; the cost of NOT writing one is reconstruction work later.
- ADR drift is possible (the code diverges from the ADR over time) but mitigated by the rule that ADR changes go through PRs alongside code changes, and by the propagation contract that keeps `docs/adr/` the canonical source and operational docs mere pointers.

## Alternatives considered

- **Decisions captured only in commit messages.** Rejected — commit prose is unstructured, hard to discover, and lost in long histories. Geographica demonstrated this failure mode.
- **Decisions captured only in the plans under `docs/superpowers/plans/`.** Rejected as the sole record — plans capture the forward-looking implementation strategy for one body of work; they are not the durable, indexed record of *why* an architecture was chosen, and they are not superseded-in-place when a decision reverses. ADR 0003 demonstrates the right relationship: the ADR summarizes and points at the plan.
- **Decisions captured in wiki / Notion / external doc.** Rejected — splits the source of truth from the code, requires separate access, and tends to go stale when the maintainer rotates.
- **MADR (Markdown Architectural Decision Records) format.** A more structured alternative. Defer; Nygard format is sufficient for a single-maintainer project, matches Tuxlink (easing cross-project knowledge transfer), and migrating to MADR later is a mechanical exercise.
- **No ADRs (rely on README + pitfalls docs).** Rejected — pitfalls docs capture rules-of-thumb, README captures the user-facing surface, neither captures the *reasoning* behind architectural choices.
