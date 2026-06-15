#!/usr/bin/env python3
"""Backend physics gate for the multi-frame ARDOP transfer (sonde-imh.1).

Integration test: drives the real ardopcf + hf-channel-sim round-trip, so it needs
both external binaries (skips cleanly if absent). Asserts the honest invariants the
demo depends on:

  - a clean channel recovers the WHOLE payload (every frame decodes);
  - a hopeless channel loses every frame (no false "recovered" bytes);
  - on a decode PASS the reassembled bytes EQUAL the transmitted ground truth
    (the CRC-trust assumption the reassembly is built on);
  - the spectrogram is a well-formed 0..255 grid of the advertised shape.

Runnable directly (`python3 demo/ardop/test_transfer.py`) or under pytest. WAV files
only; no real radio. ardopcf is external/reference (clean-sheet / ADR 0014).
"""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
import ardop_channel as ac  # noqa: E402

_HAVE_BINS = os.path.exists(ac.ARDOPCF) and os.path.exists(ac.WAVCHAN)
_SKIP = "ardopcf/wav_channel not built (set ARDOPCF/WAVCHAN); see demo/ardop/SPIKE.md"

try:
    import pytest
    _skip = pytest.mark.skipif(not _HAVE_BINS, reason=_SKIP)
except ImportError:  # allow direct `python3 test_transfer.py` without pytest
    def _skip(fn):
        return fn


@_skip
def test_clean_channel_recovers_whole_payload():
    payload = ac.default_payload()
    r = ac.run_transfer(frame="16QAM.2000.100.E", snr_db=30, condition="none", seed=1,
                        payload=payload)
    s = r["summary"]
    assert s["frames_decoded"] == s["frames_total"], r["frames"]
    assert s["frame_loss"] == 0.0
    assert bytes.fromhex(r["recovered_hex"]) == payload  # full, exact recovery


@_skip
def test_hopeless_channel_loses_all_frames():
    r = ac.run_transfer(frame="4PSK.500.100.E", snr_db=-12, condition="none", seed=1)
    assert r["summary"]["frames_decoded"] == 0, r["frames"]
    # Nothing claimed recovered → reassembled buffer is all zeros (holes, no fiction).
    assert set(bytes.fromhex(r["recovered_hex"])) == {0}


@_skip
def test_decoded_frames_equal_ground_truth():
    """The reassembly trusts ARDOP's CRC: any PASS chunk must equal what was sent."""
    payload = ac.default_payload()
    r = ac.run_transfer(frame="4PSK.500.100.E", snr_db=-6, condition="none", seed=1,
                        payload=payload)
    rec = bytes.fromhex(r["recovered_hex"])
    cap = r["capacity"]
    for f in r["frames"]:
        if f["decoded"]:
            lo, hi = f["byte_start"], f["byte_end"]
            assert rec[lo:hi] == payload[lo:hi], f"PASS frame {f['seq']} != ground truth"


@_skip
def test_spectrogram_is_wellformed():
    r = ac.run_transfer(frame="16QAM.2000.100.E", snr_db=20, condition="none", seed=1)
    sp = r["spectrogram"]
    assert sp is not None
    assert sp["rows"] >= 2 and sp["cols"] >= 2
    assert len(sp["mag_q"]) == sp["rows"] * sp["cols"]
    assert min(sp["mag_q"]) >= 0 and max(sp["mag_q"]) <= 255
    assert len(sp["freqs_hz"]) == sp["cols"] and len(sp["times_s"]) == sp["rows"]


@_skip
def test_capacity_map_matches_ardopcf():
    """Guard against drift: the advertised per-frame capacity must match what
    ardopcf actually carries (a single representative frame)."""
    import re
    import subprocess
    import tempfile
    frame = "4PSK.500.100.E"
    tmp = tempfile.mkdtemp()
    tx = ac.tx_wav(frame, bytes(range(256)), tmp)
    rx = os.path.join(tmp, "rx.wav")
    ac.apply_channel(tx, rx, 30, "none", 1)
    out = subprocess.run([ac.ARDOPCF, "--nologfile", "--decodewav", rx, "-y",
                          "--hostcommands", "CONSOLELOG 1"],
                         capture_output=True).stdout.decode("iso-8859-1")
    m = re.search(r"frameLen = (\d+)", out)
    assert m and int(m.group(1)) == ac.FRAME_CAPACITY[frame]


def test_transfer_window_fits_the_default_payload():
    """Regression guard for sonde-0t3: the post-CONNECT transfer window must stay
    large enough for the ~3.3 KB default message to finish on a non-ideal link.

    The default payload fully delivers over a Good @ SNR 10 channel in ~287 s
    (measured); the historic 90 s window only ever fit the pristine Ideal channel,
    so Good/Moderate/Poor silently delivered a partial. Keep a generous floor so a
    future edit can't quietly reintroduce that bug. No binaries needed."""
    import testbench as tb
    assert tb.SessionParams().data_timeout >= 300.0, (
        "data_timeout regressed below the measured Good@SNR10 transfer time "
        "(~287 s) — the default payload will partial on non-ideal channels")


if __name__ == "__main__":
    # The config regression test needs no binaries — always exercise it first.
    print("  test_transfer_window_fits_the_default_payload ...", end=" ", flush=True)
    test_transfer_window_fits_the_default_payload()
    print("ok")
    if not _HAVE_BINS:
        print("SKIP (integration tests):", _SKIP)
        sys.exit(0)
    fns = [v for k, v in sorted(globals().items())
           if k.startswith("test_") and k != "test_transfer_window_fits_the_default_payload"]
    for fn in fns:
        print(f"  {fn.__name__} ...", end=" ", flush=True)
        fn()
        print("ok")
    print(f"\n{len(fns) + 1} backend transfer tests passed.")
