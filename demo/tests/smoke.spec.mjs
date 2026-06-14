import { test, expect } from "@playwright/test";
// Assumes a static server serving demo/site at BASE (see demo/README.md).
const BASE = process.env.DEMO_BASE || "http://localhost:8080";

/** Set a range input's value and fire the `input` event the page listens for. */
async function setRange(page, selector, value) {
  await page.$eval(
    selector,
    (elv, v) => {
      const el = /** @type {HTMLInputElement} */ (elv);
      el.value = String(v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    },
    value,
  );
}

test("loads, runs a link, renders image at high SNR", async ({ page }) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto(BASE);
  await page.waitForSelector("#waterfall-mount canvas", { timeout: 15000 }); // wasm + three up
  // Set SNR high + Ideal channel, Auto.
  await setRange(page, "#snr-slider", 60);
  await page.click('#condition-group [data-condition="none"]');
  // BER readout should reach 0.00% and a mode is chosen.
  await expect(page.locator("#stat-ber")).toContainText("0.00", { timeout: 15000 });
  await expect(page.locator("#stat-mode")).toContainText("floor-wblo");
  expect(errors, errors.join("\n")).toEqual([]);
});

test("multipath degrades without crashing", async ({ page }) => {
  await page.goto(BASE);
  await page.waitForSelector("#waterfall-mount canvas", { timeout: 15000 });
  await setRange(page, "#snr-slider", -6);
  await page.click('#condition-group [data-condition="poor"]');
  // Either non-zero BER or an explicit failed state — must not be a clean 0.00%.
  // (Exact-text match: substring would falsely match "100.00%" against "0.00%".)
  await expect(page.locator("#rx-console")).toBeVisible();
  await expect(page.locator("#stat-ber")).not.toHaveText("0.00%", { timeout: 15000 });
});
