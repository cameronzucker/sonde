// controls.js — lever DOM (SNR / condition / Auto-Manual / mode picker) → state (ES module).
// Owns all lever interaction + the live SNR readout; emits a debounced onChange so the
// host re-runs the link. Does NOT own the transport (play/scrub/speed) — main.js wires that.

const DEBOUNCE_MS = 150;

/**
 * createControls(modes, onChange)
 *   modes    — array of ModeInfo from engine.listModes() (used to populate the picker).
 *   onChange — called (debounced) with getState() whenever a lever changes.
 * Returns { getState }.
 */
export function createControls(modes, onChange) {
  const snrSlider = document.getElementById("snr-slider");
  const snrReadout = document.getElementById("snr-readout");
  const conditionGroup = document.getElementById("condition-group");
  const modeToggle = document.getElementById("mode-toggle");
  const modePicker = document.getElementById("mode-picker");

  const conditionButtons = Array.from(conditionGroup.querySelectorAll("[data-condition]"));
  const toggleButtons = Array.from(modeToggle.querySelectorAll("[data-mode-sel]"));

  // ── Populate the manual mode picker from the catalogue ────────────────────
  // Implemented modes are selectable; unimplemented ones are disabled + "pending".
  modePicker.innerHTML = "";
  (modes || []).forEach((m) => {
    const opt = document.createElement("option");
    opt.value = m.id;
    opt.textContent = m.implemented ? m.id : `${m.id} — pending`;
    opt.disabled = !m.implemented;
    modePicker.appendChild(opt);
  });
  // Default the picker to the first implemented mode.
  const firstImpl = (modes || []).find((m) => m.implemented);
  if (firstImpl) modePicker.value = firstImpl.id;

  // ── Live SNR readout ──────────────────────────────────────────────────────
  function paintSnr() {
    snrReadout.textContent = `${Number(snrSlider.value)} dB`;
  }
  paintSnr();

  // ── Debounced change emission ─────────────────────────────────────────────
  let debounceTimer = null;
  function emitChange() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => onChange(getState()), DEBOUNCE_MS);
  }

  // ── State accessor ────────────────────────────────────────────────────────
  function activeCondition() {
    const btn = conditionButtons.find((b) => b.getAttribute("aria-pressed") === "true");
    return btn ? btn.dataset.condition : "none";
  }
  function isAuto() {
    const active = toggleButtons.find((b) => b.classList.contains("is-active"));
    return !active || active.dataset.modeSel === "auto";
  }
  function getState() {
    const auto = isAuto();
    return {
      snrDb: Number(snrSlider.value),
      condition: activeCondition(),
      auto,
      mode: auto ? null : (modePicker.value || null),
    };
  }

  // ── Wiring ────────────────────────────────────────────────────────────────
  snrSlider.addEventListener("input", () => { paintSnr(); emitChange(); });

  conditionButtons.forEach((btn) => {
    btn.addEventListener("click", () => {
      conditionButtons.forEach((b) => b.setAttribute("aria-pressed", String(b === btn)));
      emitChange();
    });
  });

  toggleButtons.forEach((btn) => {
    btn.addEventListener("click", () => {
      toggleButtons.forEach((b) => {
        const on = b === btn;
        b.classList.toggle("is-active", on);
        b.setAttribute("aria-pressed", String(on));
      });
      modePicker.disabled = isAuto();
      emitChange();
    });
  });

  modePicker.addEventListener("change", emitChange);

  // Initial picker enabled-state matches the toggle.
  modePicker.disabled = isAuto();

  return { getState };
}
