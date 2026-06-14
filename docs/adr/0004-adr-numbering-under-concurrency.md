# 0004. Allocate ADR numbers by claim-next, renumber-on-collision

Date: 2026-06-14
Status: Accepted
Deciders: opossum-magnolia-taiga

## Context

Sonde uses sequential, zero-padded ADR numbers (`NNNN-slug.md`, [ADR 0001](0001-record-architecture-decisions.md) / [README](README.md)). That is fine when one author writes ADRs one at a time. Sonde is not that repo: **multiple Claude Code sessions work the codebase concurrently** — governance in one session, the modem subsystems in others — and any of them may author an ADR on its own per-task branch ([ADR 0002](0002-git-workflow-and-governance.md)).

With concurrent authoring, plain sequential numbering has a race. Two branches both read `main`, both see `0003` as the highest ADR, and both create `0004-…`. Nothing surfaces the conflict until the second PR merges — at which point `main` has two different ADRs claiming `0004`, or a merge conflict on the index. This is the **same class of hazard** as the main-checkout HEAD race that the `block-main-checkout-race.sh` lease guards against, and the moniker-collision pre-flight in `get_agent_moniker.py` — a shared, monotonic namespace allocated without coordination.

The fix should match the lightweight spirit of the Nygard format. Heavy allocation machinery (a number-reservation service, a lock) is not warranted for a decision log that grows a handful of entries.

## Decision

ADR numbers are allocated by **claim-next, renumber-on-collision**:

1. **Claim.** When authoring an ADR, take the next free number from the **`docs/adr/README.md` index on the latest `origin/main`** (not your local branch point). Add your index row immediately, with status `Proposed`, as the first commit on your branch — so the claim is visible in your PR.
2. **Detect.** A number is "taken" if it appears in the index on `main` **or** in any open PR's diff. Before opening your PR, refresh `origin/main` and skim open PRs (`gh pr list`); if your number was taken by something that merged or is ahead of you, you have a collision.
3. **Renumber.** The **later-to-merge** ADR renumbers — rename the file, fix the `# NNNN.` heading and the index row, and update any inbound links. Because an unmerged ADR is still `Proposed` and lives only on its branch, renumbering it costs nothing but a rename.
4. **Immutability after merge.** Once an ADR is **merged to `main`** (status `Accepted`), its number is permanent and never reused — superseding ADRs get new numbers ([README](README.md) lifecycle). The renumber step only ever touches a `Proposed` ADR on a branch, never an `Accepted` one on `main`.

The reviewer/merger of a PR adding an ADR confirms the number is still free on `main` at merge time; if not, the PR renumbers before landing.

## Consequences

- The collision window shrinks to "two PRs in flight that the merger must eyeball," and the resolution (rename a `Proposed` file) is cheap and local — no `Accepted` ADR is ever renumbered, so inbound links to merged ADRs stay stable.
- Adds a small obligation: ADR authors must read `origin/main`'s index (not their stale branch point) and add the `Proposed` index row early. This is documented in [README §"Allocating a number"](README.md).
- It is advisory discipline, not a hook — there is no automated enforcement, by design (the cadence does not justify tooling). If ADR volume ever rises enough that manual coordination fails, this ADR should be superseded by a tooling-backed scheme.

## Alternatives considered

- **Range partition per workstream** (governance `0001–0019`, modem-PHY `0020–0049`, …). Collision-free by construction, but rigid: it requires enumerating workstreams up front, wastes numbers, and forces a re-carve when a new workstream appears. Overkill for the current cadence.
- **Non-sequential identifiers** (date-stamped or ULID, e.g. `adr-2026-06-14-foo`). Eliminates collisions entirely, but discards the tidy, citable sequential numbering Sonde inherited from Tuxlink and that the rest of the discipline references (`ADR 0002`). Too large a departure for the problem.
- **Do nothing** — rely on ad-hoc PR review to catch duplicate numbers. Rejected: with several sessions live, "someone will notice at review" is exactly the kind of unwritten assumption the governance is meant to replace with an explicit, cheap rule.
