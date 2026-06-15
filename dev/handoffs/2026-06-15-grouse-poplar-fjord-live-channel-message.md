# Handoff — live real-channel ARDOP demo + S-meters + message payloads (sonde-imh.2)

> **CORRECTION — 2026-06-15 (agent magpie-juniper-alder).** The "PR #44 merged
> only an early slice; main is MISSING the rest" framing below is **wrong as of
> this date.** Git history shows PR #44's merge commit (`9d46730`) has `3b75c20`
> (the message-payloads commit, the branch's final substantive commit) as its
> **second parent** — i.e. **#44 merged the FULL branch**: real Watterson channel
> (`hf-channel-pcm`), S-meters, SNDM message payloads, and the corrected
> `sonde_through_channel.rs` example are **all already on `main`**. Every work
> file was confirmed present on `origin/main`. **PR #51 therefore carries no
> functional change — only this handoff doc.** It is being merged as a docs-only
> record. The earlier `e19b636` "correction" commit (which re-asserted the false
> claim) is itself superseded by this note. The laptop visual verification is
> still worthwhile as a "confirm the shipped demo works" check, but it is **not a
> merge gate** — the demo already shipped via #44.

- **Agent:** grouse-poplar-fjord
- **Date:** 2026-06-15
- **Branch:** `sonde-imh.1/ardop-live-backend` → **PR #51** (docs-only; superseded
  by the correction above — all functional work landed earlier via **PR #44**).
- **Worktree:** `worktrees/sonde-imh.1-ardop-live-backend`
- **Builds on:** `2026-06-14-grouse-poplar-fjord-ardop-connected-session.md`

## What this session shipped (all on PR #44)
1. **Live on-air audio over SSE** — backend tails the on-air tap and streams base64 PCM
   `audio` events as the modems transmit; frontend `live-audio.js` plays them
   continuously and the waterfalls/S-meters run off the analysers. No record-then-replay.
   SIGTERM/SIGINT teardown in `server.py` (killing the server mid-session no longer
   orphans ardopcf/arecord/aplay).
2. **Two adjacent waterfalls** — A→B (data, teal) and B→A (ACK/NAK, amber), each its own
   direction's stream, on a **3D freq/time grid** with Hz labels. Shows the half-duplex
   turn-taking (you can SEE/HEAR the ACKs — they were on the untapped reverse path before).
3. **Per-station S-meters** (`s-meter.js`) — S-units (6 dB each), over-S9 as "S9+N",
   anchored S9 ≈ −5 dBFS. Relative display (no calibrated RF reference). Honest constellation
   **stub** (ARDOP exposes no demod symbols; reserved for the Sonde side).
4. **REAL ITU-R F.520 Watterson channel** — new bin **`hf-channel-pcm`** in `hf-channel-sim`
   (`src/bin/hf-channel-pcm.rs` + `AwgnGenerator::add_noise_fixed`). `testbench.py` now bridges
   through it instead of the deleted AWGN-only `channel_filter.py`. The Ideal/Good/Moderate/
   Poor/Flutter selector finally does something — validated: same SNR 10, Ideal passes
   2895/2895 in 53 s, **Poor** delivers only 512/2895 and times out (rate adapts down, NAKs).
5. **Realistic message payloads** — payload is now a self-describing **`SNDM` container**
   (text + image attachment(s), Winlink/ICS-213-flavoured). Default = SITREP + tower photo
   (3321 B). `make_payload.py` CLI composes custom messages (`--text/--text-file`,
   `--image PATH` repeatable, `--label`). `message.js` renders the delivered text + image(s);
   "Recon Attachment" panel → "Delivered Message". Retired `image-reveal.js`.
6. **Honest Sonde-through-channel example** (`crates/sonde-phy/examples/sonde_through_channel.rs`)
   — CORRECTED: the first version called the naive per-symbol primitives (no sync/FEC) and
   badly understated the modem (a "gate-on-physics, not artifacts" own-goal; I walked back the
   "Sonde collapses on multipath" claim). Now drives the production receive chain (coded floor +
   preamble + Watterson + calibrated Eb/N0 + sync/n0/pilot-smoothing/LDPC), mirroring the
   authoritative `tests/step3_coded_fading_gate.rs`. Shows the honest picture: decodes in AWGN
   at ~8–11 dB Eb/N0, needs margin on fading.
7. **Branch synced to `main`** (merge `dbdd241`, clean) — demo branch now carries the CURRENT
   `sonde-phy` (today's physics gates) AND `sonde-link`, so it's positioned to wire Sonde next.

Commits: `ffae8d0` (channel bin) → `b77c983` (real channel) → `a0251fd`/`a189ab2`/`25945e4`
(meters) → `efbea40` (naive example) → `dbdd241` (sync main) → `5986fa3` (example fix) →
`3b75c20` (message payloads). Plus the earlier live-audio/two-waterfall commits.

## State of Sonde's PHY (for context — operator asked "corner-cutting or deferred?")
NOT corner-cutting: methodical, gated construction. Today on main: Step 1 (PAPR/spectrum),
Step 2 (real Schmidl-Cox sync over Watterson), Step 3 (coded over fading, xhw.4), Eb/N0
BER-vs-theory within ~0.1 dB, **vb9 coding-gain bug CLOSED** (PR #42). Robust **floor**
waveform is gated-working; higher-rate **OFDM main family** (`sonde-c7i`) still open. Remaining
gaps are filed, not hidden.

## Verification gate (operator, on a LAPTOP — Pi can't render/play)
Server is **LIVE on `0.0.0.0:8771`** → `http://192.168.20.122:8771/`. Reload and **Connect** at:
- SNR 16, Ideal → intact delivery, rate adapts up, both waterfalls + S-meters live, **Delivered
  Message** shows the SITREP text + tower photo.
- SNR 10, **Poor** → degrades (rate down, NAKs, partial) — the real Watterson channel.
- SNR −15 → CONNECT FAILS.
- Try `make_payload.py --text ... --image yours.jpg` then Connect → your message delivers.
Confirm: layout fits one screen (no clip), S-meters read sensibly, message renders.

## Environment (left UP for verification; restore if lost)
- `snd-aloop` 2 cards (card 10 aldA / 11 aldB): `sudo modprobe -r snd-aloop; sudo modprobe
  snd-aloop enable=1,1 index=10,11 pcm_substreams=1 id=aldA,aldB`
- PipeWire user stack STOPPED (grabs the loopback control device).
- Demo server: `python3 demo/ardop/server.py --port 8771` from the worktree.
- `hf-channel-pcm` built at `target/release/hf-channel-pcm` (`cargo build --release -p
  hf-channel-sim`). ardopcf at `~/Code/ardopcf-spike/build/linux/ardopcf`.
- If a demo wedges (orphaned procs hold the loopback): `pkill -f 'ardopcf --nologfile';
  pkill -f channel_filter; pkill -f 'arecord -t raw'; pkill -f 'aplay -t raw'`. `ARDOPDebug*.log`
  are gitignored.

## What REMAINS
1. ~~**Operator verify + merge PR #51**~~ — **DONE / superseded (see CORRECTION at top).**
   All functional work already landed on `main` via **PR #44** (its merge commit
   `9d46730` has the final work commit `3b75c20` as second parent). PR #51 is
   docs-only and is merged via `gh pr merge 51 --merge --delete-branch` to preserve
   this session record on `main`. Optional: an operator laptop pass to confirm the
   *shipped* demo still renders (not a gate).
2. **Wire Sonde as a connected peer** — `sonde-sc0` (P1). Two SondePhy stations over snd-aloop
   bridged through hf-channel-pcm, each on sonde-link's real-time Driver. BLOCKER: the
   two-station-over-shared-medium path is the link agent's documented follow-up (needs a
   shared-medium Radio + conn_id two-party handshake over audio). Coordinate with the link agent.
3. **Calibrate displayed SNR to a 2.4 kHz reference** — `sonde-8dc` (P2). Currently labeled
   "relative" (honest); follow-up makes absolute dB meaningful.

## Git / coordination notes
- Many agents active (link, phy-quality, snr-adapt). The `block-main-checkout-race` hook denies
  git ops when your shell cwd has drifted to the MAIN checkout under contention — run git from
  **inside the worktree** (cwd = worktree) and it's fine. Read-only ops too.
- The main-sync merge was clean (demo/ + hf-channel-sim/ disjoint from the synced crates).

## Safety
ardopcf + hf-channel-pcm run on virtual audio only (snd-aloop): no PTT, no RF. The Sonde
example is DSP-only (no sonde-tx/PTT). RADIO-1 held throughout.
