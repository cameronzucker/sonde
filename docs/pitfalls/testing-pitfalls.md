# Sonde — Testing Pitfalls

> **Purpose:** A test-discipline reference for the Sonde codebase. Every item
> here exists because it catches a class of bug that has bitten real
> codebases — and, for a modem that transmits, because some "tests" are
> themselves a regulatory hazard if written naively.
>
> **Relationship to implementation-pitfalls.md:**
> [`implementation-pitfalls.md`](implementation-pitfalls.md) specifies *what*
> to implement and *why*. This document specifies *how to verify* those
> implementations work correctly. Cross-references are noted inline. The
> cardinal pair: RADIO-1 there (an agent never keys a transmitter) and
> TEST-RADIO-1 here (never "verify" anything by transmitting).

---

## How to Use This Document

**If you're writing tests:** Read the universal-discipline section, then the
entry for the domain you're testing. Verify your suite covers the relevant
checks; where a check doesn't apply, note explicitly why.

**If you're reviewing tests:** A passing suite with missing coverage is worse
than a failing suite with complete coverage — you don't know what's actually
protected. Use the entries to audit for gaps. For anything touching the
transmit path, TEST-RADIO-1 is non-negotiable.

**If you're maintaining this document:** When a bug reaches integration,
staging, or the air because a test was missing or wrong, add the lesson here.
See [How to Add an Entry](#how-to-add-an-entry). Partial updates cause drift.

---

## Entry Format

Universal disciplines are written as a checklist. Domain-specific pitfalls use
a stable ID (e.g. `TEST-RADIO-1`), a one-line title, then
**Flaw → Why It Matters → Fix → Lesson**, mirroring
[`implementation-pitfalls.md`](implementation-pitfalls.md).

---

## Universal Testing Discipline

These apply to every test in the repo, regardless of domain.

- [ ] **Test behavior, not implementation.** Assert on what the code is
  supposed to *do* (the decoded payload, the BER, the released PTT line), not
  on incidental internals (a private field's value, the exact order of helper
  calls). Implementation-coupled tests break on every refactor and protect
  nothing.
- [ ] **A test that can't fail is worthless.** A test with no meaningful
  assertion, or one whose assertion holds regardless of whether the code is
  correct, is documentation cosplaying as verification. (The existing
  `sim_adapter_integration_point_marker` test is an explicit, *labeled*
  exception — it exists only to keep a scaffold file compiling, and says so;
  don't ship silent versions of it.)
- [ ] **Verify the test fails before you make it pass (TDD red step).** Write
  the test, watch it fail for the *expected reason*, then implement. A test
  that was green before the implementation existed is testing nothing — or
  testing the wrong thing.
- [ ] **Don't assert on incidental output.** Pin the contract, not the cosmetic
  detail. Asserting on an exact log string, a float to 15 digits, or
  whitespace makes the test flaky and obscures what actually matters.
- [ ] **Test output stays clean.** Stray errors, warnings, or stack traces in a
  passing run hide real failures. If a test legitimately produces an error,
  capture it and assert on its content.
- [ ] **Skipped / `#[ignore]`d tests are not passing tests.** Every skip carries
  a reason and a condition for re-enabling. "100 passed, 5 ignored" is not
  "105 passed."

---

## TEST-RADIO-1: Never "verify" by transmitting  — **CRITICAL**

This is the testing-side companion to
[RADIO-1](implementation-pitfalls.md#radio-1-agent-autonomous-transmission-without-licensee-consent--critical).

**The Flaw:** A test, a CI job, or an agent trying to satisfy an acceptance
criterion exercises the modem by keying a real radio — opening the production
audio device and asserting PTT — to "prove the transmit path works."

Examples in the wild:

- An integration test that opens the real soundcard and the real PTT TTY
  because the hardware happens to be connected.
- A CI step that runs `sonde-tx` full mode against an attached rig.
- An agent running the live binary in its own shell to check a box on a plan.

**Why It Matters:** Automated transmission under the operator's callsign is a
Part 97 violation regardless of intent (see RADIO-1 for the full rationale).
"It was just a test" is not a defense to a regulator, and an on-air test can
cause real interference. The transmit path is operator-only; the verification
path must be hardware-free.

**The Fix:** Automated tests and CI MUST exercise the modem through
hardware-free doubles, never by keying a real radio:

- **`LoopbackRadio`** — the in-memory `Radio` double in the planned
  `crates/sonde-phy-runtime`. The end-to-end `PhyTransport` contract test
  (`tests/phytransport_loopback.rs`) drives `SondePhy` through `LoopbackRadio`
  + `FloorWaveform` with no hardware. The production `SoundcardRadio` is
  deliberately left untested (RADIO-1).
- **WAV-file round-trips** — `sonde-tx --dry-run` and its WAV-write mode
  encode/emit the waveform to disk with no audio device and no PTT; assert on
  the file, not on RF.
- **The `hf-channel-sim` simulated channel** — for anything that needs a
  channel between TX and RX (`crates/sonde-phy/tests/sim_adapter.rs`).
- **`NullPhy` loopback** — for the `PhyTransport` contract surface in
  `crates/sonde-phy`.

The decode / offline path is agent-runnable. The transmit path is
operator-only. If a test can only pass by transmitting, it is the wrong test.

**The Lesson:** Verification of a transmit path is done with doubles, not with
RF. The receiver and the offline waveform never break the law; the
transmitter, run by automation, always does.

---

## TEST-FLAKY-THREADS-1: Poll a worker thread to a deadline, never a fixed sleep

**The Flaw:** A test for the threaded `SondePhy` runtime waits for the
background worker to produce a result by sleeping a fixed duration
(`thread::sleep(200ms)`) and then asserting, on the assumption that the worker
"should be done by now."

**Why It Matters:** The `SondePhy` runtime runs a background worker (the
half-duplex pump) and surfaces results through `poll_rx` / a shared snapshot.
A fixed sleep encodes a guess about timing. On a fast dev box the guess holds;
on slow or loaded CI — notably a Raspberry Pi, a first-class Sonde target —
the worker may not be done yet, and the test fails *intermittently*. Flaky
tests erode trust until they're ignored, at which point they protect nothing.
(Conversely, padding the sleep to be "safe" makes the whole suite slow.)

**The Fix:** Poll for the expected condition in a loop, up to a *generous*
timeout, and assert *failure* only when the deadline passes:

- Loop on `poll_rx()` (or the snapshot) until the expected frame/state
  appears or a generous deadline (e.g. a few seconds) elapses; fail with a
  clear message if the deadline is hit.
- Pick the timeout for the slowest realistic target (Pi under load), not the
  dev box. The happy path returns as soon as the condition is met, so a large
  timeout costs nothing when things work.
- Never assert on "how long it took" as a proxy for correctness — assert on
  the *result*.

**The Lesson:** When waiting on a concurrent producer, wait on the *condition*,
not the *clock*. A deadline-bounded poll is correct on both the fastest and
the slowest machine; a fixed sleep is a race you wrote on purpose.

---

## TEST-CHANNEL-SIM-1: BER thresholds belong in sim, not on the air

This is the testing-side companion to
[FEC-1](implementation-pitfalls.md#fec-1-validating-the-modem-only-in-clean-loopback).

**The Flaw:** Bit-error-rate behavior is "validated" by hand-eyeballing a real
on-air capture ("looks clean enough"), or by a loopback run that can't fail,
rather than by an asserted threshold against the channel simulator.

**Why It Matters:** A hand-eyeballed capture is not a regression test — it
can't run in CI, it isn't reproducible, it varies with band conditions, and
(per TEST-RADIO-1 / RADIO-1) producing it means transmitting. A loopback BER
of zero proves only that the plumbing is connected, not that the waveform
survives a channel. Without an asserted, reproducible BER-vs-SNR check, a
waveform regression ships silently and is found on the air.

**The Fix:** Validate BER vs SNR against the channel simulator with asserted
thresholds:

- Run encode → `hf-channel-sim` channel → decode end-to-end (real FEC wired,
  per FEC-1), at defined SNR points and channel conditions (AWGN as the floor,
  plus fading/Doppler).
- Assert the measured BER is at or below a defined threshold for each
  (mode, SNR, condition) point — a hard pass/fail, not a visual impression.
- Wire these through `crates/sonde-phy/tests/sim_adapter.rs`, the designated
  integration point for `hf-channel-sim`. Keep them runnable in CI (no
  hardware).
- A real-radio capture is at most a final operator-run sanity check *after*
  the sim thresholds pass — never the primary or the regression check.

**The Lesson:** A reproducible, asserted threshold in the simulator is a test;
a clean-looking capture is an anecdote. Put the BER bar where it can fail in
CI, not where it can only be admired on a spectrum display.

---

## How to Add an Entry

When a bug reaches integration, staging, or the air because a test was missing
or wrong:

1. **Decide the form.** A cross-cutting habit goes in the
   universal-discipline checklist. A specific, named pitfall gets its own
   `TEST-<DOMAIN>-N` entry with the Flaw → Why → Fix → Lesson structure.
2. **Assign a stable ID** for entry-style pitfalls (`TEST-RADIO-`,
   `TEST-FLAKY-THREADS-`, `TEST-CHANNEL-SIM-`, …). IDs are forever.
3. **Ground the Fix in real test surfaces** — name the crate, the test file,
   the double (`LoopbackRadio`, `NullPhy`, `hf-channel-sim`, the dry-run/WAV
   seam).
4. **Cross-reference [`implementation-pitfalls.md`](implementation-pitfalls.md)**
   if there's a paired "what to build" entry — and ensure that entry exists.
5. **Close the gap, don't just document it.** If you add a check and don't
   write the corresponding test, you've recorded a gap, not fixed one. Write
   the test.

---

## Completeness & Voice

The test suite is the enforcement mechanism for this document. **Every pitfall
that costs us a real bug gets added** — partial updates (a lesson learned but
not written, an entry added but no test behind it) cause the exact drift this
discipline exists to prevent.

Voice: pass/fail clarity over cleverness. "Test X under condition Y" beats a
novel testing philosophy. Lead with the concrete failure mode, ground the fix
in a real surface, and make the Lesson a principle the reader can apply to a
test this document hasn't imagined yet.

---

## Summary Table

| ID | Title | Severity | Status | Pairs with |
|----|-------|----------|--------|------------|
| TEST-RADIO-1 | Never "verify" by transmitting | CRITICAL | VALIDATED | RADIO-1 |
| TEST-FLAKY-THREADS-1 | Poll a worker thread to a deadline, never a fixed sleep | HIGH | VALIDATED | HALF-DUPLEX-1 |
| TEST-CHANNEL-SIM-1 | BER thresholds belong in sim, not on the air | HIGH | VALIDATED | FEC-1 |

Severity levels: `CRITICAL` (regulatory violation / production data loss /
security), `HIGH` (correctness bug under predictable conditions), `MEDIUM`
(edge-case bug), `LOW` (cleanliness / clarity).

Status values: `VALIDATED` (discipline reflected in the codebase or its plans),
`UNIMPLEMENTED` (documented but not yet enforced by a test), `SUPERSEDED`
(replaced or no longer applicable).
