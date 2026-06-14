import assert from "node:assert";
import { toHex, byteDiff, viridis } from "../site/js/format.js";

// toHex
assert.strictEqual(toHex([0x7e, 0x22, 0x04]), "7E 22 04");
assert.strictEqual(toHex([]), "");

// byteDiff: indices where a and b differ (compares min length; extra = differ)
assert.deepStrictEqual(byteDiff([1,2,3],[1,9,3]), [1]);
assert.deepStrictEqual(byteDiff([1,2],[1,2,3]), [2]);     // length mismatch -> trailing differ
assert.deepStrictEqual(byteDiff([],[]), []);

// viridis: returns [r,g,b] 0..255; clamps; monotone-ish endpoints
const lo = viridis(0), hi = viridis(1);
assert.ok(lo.length === 3 && hi.length === 3);
assert.ok(lo[2] > lo[0], "viridis(0) is bluish/purple");   // more blue than red at low end
assert.ok(hi[0] > 150 && hi[1] > 150, "viridis(1) is yellowish");

console.log("format.test ok");
