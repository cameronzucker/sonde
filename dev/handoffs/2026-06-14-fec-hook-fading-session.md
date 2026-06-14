# Handoff — FEC slice shipped, hook bug fixed, floor-fading fix in progress

**Date:** 2026-06-14 · **Agent:** mesa-falcon-basil · **Context:** hit the context wall mid-fading-fix.

This was a long multi-thread session. Three workstreams; the first two are **done + merged to main**, the third is **in progress (RED)** and is where the next session resumes.

---

## 1. FEC-first floor LDPC wiring — DONE, MERGED (bd sonde-64w.1, epic sonde-64w)

Real rate-1/4 LDPC wired end-to-end into the robustness floor, through `sonde-tx`/`sonde-rx`, proven by a differential channel-sim gate. **Merged: PR #5, merge `65ed5b2`.** 340 workspace tests green at merge.

- IRA/dual-diagonal floor code + `FloorRate14Codec` (`sonde-fec`); `coded_framing` + single coded soft-LLR path in the floor (`sonde-phy`); codec injected in `sonde-tx::encode_payload` + `sonde-rx::decode_one_symbol`; `sonde-tx/tests/fec_differential_gate.rs`.
- Spec/plan: `docs/superpowers/specs/2026-06-14-fec-floor-wiring-design.md`, `docs/superpowers/plans/2026-06-14-fec-floor-wiring.md` (on main).
- **The differential gate is AWGN-only** specifically because of workstream #3's fading bug. Once #3 lands, that gate can be upgraded to a fading condition.

## 2. Commit-discipline hook bug — DONE, MERGED (bd sonde-ge9.1, closed)

The `check-commit-discipline.sh` hook blocked ALL worktree commits (not just main). **Root cause:** it resolved the branch via `cd "$REPO"` (the main checkout, which sits on `main` under ADR 0002), so every worktree commit was misclassified as a `main` commit. **Fix (one line):** resolve from the payload `.cwd`, mirroring `block-main-checkout-race.sh`. **Merged: PR #8, merge `4025485`.**

- **Lessons (important):** (a) I first over-engineered this into a 4-Codex-round adversarial-parser rewrite — the operator stopped me; it's the wrong threat model. This hook is a **guardrail against an accidental `main` commit by a cooperative agent**, NOT an adversarial security boundary. The over-engineered version (PR #7) was **closed**. (b) The "realign with Tuxlink" question resolved to: **Tuxlink already uses main-as-integration too** and has the **same latent hook bug**; it just doesn't trip it because its main checkout sits on a task branch. No branch migration needed; the one-liner is the fix.
- A background security review flagged the surviving `-C`/`--git-dir` override bypass → **acknowledged as an accepted tradeoff** (cooperative-agent model; operator-informed). Do NOT reopen the hook to "harden" it.
- **Loose end:** the **main checkout** (`/home/administrator/Code/sonde`) carries the same one-line fix as a *live uncommitted edit* to `.claude/hooks/check-commit-discipline.sh`. It's identical to what's now on `origin/main`; it reconciles when the main checkout next pulls. Harmless. (Don't `git restore` it — the main checkout's local HEAD is behind origin/main, so restore would re-introduce the buggy version.)

## 3. Floor fading-decode fix — IN PROGRESS, RED (bd sonde-64w.2) ← RESUME HERE

**Branch `sonde-64w.2/floor-equalizer`** (pushed, tip `06ae8c2`), worktree `worktrees/sonde-64w.2-floor-equalizer`.

**Problem:** the robustness floor decodes clean/AWGN but **fails through any Watterson fading**.

**Root cause (Codex-converged):** the real-only Zadoff-Chu preamble correlator (`crates/sonde-phy/src/sync/preamble.rs`) returns the frame start **LATE** by the channel path delay under a complex/delayed channel (24 samples for Good @0.5ms/48k). The body is sliced at `detection.start + PREAMBLE_LEN` and each symbol's FFT drops the CP then reads `[start+CP .. start+CP+FFT]`. A *late* start pulls samples from the next symbol (the 512-sample CP protects *early* starts, not late) → block-0 (length header) demod garbage → `FrameDetect("coded block truncated")`. Sync **succeeds**; demod of block 0 is what fails. With the correct window, block 0 decodes exactly even under a constant complex gain (Codex probe).

**Channel model (Codex-confirmed):** the floor emits REAL passband audio; the Watterson sim is complex-baseband. Apply it faithfully as: real audio → **analytic signal (Hilbert)** → Watterson → AWGN → `.re`. (Feeding `(s,0)` directly is wrong.)

**Seam found:** `hf-channel-sim` was **never wired** to `sonde-phy` (`tests/sim_adapter.rs` was a placeholder, dep commented out). Clean/AWGN-only validation is why this hid. This session finally wired it (the gate below).

**Done this session (committed in `06ae8c2`):**
- `crates/sonde-phy/Cargo.toml`: added `hf-channel-sim` dev-dep.
- `crates/sonde-phy/tests/robustness_floor_fading.rs`: the fading gate (analytic Hilbert helper + Watterson Good/Moderate + AWGN 30 dB + 1024-sample guard tail; IdentityFec floor to isolate sync from FEC). **Confirmed RED before the fix.**
- `wideband_lowdensity.rs`: added `SYNC_WINDOW_GUARD_SAMPLES = 128` and changed the body slice to `(detection.start + PREAMBLE_LEN).saturating_sub(128)` (Codex's A1 fix — start the FFT window 128 early so a late detection lands inside the CP).

**THE BLOCKER:** **A1 (G=128) did NOT make the gate green.** The gate is still RED (Good + Moderate fail). The existing `sonde-phy` suite stays green (36 lib + integration) — no regression. So a fixed early-window guard alone is insufficient; the failure is more than a ≤128-sample late window.

**Next steps (next session):**
1. **Instrument the real offset.** Add a scratch test (the old `zz_fading_repro.rs` was deleted) that prints `detection.start_sample` for the analytic-Good capture vs the known preamble position, and whether the late offset exceeds 128. If it does, raise G or go to step 4.
2. **Rule out frequency-selectivity (likely culprit).** Good = 0.5 ms delay spread = frequency-selective. The equalizer (`ofdm_main/equalizer.rs`) uses **every-4th-pilot LINEAR interpolation**; if the channel varies across sub-carriers faster than that tracks, no timing fix helps. Instrument per-symbol BER / the block-0 header bits under the channel. If this is it, the fix is denser pilots or a better channel estimator — a bigger change than A1.
3. **Larger G** (192/256) — but Codex warned larger G steepens the pilot-to-pilot phase ramp the linear interpolator must handle.
4. **A2 — earliest-peak detector** (Codex's sketch): in `sync/preamble.rs`, pick the earliest correlation peak within a bounded multipath window before the strongest peak (absolute + relative-to-best thresholds), aligning to the direct path. More invasive.
5. Drive **Codex to convergence** on whichever path (per standing directive below), and seam-check.

**Gate to GREEN it against:** `cargo test -p sonde-phy --test robustness_floor_fading` (Good + Moderate must pass). Then full `cargo test/clippy/fmt --workspace`, commit (replace the `wip:` with a real `feat:`/`fix:`), PR, merge.

---

## Standing directives from the operator (apply going forward)
- **Codex adrev to convergence at every phase boundary**, and explicitly **seam-check** ("are these features actually wired, or is there a disconnected seam?") — the unwired-island pitfall has bitten repeatedly (the FEC crate, dead bit-loader, panicking codec, the never-wired channel sim).
- **Signals/DSP design judgment is delegated to me, but converge with Codex first** (memory: `codex-adrev-for-signals-design`).
- **Don't over-engineer / reinvent.** Copy Tuxlink's proven patterns; minimal fixes.
- **Velocity + reliability with up to ~30 concurrent agents, no data loss/collisions** is the operating priority.

## Environment / other worktrees
- `worktrees/sonde-interactive-demo` (branch `sonde-669/interactive-demo`) belongs to **another live session** — do NOT touch.
- ALSA present (1.2.14); `cargo` works. Codex CLI at `/usr/local/bin/codex` (use `codex exec --sandbox read-only|workspace-write -C <dir> - < promptfile`; write prompts to a FILE so the Bash command doesn't trip the commit hook on literal "git commit" text).
- Epic `sonde-64w` (HF high-speed adaptive stack) stays open; after fading, the remaining ladder work (OFDM main-family modes, bit-loading, link adaptation) is untracked beyond the epic.
