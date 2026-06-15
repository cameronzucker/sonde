# Handoff — ARDOP demo: connected-session backend + frontend (sonde-imh.2)

- **Agent:** grouse-poplar-fjord
- **Date:** 2026-06-14
- **Branch:** `sonde-imh.1/ardop-live-backend` (worktree `worktrees/sonde-imh.1-ardop-live-backend`)
- **Issue:** `sonde-imh.2` (real ARDOP connected mode) — implementation done; in_progress pending operator verification + PR merge
- **Builds on:** raven-tamarack-pika handoff (`2026-06-14-raven-tamarack-pika-ardop-connected-mode.md`)

## UPDATE — LIVE REWORK (same session, after operator laptop review)
Operator review found the first cut had the **wrong shape**: it was run-then-replay
(Run session → wait → press "Play audio" to see the waterfall), audio didn't play on
its own, and the multi-panel page needed constant scrolling. Reshaped to **live + one
screen** (commits `f7ed0ee`, `1bb892d`):

- **Live on-air audio over SSE.** The backend now tails the growing on-air tap file
  and streams base64 PCM `audio` events interleaved with the protocol events
  (`server.py::_stream_tap_audio`). The frontend (`live-audio.js`) schedules them
  continuously on the AudioContext clock; the AnalyserNode drives the waterfall. So
  **audio + waterfall are LIVE** as the modems transmit — no replay step. The proven
  `arecord|channel_filter|aplay` OS pipe is untouched (tailing a file can't perturb
  the half-duplex timing).
- **One action.** `Connect` starts a live session; the click also unlocks audio. The
  log, telemetry, and recon image stream live too. Removed the play/scrub/replay
  transport.
- **Single-viewport cockpit.** `index.html` + cockpit CSS: a topbar of levers (SNR /
  condition / bandwidth ceiling) + Connect + mute, a hero live waterfall (left), and a
  telemetry / ARQ-log / recon rail (right, log scrolls internally). Honesty banner +
  explainer moved below the fold. No scrolling to reach the interactive parts.
- **Clean shutdown (SIGTERM/SIGINT).** Killing the server mid-session now aborts the
  session so ardopcf/arecord/aplay are torn down — fixes an orphan-leak that held the
  loopback devices and poisoned later sessions (it caused a long debugging detour;
  if you ever see a stuck demo, `pkill ardopcf channel_filter; pkill -f 'arecord -t raw';
  pkill -f 'aplay -t raw'`). Removed the dead replay-WAV plumbing (live-only now).

Verified headless: a live session streams ~470 `audio` events from the handshake
onward; mid-stream chunks decode to valid 1440-sample 12 kHz PCM; pass 2895/2895;
SIGTERM mid-session leaves **zero** orphaned procs; node --check + served-cockpit DOM
checks all green. **Operator visual/audio verification on a laptop is still the merge
gate** — the Pi can't render or play audio.

New/changed files this rework: `demo/ardop/server.py`; `demo/site/js/live-audio.js`
(new), `session-engine.js`, `main.js`, `controls.js`; `demo/site/index.html`,
`demo/site/css/app.css`; deleted `demo/site/js/audio.js`.

---

## What this session did (first cut — superseded shape, kept for history)
Turned the *proven* connected-mode testbench into a **live, streamed demo** —
backend service + reshaped frontend — and retired the superseded one-way PHY path.

### Backend (commit `a3a982b`) — verified HEADLESS
- `channel_filter.py`: new `--tap <path>` writes the impaired on-air PCM (what the
  RX hears) to a file → the live waterfall plays the honest on-air spectrum.
- `testbench.py`: extracted **`run_session(params, emit, should_abort)`** from
  `main()`. The orchestration now emits structured milestone events
  (`phase/station/host/connected/data_start/progress/mode/delivered/result/error/done`);
  the CLI is a thin pretty-printer. `should_abort` tears ardopcf down cleanly.
- `server.py`: **SSE `GET /api/session?snr=&condition=&seed=&arqbw=`** streams those
  events and serves the teed on-air WAV (`/api/audio`). Sessions are serialized on a
  lock with **latest-lever-wins abort** (a new request signals the running one to
  release). Retired the PHY routes (`/api/run`, `/api/modes`, `/api/frames`,
  `/api/run_once`, the Auto ladder); inlined the tiny WAV writer so the backend no
  longer imports `ardop_channel`.
- Evidence: full SSE run at SNR 18/20 → `connected` BW **2000**, `mode` adaptation
  **4PSK.1000→2000**, `progress` to **2895/2895**, `result` outcome **pass**,
  `image_hex` = real JPEG (`ffd8…JFIF`), `/api/audio` → valid 16-bit/12 kHz WAV,
  clean teardown, **no leaked procs**. Routing: PHY routes now 404; bad
  condition/arqbw → 400 before any SSE byte.

### Frontend (commit `450cea3`) — headless-checked only
- `session-engine.js` (new): `EventSource` driver; closes on `done` so it never
  auto-reconnects into a *new* session; decodes the on-air WAV.
- `session-log.js` (new): the **Connection / ARQ log** (Panel 05) — handshake,
  negotiated BW, rate-adaptation steps, ARQ delivery progress.
- `controls.js`: levers are now SNR + condition + **ARQ bandwidth ceiling**
  (200/500/1000/2000 Hz MAX). ARDOP negotiates/adapts the actual mode. A session is
  real airtime → explicit **"Run connected session"** button (no auto-run on slider
  tick), with latest-lever-wins cancel.
- `main.js`: wires the streaming events to the views (status, negotiated-mode
  telemetry, ARQ log, recon image from delivered ARQ bytes, on-air waterfall).
- `image-reveal.js`: `showFailed(line1, line2)` now parameterized (RECEIVING…,
  CONNECT FAILED).
- `index.html` + `app.css`: honesty banner + explainer rewritten to the **ARQ
  story** (intact at workable SNR; rate adapts down; CONNECT fails below the cliff);
  bandwidth-ceiling control, run button, negotiated-BW stat.
- Deleted superseded modules: `ardop-engine.js`, `frame-log.js`, `playback.js`
  (PHY) and the earlier dead `engine.js`, `console.js`. `format.js` kept (waterfall).
- Headless checks: `node --check` all modules ✓; served page exposes the new DOM
  ids and none of the removed ones ✓; every module loads 200 ✓.

### Decisions taken with the operator (this session)
- **Mode control → bandwidth-ceiling lever** (not read-only). Negotiated/adapting
  mode is a readout.
- **Full narrative reframe** (banner + explainer + Panel 05 → ARQ log).
- PR boundary: **one combined PR** from this branch (folds imh.1 PHY MVP + imh.2
  connected mode), per the session prompt.

## Environment left UP for operator verification (do not tear down)
- `snd-aloop` loaded as two cards (card 10 `aldA`, card 11 `aldB`). Restore if lost:
  `sudo modprobe -r snd-aloop; sudo modprobe snd-aloop enable=1,1 index=10,11 pcm_substreams=1 id=aldA,aldB`
- **PipeWire user stack stopped** (grabs the loopback control device). Reversible:
  `XDG_RUNTIME_DIR=/run/user/1000 systemctl --user start pipewire pipewire-pulse wireplumber`
- Demo server running on **`0.0.0.0:8771`** (LAN, for laptop review). Restart from
  the worktree: `python3 demo/ardop/server.py --port 8771`.
- `ardopcf` at `~/Code/ardopcf-spike/build/linux/ardopcf`. Stray `ARDOPDebug*.log`
  are now gitignored.

## What REMAINS (next session)
1. **Operator VISUAL verification on a laptop** (the Pi can't render a browser).
   Open `http://<pi-lan-ip>:8771/`, press **Run connected session** at:
   - SNR **18**, 2000 ceiling → intact delivery, mode adapts up, waterfall + audio.
   - SNR **0** → adapts the rate **down**, slower, image still lands (give it time).
   - SNR **−15** → **CONNECT FAILED**, nothing delivered (honest cliff).
   - Try a lower ceiling (e.g. 500) → narrower negotiated mode.
   Watch: the ARQ log timeline, negotiated-BW + mode telemetry, the recon image,
   and the live waterfall while audio plays. Each session is ~30–60 s of real
   airtime; the Run button disables while one is in flight.
2. **Open + merge the combined PR** (`gh pr create` from this branch; merge with
   `gh pr merge --merge --delete-branch`). A PR may already be open if this session
   created it — check first.
3. Optional polish surfaced by the visual pass (layout of the new bandwidth control,
   ARQ-log density, recon "RECEIVING…" state).

## Safety
`ardopcf` runs only against virtual audio (snd-aloop): no PTT device, no RF. RADIO-1
governs Sonde's own TX path, not external ardopcf — held throughout. No Rust changed,
so the cargo gates are untouched by this work (CI still runs them on push).
