// console.js — TX|RX packet console renderer (ES module).
// Renders per-symbol byte rows into two aligned columns (TX and RX),
// highlights bit flips in red, and maintains a cumulative flip count.

import { byteDiff } from "./format.js";

/**
 * createConsole(txEl, rxEl, flipCountEl)
 *
 * @param {Element} txEl       - scrollable container for TX byte rows
 * @param {Element} rxEl       - scrollable container for RX byte rows
 * @param {Element} flipCountEl - element whose textContent holds the flip tally
 * @returns {{ showSymbol(symbol): void, reset(): void }}
 */
export function createConsole(txEl, rxEl, flipCountEl) {
  let flipTotal = 0;

  /**
   * Build an index label span: "#03"
   */
  function makeIdxSpan(idx) {
    const span = document.createElement("span");
    span.className = "sym__idx";
    span.textContent = "#" + String(idx).padStart(2, "0");
    return span;
  }

  /**
   * Render a TX .sym row.
   * Every byte gets class `b b--<field>`.
   */
  function makeTxRow(symbol) {
    const row = document.createElement("div");
    row.className = "sym";
    row.appendChild(makeIdxSpan(symbol.idx));

    const fieldClass = "b--" + symbol.field;
    symbol.bytes.forEach((byte, i) => {
      if (i > 0) row.appendChild(document.createTextNode(" "));
      const span = document.createElement("span");
      span.className = "b " + fieldClass;
      span.textContent = byte.toString(16).toUpperCase().padStart(2, "0");
      row.appendChild(span);
    });

    return row;
  }

  /**
   * Render an RX .sym row.
   * If rx_bytes is empty: show "— (no decode)" idle text.
   * Otherwise: highlight flipped indices with b--flip.
   * Returns the number of byte flips found (0 when no decode).
   */
  function makeRxRow(symbol) {
    const row = document.createElement("div");
    row.className = "sym";
    row.appendChild(makeIdxSpan(symbol.idx));

    if (symbol.rx_bytes.length === 0) {
      const idle = document.createElement("span");
      idle.className = "console__idle";
      idle.textContent = "— (no decode)";
      row.appendChild(idle);
      return { el: row, flips: 0 };
    }

    const flippedSet = new Set(byteDiff(symbol.bytes, symbol.rx_bytes));
    const fieldClass = "b--" + symbol.field;

    symbol.rx_bytes.forEach((byte, i) => {
      if (i > 0) row.appendChild(document.createTextNode(" "));
      const span = document.createElement("span");
      span.className = flippedSet.has(i)
        ? "b " + fieldClass + " b--flip"
        : "b " + fieldClass;
      span.textContent = byte.toString(16).toUpperCase().padStart(2, "0");
      row.appendChild(span);
    });

    return { el: row, flips: flippedSet.size };
  }

  /**
   * Append one symbol's TX + RX rows and update the cumulative flip count.
   * @param {object} symbol - { idx, bytes, rx_bytes, field, byte_start, byte_end }
   */
  function showSymbol(symbol) {
    const txRow = makeTxRow(symbol);
    const { el: rxRow, flips } = makeRxRow(symbol);

    txEl.appendChild(txRow);
    rxEl.appendChild(rxRow);

    flipTotal += flips;
    flipCountEl.textContent = String(flipTotal);

    // Scroll both columns to show the newest row.
    txEl.scrollTop = txEl.scrollHeight;
    rxEl.scrollTop = rxEl.scrollHeight;
  }

  /**
   * Clear both columns and reset the flip counter to zero.
   */
  function reset() {
    txEl.innerHTML = "";
    rxEl.innerHTML = "";
    flipTotal = 0;
    flipCountEl.textContent = "0";
  }

  return { showSymbol, reset };
}
