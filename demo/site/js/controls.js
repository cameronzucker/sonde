// controls.js — lever DOM for the connected-mode ARDOP demo (ES module).
//
// Connected ARDOP negotiates + adapts the DATA mode itself, so the operator no
// longer picks it. The real operator levers are: the channel (SNR + condition) and
// the ARQ BANDWIDTH CEILING (the widest mode ardopcf is allowed to negotiate). A
// connected session is ~30–60 s of real airtime, so we do NOT auto-run on every
// slider tick — the operator dials the levers, then presses "Run connected session".

const ARQBW = ["200MAX", "500MAX", "1000MAX", "2000MAX"];

/**
 * createControls(onRun)
 *   onRun — called with getState() when the operator starts a session.
 * Returns { getState, setRunning(bool) }.
 */
export function createControls(onRun) {
  const snrSlider = document.getElementById("snr-slider");
  const snrReadout = document.getElementById("snr-readout");
  const conditionGroup = document.getElementById("condition-group");
  const bwGroup = document.getElementById("bw-group");
  const runBtn = document.getElementById("run-btn");

  const conditionButtons = Array.from(conditionGroup.querySelectorAll("[data-condition]"));
  const bwButtons = Array.from(bwGroup.querySelectorAll("[data-arqbw]"));

  // ── Live SNR readout ──────────────────────────────────────────────────────
  function paintSnr() {
    snrReadout.textContent = `${Number(snrSlider.value)} dB`;
  }
  paintSnr();

  // ── State accessors ───────────────────────────────────────────────────────
  function activeCondition() {
    const btn = conditionButtons.find((b) => b.getAttribute("aria-pressed") === "true");
    return btn ? btn.dataset.condition : "none";
  }
  function activeArqbw() {
    const btn = bwButtons.find((b) => b.getAttribute("aria-pressed") === "true");
    return btn ? btn.dataset.arqbw : "2000MAX";
  }
  function getState() {
    return {
      snrDb: Number(snrSlider.value),
      condition: activeCondition(),
      arqbw: activeArqbw(),
    };
  }

  // ── Segmented-group selection helper ──────────────────────────────────────
  function wireSegmented(buttons) {
    buttons.forEach((btn) => {
      btn.addEventListener("click", () => {
        buttons.forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
      });
    });
  }
  wireSegmented(conditionButtons);
  wireSegmented(bwButtons);

  snrSlider.addEventListener("input", paintSnr);
  runBtn.addEventListener("click", () => onRun(getState()));

  // ── Run-button busy state (disabled + relabelled while a session streams) ──
  function setRunning(running) {
    runBtn.disabled = running;
    runBtn.setAttribute("aria-busy", String(running));
    const txt = runBtn.querySelector(".runbtn__txt");
    if (txt) txt.textContent = running ? "Session running…" : "Run connected session";
  }

  return { getState, setRunning };
}
