# Handoff — PR #51 reconciled + demo transfer-timeout bug fixed (sonde-imh.2 / sonde-0t3)

- **Agent:** magpie-juniper-alder
- **Date:** 2026-06-15
- **Branches landed:** PR #51 (merge `37fcc29`), PR #57 (merge `cb2848f`) — both on `main`.
- **Worktrees touched:** `worktrees/sonde-imh.1-ardop-live-backend` (merged branch; holds the
  built `target/release/hf-channel-pcm` + snd-aloop env) and the **nested**
  `worktrees/sonde-imh.1-ardop-live-backend/worktrees/sonde-0t3-demo-tx-timeout`
  (PR #57 branch, merged; **the live demo server runs from here** — see below).

## What this session did

### 1. Reconciled PR #51 (the live-channel demo) with reality — merged
The starting prompt said "PR #51 carries the real-channel/S-meter/message-payload work;
main is missing it — merge #51 to land it." **That premise was false by the time I looked.**
Git proof: PR #44's merge commit `9d46730` has `3b75c20` (the message-payloads commit, the
branch's final substantive commit) as its **second parent** — so **#44 already merged the
FULL branch**. Every work file was confirmed present on `origin/main`. PR #51's branch added
**only** the session handoff doc (and an earlier `e19b636` "correction" that wrongly
re-asserted "main is missing the rest").

Action (operator chose "fix doc, then merge"): added a dated, attributed **CORRECTION block**
to the grouse handoff superseding the false claims, then merged #51 as a docs-only record
(`37fcc29`). No functional change to main from #51.

### 2. Fixed the demo transfer-timeout bug — merged (PR #57, sonde-0t3)
**Operator-reported during laptop verification:** the ~3.3 KB SNDM message (SITREP + tower
JPEG) only delivered on a pristine Ideal channel; any non-ideal condition delivered a partial.

- **Root cause:** `server.py` builds `SessionParams()` without overriding `data_timeout`, so
  the post-CONNECT window was the dataclass default **90 s**, and stations were told
  **`ARQTIMEOUT 90`**. At the link's effective HF throughput (~440 bps Ideal, far less once
  ARDOP rate-adapts down under fading) a 3.3 KB transfer takes **minutes** — 90 s only ever
  fit Ideal.
- **Fix (operator chose "bigger timeout only", keep the approved payload):**
  `data_timeout` 90 → **480 s**; `ARQTIMEOUT` 90 → **240 s** (ardopcf max; bounds 30..240
  verified in `HostInterface.c`). CONNECT stays capped at 60 s, so the SNR-floor / no-connect
  demonstration is unchanged. Added a binaries-free **regression test** in
  `demo/ardop/test_transfer.py` pinning the `data_timeout` floor.
- **Verified (virtual audio only — snd-aloop, no RF; RADIO-1 held):** headless testbench at
  **Good @ SNR 10** now delivers **3321/3321 B intact in ~287 s** (was ~27 % at the 90 s cutoff).
  480 s gives Good comfortable margin and lets the slower Moderate condition finish; Poor still
  shows honest partial/degradation. **Note:** Moderate/Poor completion times were NOT
  independently re-measured (each run is 5–8 min and contends with the live server); 480 s is
  sized from the Good=287 s data point. A degraded-link transfer is a real ~5 min watch — that
  is the HF physics the operator accepted, not a bug.

## Live environment (LEFT UP for operator verification)
- **Demo server LIVE** at `http://192.168.20.122:8771/` (bound 0.0.0.0), **running the merged
  480 s fix**, launched from the nested `sonde-0t3` worktree with
  `HF_CHANNEL_PCM=<imh.1 worktree>/target/release/hf-channel-pcm` and
  `ARDOPCF=~/Code/ardopcf-spike/build/linux/ardopcf`. snd-aloop cards 10/11 (aldA/aldB) loaded;
  PipeWire stopped. To restart: from a checkout that has the fix (origin/main does),
  `python3 demo/ardop/server.py --port 8771` with those two env vars set.
- If a session wedges: `pkill -f 'ardopcf --nologfile'; pkill -f hf-channel-pcm;
  pkill -f 'arecord -t raw'; pkill -f 'aplay -t raw'`.

## What REMAINS
1. **sonde-sc0 — wire Sonde as a connected peer — still BLOCKED.** Confirmed on main the only
   Radio is single-party `LoopbackRadio` (TX → own RX); the shared-medium / two-party Radio is
   the link layer's documented follow-up (tracked under `sonde-8xw`, "two-party threaded G5").
   `g5_wiring_smoke.rs` itself says so. Do NOT start sc0 until that Radio + the conn_id
   two-party handshake over audio land. Coordinate with the link agent.
2. **Optional demo polish (operator's call):** Moderate/Poor empirical completion times;
   `sonde-8dc` (calibrate displayed SNR to a 2.4 kHz reference, P2).
3. **Worktree disposal (future session):** once the demo server is no longer needed, dispose
   the nested `sonde-0t3` worktree and the `sonde-imh.1` worktree via the ritual in
   `docs/git-strategy.md` (`git worktree remove` is hook-banned). Both branches are merged.

## Beads / git notes
- bd `sonde-0t3` CLOSED (fix landed). `sonde-sc0` OPEN (blocked, link-layer prereq).
- `bd dolt` has no remote configured — issue sync is via git-tracked `.beads/issues.jsonl`
  (churned by many concurrent sessions). This session's bd changes are persisted in local
  embedded dolt; they ride to git whenever `.beads/issues.jsonl` is next committed.
- `gh pr merge --delete-branch` printed a `'main' is already used by worktree` error both times
  — that is the cosmetic LOCAL cleanup failing because `main` is checked out elsewhere; the
  REMOTE merge + branch delete SUCCEEDED both times (verify by `gh pr view <#> --json state`).

## Safety
All demo audio is virtual (snd-aloop): no PTT, no RF. The fix touches only `demo/ardop`
(Python), never `sonde-tx`/PTT. RADIO-1 held throughout.
