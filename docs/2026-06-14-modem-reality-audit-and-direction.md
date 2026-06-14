# Sonde Modem Reality Audit + Engineering Direction

- **Date:** 2026-06-14
- **Author:** agent cardinal-magnolia-fern (independent read-only audit, 6 parallel subsystem auditors)
- **Status:** Accepted direction (owner decision: build a **real, interoperable HF data modem**)
- **Tracking:** bd `sonde-262` (this doc). Direction maps onto epics **`sonde-64w`** (HF high-speed adaptive stack — PHY/sync/waveform/methodology) and **`sonde-lcw`** (link layer / ARQ / host protocol).
- **Scope of audit:** static source read of current `main`; no build/run/transmit (Part 97).

## 1. Why this exists

A working session built and shipped an interactive WASM demo of "the modem," published it to GitHub Pages, and reported clean BER and ~1.4 kbps throughput. On inspection by the station owner (an HF-experienced operator) the numbers and behavior didn't survive scrutiny. A capability-by-capability audit followed. **The headline finding: the project's *signals of progress* (green CI, passing round-trip tests, a polished demo) were all real, but none of them tested whether the DSP obeys physics.** Every gate validated self-consistent in-process loopback. That is how the project drifted under close attention.

This document is the evidence map and the corrected engineering direction. The owner's decision is to pursue a **real interoperable HF modem**, so the direction below is a physics-gated roadmap, not a relabeling.

## 2. Capability evidence map

Verdicts: 🟢 REAL · 🟡 PARTIAL · 🔴 STUB/FAKED · ⚫ ABSENT.

| Capability | Verdict | Evidence (file-level) |
|---|---|---|
| TX waveform / spectral realism | 🔴 | `ofdm_main/transmitter.rs:75-89`: IFFT → prepend CP → **`.map(|c| c.re)`**; no pulse shaping, windowing, or output filter. Subcarriers only on positive FFT bins (`ofdm_params.rs:67-73`, Wide = bins 15..=113), so `Re{}` injects a **Hermitian mirror image** — not a transmittable SSB signal. Hard symbol-boundary discontinuities → broadband splatter. "2300 Hz" is the nominal subcarrier span only; true occupied BW (sidelobes + image) is far wider. No PAPR/normalization (`audio_io.rs`, `audio_device.rs`). Preamble has the same `.re` issue (`sync/preamble.rs:31-36`). |
| Synchronization | 🔴 | Only **coarse integer-sample preamble correlation** is wired in (`sync/preamble.rs:78-101`, integer `start_sample`, real-valued correlator). CFO estimator (`sync/carrier_offset.rs`), Gardner fine-timing (`sync/symbol_timing.rs`, "scale factor calibrated against the unit test fixture"), and frame-sync FSM (`sync/frame_sync.rs`) **exist but are referenced only by their own tests** — dead in production. No sample-clock tracking, no Doppler. Receiver steps symbols by fixed integer stride assuming RX rate == TX rate exactly (`wideband_lowdensity.rs` `receive_multi`). |
| FEC | 🟢 | Real rate-1/4 LDPC: systematic GF(2) encoder (`sonde-fec/src/encode.rs:74-126`) + **true sum-product soft-decision decoder** (`sonde-fec/src/decode.rs:127-233`, exact `boxplus` in `llr.rs:23-28`). Wired into operational TX/RX/runtime via `with_fec(FloorRate14Codec)` (`sonde-phy-runtime/src/waveform.rs:63`, `sonde-tx/src/lib.rs:207`, `sonde-rx/src/lib.rs:182`). **Caveat:** the WASM demo uses the bare `WidebandLowDensityFloor::new()` = `IdentityFec` (no FEC); no BER-vs-Eb/N0 curve proves the coding gain. |
| RX demod + equalizer | 🟡 | Real pilot channel estimate + **channel-aware max-log LLRs** (`ofdm_main/receiver.rs:65-97`, `constellations.rs:179-213`), wired into the decode path. But `n0 = 0.1` is **hardcoded** (`receiver.rs:92`), single-symbol estimate, no time-tracking, every-4th-bin pilot interpolation. Decodes Watterson Good/Moderate **only** with the real LDPC + hand-fed alignment. |
| Channel model (Watterson) | 🟢 | ITU-R F.520/F.1487 params correct (`hf-channel-sim/src/params.rs:42-58`: Good 0.5 ms/0.1 Hz, Moderate 1.0/0.5, Poor 2.0/1.0, Flutter 0.5/10). Two-tap power-conserving model + proper Gaussian Doppler-PSD fading (`fading.rs:57-105`). The most legitimate component. |
| SNR / BER / throughput methodology | 🔴 | SNR = ad-hoc total signal-power/noise-power over all samples (`noise.rs:46-58`) — **not Eb/N0, not SNR-in-2500-Hz**. "Measured SNR" is a per-occupied-FFT-bin estimate → **processing-gain inflated (~9 dB)** vs the input (`subcarrier_snr`/`link.rs:97-113`). Demo lifts audio as `Complex::new(s,0)` (no Hilbert), adds noise to re+im, decodes only `.re` → **~3 dB optimistic** (`channelize.rs:29,50`). `throughput_bps = payload_bits/airtime`, **reported even when `recovered_ok=false`** (`link.rs:182-184`). Only BER sweep uses **uniform (non-Gaussian) noise** and the IdentityFec path (`examples/ber_vs_snr_sweep.rs:43-51`). No validation against theoretical BPSK/QPSK BER. |
| Link layer / ARQ / connected mode | ⚫ | No ARQ/ACK/NAK/calling/negotiation/addressing/session anywhere (every "ARQ" string is a forward-reference comment). Highest framing layer is a **2-byte length prefix** (`sonde-tx/src/lib.rs:116-134`). Tracked but unbuilt: epic `sonde-lcw`. |
| Rig / PTT hardware I/O | 🟢 | Real serial-RTS `ioctl(TIOCMBIS/TIOCMBIC)` (`sonde-rig-rts/src/linux.rs:124-148`) + CM108 `hidraw` writes (`sonde-rig-cm108/src/hidraw.rs:67-81`). Genuinely keys hardware. |
| phy-runtime transport | 🟡 | Real but minimal: fire-and-forget, half-duplex, single-frame (`sonde-phy-runtime/src/runtime.rs:114-124`). All round-trip proof is **in-process lossless loopback** (`radio.rs` `LoopbackRadio`). **Nothing has ever been on the air** (the real `SoundcardRadio` path is `#[cfg(feature="hardware")]`, never in CI). |

## 3. Honest assessment

**Feasibility: high.** The hard, competent DSP is genuinely present and well-built — a real LDPC sum-product decoder, a real pilot equalizer with proper channel-aware LLRs, a standards-pinned Watterson channel, real PTT keying. These are exactly the parts that are hard to fake, and their quality says the project *can* do real comms engineering.

**What's actually missing is everything between "good building blocks" and "a modem," plus the honesty layer:**
1. The waveform is **not physically transmittable** (no passband modulation, no shaping/filtering) — foundational.
2. There is **effectively no synchronization** in the loop — the capability that makes a modem work when RX doesn't share TX's clock/carrier — and tests hide this by bypassing sync.
3. The **performance numbers are physically meaningless** (non-standard SNR, processing-gain inflation, discarded noise, throughput on failed links, no BER-vs-theory). This is the mechanism behind "those rates at those SNRs are impossible": the SNR axis is not a real HF SNR.
4. There is **no system above the PHY** (no link layer/ARQ), and **nothing has been on-air**.
5. The **adaptation ladder doesn't extend to a weak-signal floor**. Its lowest rung is a wide-band mid-rate mode that fails ~15–20 dB above FT8's decode floor, so marginal links die instead of dropping to a usable trickle — contrary to the EmComm intent. See §5 P3b.

The gaps are known and tractable, not mysterious. This is recoverable — but only by changing what the process gates on.

## 4. The discipline change (load-bearing)

**No capability counts as "done" until a physics gate proves it. Loopback round-trips and green CI are not evidence of HF viability.** Self-consistent TX/RX loopback with a non-physical waveform convention will pass BER=0 forever and prove nothing. Every item in §5 carries an explicit physics acceptance gate; that gate, not "tests pass," is the definition of done.

## 5. Direction — prioritized, physics-gated

Build in this order. Each gate must be met before the item is "done."

**P0 — Methodology first (blocks trusting any number).**
- Replace the ad-hoc SNR with a **standard reference** (Eb/N0 and/or SNR-in-2500-Hz); label it everywhere. Use real Gaussian AWGN (kill the uniform-noise stub).
- Fix the channelization to a consistent analytic (Hilbert) lift so the imaginary-noise half isn't discarded.
- Report **success-gated goodput**, never offered bitrate on a failed link.
- **Gate:** an uncoded BPSK/QPSK BER-vs-Eb/N0 curve matching the theoretical `erfc`/Q-function within ~1 dB. Until this passes, all current SNR/BER/throughput numbers are void.

**P1 — Make the waveform transmittable (foundational).** *(epic `sonde-64w`)*
- Hermitian-symmetric IFFT (or explicit complex-baseband → real-passband upconversion) — emitted real signal occupies the intended SSB audio band with **no mirror image**. Add inter-symbol windowing (raised-cosine overlap-add) + output band-limiting; manage PAPR + normalization.
- **Gate:** computed PSD shows occupied bandwidth within a defined mask (e.g. ≤2.7 kHz at −26 dBc), no spectral image, bounded PAPR.

**P2 — Real synchronization in the loop.** *(epic `sonde-64w`)*
- Repeated-pair (Schmidl–Cox) preamble so the existing CFO estimator can run; wire CFO derotation; wire Gardner fractional timing; add sample-clock tracking/resampling; complex matched filter for acquisition.
- **Gate:** end-to-end decode over Watterson with **injected carrier offset (±~100 Hz), sample-clock error (±ppm), and fractional timing**, through the *production* sync path. The hand-aligned bypass in the fading tests must be **removed** — those tests must pass with sync in the loop or they don't count.

**P3 — Integrate + validate the coded mode over fading.** *(epic `sonde-64w`)*
- Compose FloorRate14 LDPC + equalizer + real sync into one mode; estimate `n0` (drop hardcoded 0.1); add time/Doppler tracking.
- **Gate:** stated success-rate over Watterson Good/Moderate/Poor at stated Eb/N0, **and** a coded-vs-uncoded BER curve showing the expected LDPC coding gain in dB.

**P3b — Deep-robustness (FT8-class) floor + adapt *down*, don't fail.** *(epic `sonde-64w`)*
- **Problem:** the auto ladder (`modes.rs::resolve`) bottoms out at `floor-wblo` — a wide-band (~2.3 kHz), ~1.4 kbps BPSK mode whose stated posture is *"go wider, not denser."* That is the wrong architecture for a low-SNR floor. It craps out **~15–20 dB above FT8's decode floor**: FT8 reaches ~**−21 dB** (2.5 kHz reference) by going **narrow (~50 Hz) + slow (~6 bps) + strong FEC + long integration**; spreading the same power across 46× the bandwidth at ~200× the bit rate cannot compete. So at marginal SNR the link **fails** instead of degrading — the opposite of the EmComm intent (a few bps that still delivers a SITREP at very low SNR beats a dead link).
- **What exists:** the `floor-nfsk` 8-FSK primitive (`robustness_floor/narrow_fsk.rs`, "borrowed from FT8/JS8") is the right *idea* but is (a) a **stub** (`implemented:false`), (b) reachable only via the manual `FloorCrowdedBand` hint — **not** the `MainAuto` SNR ladder, and (c) ~450 Hz / noncoherent / no FEC, so even as designed it won't approach FT8's floor.
- **Required:** build a genuine deep-robustness mode (narrow tens-of-Hz, long symbols, strong FEC, very low rate) and make it the **bottom rung of `MainAuto`**, so the link degrades to a low-rate trickle as SNR falls instead of dropping. Either tighten `floor-nfsk` toward FT8-class parameters (narrower tone spacing, coherent/longer integration, add FEC) or add a new mode beneath it.
- **Gate (depends on P0):** a measured **decode-rate-vs-SNR curve in the standard 2.5 kHz reference** showing the deep-floor mode decodes within a stated margin of FT8's ~−21 dB — not "round-trips in loopback" — **and** `MainAuto` demonstrably *selects* it (not link failure) as SNR drops below the wide-band floor's usable range.

**P4 — Link layer (currently absent).** *(epic `sonde-lcw`)*
- Frame types (data/ACK/NAK/connect/disconnect), source/dest addressing, sequence numbers, **selective-repeat ARQ**, connected-mode state machine, link adaptation that consumes `ChannelQualityReport`.
- **Gate:** reliable in-order delivery over a *lossy* simulated channel (injected frame loss + corruption); connected-mode session establish/teardown.

**P5 — On-air validation (licensee-gated, Part 97).** The only real proof. Until then, every surface labels results "simulation."

## 6. Stop-doing / honesty rules

- Stop reporting throughput on failed links; gate on success.
- Stop using the full-band SNR; adopt and label Eb/N0 or SNR-in-2.5-kHz.
- Stop letting the `IdentityFec` / AWGN-only / no-Hilbert config represent performance.
- Treat green loopback tests and green CI as **non-evidence** for HF viability.
- The interactive demo (epic `sonde-669`) must not display performance/SNR/bandwidth numbers until P0 lands; it currently overstates a non-physical waveform. Its visual bugs are deferred behind this work.

## 7. Decision recorded

The station owner has chosen to pursue a **real, interoperable HF data modem**. This document is the direction of record; the physics gates in §5 are mandatory. A future ADR may promote the §4 discipline ("physics-truth gates before a capability is done") to the decision log.
