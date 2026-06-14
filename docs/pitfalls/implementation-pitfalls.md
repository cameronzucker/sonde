# Sonde — Implementation Pitfalls & Review Findings

> **Purpose:** Document implementation traps, design flaws, and corrected
> decisions that would cause production failures, security vulnerabilities,
> data-correctness bugs, OR regulatory violations if shipped. This document
> is the primary code-review reference for the Sonde codebase.
>
> **What Sonde is:** Sonde is an HF/SSB software modem. It does not merely
> describe radio — it *transmits*: `sonde-tx` asserts PTT on a real rig and
> plays a modulated waveform out the soundcard. That single fact makes the
> first entry in this file the most important one in the repo.
>
> **Relationship to testing-pitfalls.md:** This document specifies *what* to
> implement and *why*. [`testing-pitfalls.md`](testing-pitfalls.md) specifies
> *how to verify* those implementations work correctly. They are
> complementary — cross-references are noted inline. The cardinal example:
> RADIO-1 here (don't let an agent key a transmitter) has a companion
> TEST-RADIO-1 there (don't "verify" anything by transmitting).
>
> **Last validated against codebase:** 2026-06-14 (replace when you re-audit
> against the current code).

---

## How to Use This Document

This document serves three audiences. Start here, then go directly to the
section you need.

**If you're implementing code:** Go to the domain section matching your work
area. Each entry has a clear *Flaw → Why It Matters → Fix → Lesson*
structure. Follow the Fix. The Lesson teaches the generalizable principle so
you'll catch the next instance of the pattern.

**If you're reviewing code:** Read the entries in the domain your change
touches. Any change that adds, moves, or feeds a transmit-capable code path
MUST be checked against §0 (RADIO-1) before anything else.

**If you're maintaining this document:** Every pitfall discovered during
implementation, review, or debugging MUST be added here. See
[How to Add an Entry](#how-to-add-an-entry) at the end of this file. Partial
updates cause drift — a half-recorded pitfall is one the next agent re-learns
the hard way.

---

## Entry Format

Every entry uses a stable ID (e.g. `RADIO-1`, `ORCH-1`) that never changes
once assigned, a one-line title, then four parts:

- **Flaw** — the concrete mistake, with examples in the wild.
- **Why It Matters** — the consequence (regulatory, correctness, or product).
- **Fix** — what to do instead, grounded in actual Sonde surfaces.
- **Lesson** — the generalizable principle, so you catch the *next* instance.

Entries are grouped by domain. The summary table at the end lists them all.

---

# Section 0: Live Radio Transmission

> **Reader context:** I'm building or reviewing any code path that can end in
> an over-the-air transmission under an amateur radio callsign — `sonde-tx`,
> the PTT primitives in `sonde-rig-rts` / `sonde-rig-cm108`, the planned
> `SoundcardRadio` in `sonde-phy-runtime`, or anything that asserts PTT and
> plays audio into a rig.
>
> This section is **§0 because it supersedes every other pitfall.** If you are
> about to trip RADIO-1, stop. Do not continue with the task. Do not run the
> binary. Surface it to the licensee.

---

### RADIO-1: Agent-autonomous transmission without licensee consent  — **CRITICAL**

**The Flaw:** An automation, test, subagent shell, CI job, scheduled task, or
AI agent initiates an over-the-air transmission under the operator's amateur
callsign without the station licensee having given explicit, scoped,
per-invocation consent *at the moment of the run*.

Cached credentials, persisted env vars, a config file, or "the user said yes
last week / last session / when they set this up" are **NOT consent.** Consent
is the licensee, in real-time control, authorizing *this specific
transmission now*.

Examples of this flaw in the wild:

- An agent executing an implementation plan runs `sonde-tx --payload ...
  --device <AUDIO> --ptt-device <TTY>` (full mode) in its own shell to
  "verify the transmit path works end to end."
- A CI workflow invokes a transmit-capable binary against a connected rig on
  every push.
- An integration test discoverable by `cargo test` opens the real audio
  device and asserts PTT because an env var pointing at the hardware happens
  to be set.
- A `/loop` or cron job keys the radio periodically "to monitor for
  regressions on the air."
- An agent reads "the operator wired up the rig earlier in this session" and
  treats that as standing authorization for an unattended test send.

**Why It Matters:** This is a regulatory violation, not a style issue. Under
47 CFR Part 97 the station licensee is personally responsible for every
transmission bearing their callsign, and is required to maintain control over
the station. Unlicensed or unattended transmission is illegal, can cause
harmful interference to other users of a shared spectrum, and the liability
attaches personally to the callsign holder — not to "the project" and
certainly not to "the agent." There is no software convenience that justifies
keying a transmitter without the licensee in the loop.

**The Fix:** Any code path that can transmit must be gated behind an explicit
consent gate, and an agent whose task touches a TX path MUST refuse to run it
in its own shell.

1. **Write the code, commit it, let the licensee run it.** The agent's job on
   a transmit path ends at "the code is written and committed." The licensee
   runs the live binary manually, with hardware they physically control.
2. **Keep transmit-capable code out of `cargo test`-discoverable tests.** Live
   transmission lives in dedicated binaries (`crates/sonde-tx`), never in
   `#[test]` / `#[tokio::test]` functions a subagent shell could trigger by
   running the suite.
3. **Honor the existing dry-run / WAV seams.** `sonde-tx --dry-run` encodes
   the payload and reports the airtime budget *without opening any audio
   device or asserting PTT*; the WAV-file mode writes the waveform to disk for
   offline inspection. These are the agent-runnable paths. Full mode (asserts
   PTT, plays the waveform) is operator-only.
4. **If a task seems to require running a live-radio binary to "verify
   completion," the task is misspecified.** STOP and escalate to the
   licensee. Do not improvise a way to satisfy the acceptance criterion by
   transmitting. Verification of a transmit path is done through doubles (see
   TEST-RADIO-1), not through RF.

**Concrete Sonde surfaces this entry governs:**

- `crates/sonde-tx` — composes the PHY encoder + audio output with the PTT
  primitive; full mode asserts PTT and plays the modulated waveform. Its own
  module docs already carry a `## Safety (RADIO-1)` note: *"This binary MUST
  NOT be run by automation under the operator's callsign without the
  licensee's per-invocation consent. The agent that builds this code does not
  run it against the real device."*
- `crates/sonde-rig-rts` and `crates/sonde-rig-cm108` — the PTT primitives
  (serial-RTS and CM108 HID respectively). Asserting PTT *is* taking the
  channel; treat these as transmit surfaces.
- The planned `SoundcardRadio` in `crates/sonde-phy-runtime` (the production
  `Radio` impl that composes audio + PTT, behind a `hardware` feature). It is
  **therefore deliberately NOT covered by automated tests** — the `Radio`
  seam is exercised by `LoopbackRadio`, an in-memory double, instead. See
  `docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md`.

**The Lesson:** Hardware that emits RF is governed by law, not by best
practice. The agent's default posture is: **"I write transmit code, I never
key the transmitter."** When in doubt about whether a code path transmits,
assume it does and apply the fix — the consent gate is cheap, the incident is
not.

---

# Section 1: Orchestration

> **Reader context:** I'm planning or executing work that decomposes into
> multiple sub-tasks, and I need to decide how to dispatch them.

---

### ORCH-1: Failing to parallelize independent work (or parallelizing coupled work)

**The Flaw:** Two related mistakes that are the same mistake from opposite
sides:

1. **Under-parallelizing:** Several sub-tasks exist that share no state and
   have no sequential dependency between them, but they're dispatched one at a
   time — each waiting for the last to finish — wasting wall-clock time and,
   for an agent orchestrator, context.
2. **Over-parallelizing:** Tightly-coupled sub-tasks — where one's output is
   another's input, or both mutate the same file / branch / shared resource —
   are fanned out concurrently, producing merge conflicts, lost writes, or
   work built on a sibling's not-yet-finished output.

**Why It Matters:** Independent work run serially is pure latency tax;
the cost compounds when each step is itself slow (a full build, a sim sweep, a
bug-hunt pass). Conversely, coupled work run in parallel is a correctness
hazard — the second task reads stale state, two tasks race on the same file,
or an orchestrator consolidates partial results. Getting the
dispatch-shape wrong is its own class of defect, independent of whether each
individual task is correct.

**The Fix:**

1. Before dispatching, ask: **do these sub-tasks share mutable state or have a
   producer→consumer ordering?**
2. If **no** (e.g. independent bug-hunt passes over different crates,
   independent doc edits, independent read-only investigations): dispatch them
   in parallel, in a single batch.
3. If **yes** (e.g. "design the trait" then "implement against the trait", or
   two edits to the same module): sequence them, and let each consumer start
   only once its producer's output is committed/persisted.
4. When parallel dispatches produce findings that would be expensive to
   regenerate, have each worker persist its complete output to a durable path
   *before* returning — the response message is not the sole record (an
   orchestrator's context can compact mid-consolidation and drop it).

**The Lesson:** Match the dispatch topology to the dependency topology.
Parallelism is free speed for independent work and a correctness bug for
coupled work — the deciding question is always "is there shared state or an
ordering constraint?", not "how many tasks are there?"

---

# Section 2: PHY / Modem Design

> **Reader context:** I'm changing the waveform, FEC, modulation, or the
> runtime that drives them across a real channel. The pitfalls here are about
> the gap between "passes in the lab" and "works on the air."

---

### FEC-1: Validating the modem only in clean loopback

**The Flaw:** PHY/FEC changes are validated exclusively against a perfect
in-memory loopback (or, at best, additive white Gaussian noise) where
everything looks correct — and then meet real channel impairment for the
first time *on the air*.

Examples of this flaw in the wild:

- A waveform change is signed off because the `NullPhy` loopback echoes frames
  back cleanly (it sets `frame_snr_db = INFINITY, decode_ok = true` by
  construction — it does not exercise modulation/demodulation at all).
- A new constellation or bit-loading scheme passes its unit tests under AWGN
  and is declared done, with no fading or Doppler in the loop.
- BER numbers are quoted from an uncoded run because "FEC will only help" —
  but FEC was never actually wired in, so the real-channel margin is unknown.

**Why It Matters:** HF channels are nothing like loopback. They fade, they
have Doppler spread, they have multipath and impulsive noise. An uncoded or
untuned waveform that sails through loopback can collapse entirely on a real
channel — and the worst possible place to discover that is on the air, where
you've already (a) spent operator time and RF, and (b) can't iterate quickly.
Loopback proves the plumbing is connected; it proves nothing about whether the
waveform survives a channel.

**The Fix:**

1. Gate every waveform/FEC change behind a **simulated-channel BER test**
   before any hardware testing. The integration point already exists:
   `crates/sonde-phy/tests/sim_adapter.rs` is the single place a maintainer
   wires `hf-channel-sim` (`Channel` / `ChannelCondition`) into per-mode
   round-trip tests. Use it; don't ship waveform changes that have only seen
   loopback.
2. **Wire real FEC before trusting BER numbers.** A BER figure from a path
   that doesn't actually run `sonde-fec` encode/decode is not the BER the
   operator will see. Encode → channel → decode, end to end, then measure.
3. Treat AWGN as the floor, not the bar. Add fading/Doppler conditions from
   the channel simulator before declaring a mode validated.

**The Lesson:** The dress rehearsal comes before the performance. The
simulated channel *is* the dress rehearsal — it's where a waveform earns the
right to touch real hardware. "It works in loopback" is "the actors showed up
to the theater," not "the show is ready."

---

### HALF-DUPLEX-1: Assuming TX and RX can overlap on one rig

**The Flaw:** Designing a transport that transmits and receives concurrently
on a single SSB rig with one soundcard — e.g. a runtime that keeps an RX
capture thread running while the TX path keys PTT and plays a waveform, and
expects both to be meaningful.

**Why It Matters:** Keying PTT *takes the channel*. While you are
transmitting, the rig is transmitting; the receive audio you'd capture during
that window is your own sidetone / nothing useful, not the channel. A single
SSB rig + one soundcard is physically half-duplex. A transport designed as if
it were full-duplex will produce garbage RX during TX, contend for the
soundcard, and encode an abstraction the hardware can't honor.

**The Fix:**

1. The runtime must be **half-duplex by construction.** A transmission owns
   the channel for its full duration: assert PTT → play the waveform → release
   PTT. RX capture happens *only when not transmitting*.
2. Use a **TX-priority pump**: a single worker that drains queued TX frames
   first (asserting PTT, playing, releasing), and otherwise captures a short
   RX window and attempts to decode. This is exactly the shape planned for
   `SondePhy` in `crates/sonde-phy-runtime`
   (`docs/superpowers/plans/2026-06-14-sonde-phy-runtime-adapter.md`): one
   background worker implementing a half-duplex pump over the `Radio` seam.
3. Keep `send_frame` / `poll_rx` / `channel_quality` as thin queue/snapshot
   operations on top of that single worker — never as independent threads that
   could drive TX and RX hardware simultaneously.

**The Lesson:** Model the physical constraint of the hardware, not the
convenient abstraction. A half-duplex rig is half-duplex no matter how clean a
full-duplex API would look; the abstraction must encode the constraint, or the
constraint will reassert itself as a bug on the first real QSO.

---

## How to Add an Entry

When a new pitfall is discovered during implementation, review, or debugging:

1. **Pick the right section.** If none of the existing domains fit, add a new
   `# Section N: <Domain>` with a one-paragraph reader-context note.
2. **Assign a stable ID.** Prefix by domain (`RADIO-`, `ORCH-`, `FEC-`,
   `HALF-DUPLEX-`, …), next free number. IDs are forever — never renumber or
   reuse, even if an entry is later superseded (mark it `SUPERSEDED` in the
   table instead).
3. **Write all four parts.** Flaw → Why It Matters → Fix → Lesson. The Fix
   must reference concrete Sonde surfaces (crate, file, type). The Lesson must
   be the *generalizable* principle, not a restatement of the fix.
4. **Cross-reference [`testing-pitfalls.md`](testing-pitfalls.md)** if the new
   pitfall has a verification companion — and add that companion there. A
   "what to build" entry without a paired "how to verify" entry is half done.
5. **Update the summary table** below in the same change. A new entry that
   isn't in the table is invisible to the next reviewer.

---

## Completeness & Voice

This document is only as good as its discipline. **Every newly discovered
pitfall gets added** — the moment one is found and *not* recorded, the next
agent pays to re-learn it, and the document starts drifting from reality.
Partial updates (entry added but no table row; fix written but no cross-ref;
codebase moved but `Last validated` not bumped) cause exactly the silent drift
this discipline exists to prevent.

Voice: write for the next engineer, in plain professional prose. Lead with the
concrete mistake, not abstract theory. Give examples in the wild. Ground every
fix in a real file path. Earn the Lesson — it should be a principle the reader
can carry to code this document hasn't seen yet.

---

## Summary Table

| ID | Title | Severity | Status | Domain |
|----|-------|----------|--------|--------|
| RADIO-1 | Agent-autonomous transmission without licensee consent | CRITICAL | VALIDATED | §0 Live Radio Transmission |
| ORCH-1 | Failing to parallelize independent work (or parallelizing coupled work) | HIGH | VALIDATED | §1 Orchestration |
| FEC-1 | Validating the modem only in clean loopback | HIGH | VALIDATED | §2 PHY / Modem Design |
| HALF-DUPLEX-1 | Assuming TX and RX can overlap on one rig | HIGH | VALIDATED | §2 PHY / Modem Design |

Severity levels: `CRITICAL` (regulatory violation / production data loss /
security), `HIGH` (correctness bug under predictable conditions), `MEDIUM`
(correctness bug under edge cases), `LOW` (cleanliness / clarity).

Status values: `VALIDATED` (prescribed fix is implemented or codified in the
codebase), `UNIMPLEMENTED` (pitfall documented but fix not yet in code),
`SUPERSEDED` (replaced by another entry or no longer applicable).
