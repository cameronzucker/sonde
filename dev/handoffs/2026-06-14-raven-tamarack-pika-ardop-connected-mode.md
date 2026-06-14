# Handoff — ARDOP demo: frontend re-anchor + REAL connected mode

- **Agent:** raven-tamarack-pika
- **Date:** 2026-06-14
- **Branch:** `sonde-imh.1/ardop-live-backend` (all work pushed)
- **Issues:** `sonde-imh.1` (demo re-anchor), `sonde-imh.2` (connected mode)

## Arc
Continued the ARDOP demo. Operator review (on a laptop — the Pi can't render a
browser, see memory) drove three corrections that reshaped the work:
1. Waterfall was a fake (per-run-normalized) static 3D blob → rebuilt as a **genuine
   live spectrogram** (Web Audio AnalyserNode FFT of the playing audio, honest SNR).
2. Recon image was an all-black blob → **regenerated** as a recognizable dusk
   antenna-tower scene (`demo/ardop/make_payload.py`).
3. The demo was **PHY-only** (one-way frames, no protocol) → built **real ARDOP
   connected mode**.

## What's DONE (committed + pushed)
- **Frontend re-anchor** on the ARDOP backend (`ardop-engine.js`, frame-driven
  `main.js`, `frame-log.js`, live `waterfall.js`, analyser tap in `audio.js`,
  re-anchored `index.html`/CSS). Operator reviewed the waterfall — works.
- **Real ARDOP connected mode** (`demo/ardop/testbench.py` + `channel_filter.py`),
  PROVEN end to end:
  - Two `ardopcf` stations, audio bridged through `hf-channel-sim` via two
    `snd-aloop` cards (bidirectional-per-card), driven over the ARQ host TCP protocol.
  - Real CONNECT handshake → bandwidth negotiation → **adaptive-rate ARQ data
    transfer** → delivery. Evidence: SNR 15 → image 2895/2895 intact, adapt UP
    4PSK.1000→2000; SNR 0 → adapt DOWN 4PSK.1000→500 (slower, partial in 150 s);
    SNR −12/−18 → connect FAILS (honest cliff). SNR calibrated (`REF_RMS=17900`,
    measured at the modem's real loopback output).

## Environment state (IMPORTANT for the next session)
- **`snd-aloop` is loaded as two cards** (not the default one). To restore after a
  reboot / if missing:
  ```
  sudo modprobe -r snd-aloop
  sudo modprobe snd-aloop enable=1,1 index=10,11 pcm_substreams=1 id=aldA,aldB
  ```
  (passwordless sudo works on this Pi.) Verify: `aplay -l` shows card 10 `aldA`,
  card 11 `aldB`.
- **PipeWire user stack is STOPPED** (it grabbed the loopback control device).
  Reversible: `XDG_RUNTIME_DIR=/run/user/1000 systemctl --user start pipewire
  pipewire-pulse wireplumber`. Not needed for the testbench.
- **Demo web server** was running on `0.0.0.0:8770` (LAN, for laptop review at
  `http://192.168.20.122:8770/`). It may have been stopped; restart with
  `python3 demo/ardop/server.py` from the worktree.
- Run the testbench: `python3 demo/ardop/testbench.py --snr <dB> --condition none`.
  Bridge buffer is 100 ms (40 ms under-runs → `sync_ptr1 Broken pipe`). If a run is
  `timeout`-killed, the SIGTERM handler cleans up; if procs ever leak and hold the
  devices, kill `arecord`/`aplay`/`ardopcf`/`channel_filter` by PID (NOT `pkill -f`
  with those names — it self-matches the shell and returns 144; use bracketed
  patterns or PIDs).

## What REMAINS (next chunk — `sonde-imh.2`)
1. **Backend `/api/session`**: wrap `testbench.py` as a service — on an operator
   lever change (SNR/condition), run one connected session and stream the event
   timeline (handshake states, negotiated BW, data modes used, ARQ progress, bytes
   delivered, recovered image) + tee the on-air audio for the live waterfall.
   Supersedes the PHY-only `/api/run`.
2. **Frontend connected-session UI**: show the CONNECT/handshake, the negotiated +
   adapting data mode (replacing the heuristic Auto ladder), an ARQ progress/retry
   view (frame-log → ARQ-block log), the image rebuilding from delivered data, and
   the live waterfall fed by the session audio.
3. Then clean up: remove the now-dead PHY `/api/run` static spectrogram, decide the
   PR boundary (sonde-imh.1 PHY vs sonde-imh.2 connected mode — likely fold into one
   "real ARDOP demo" PR since PHY-only was rejected as incomplete).

## Pending decisions
- PR organization: sonde-imh.1 (PHY) is superseded by sonde-imh.2 (connected). Likely
  one combined PR titled for the connected-mode demo.
- Throughput/latency: a full transfer at low SNR is slow (real ARDOP airtime). Demo
  UX may want a smaller payload or a progress indicator rather than waiting it out.

## Safety
ardopcf runs only against virtual audio (snd-aloop) — no PTT device, no real radio,
no RF. RADIO-1 governs Sonde's own tx path, not external ardopcf, but the same
no-accidental-TX discipline held throughout.
