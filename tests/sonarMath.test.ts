// node --test tests/   (Node 24 runs TypeScript directly)
import { test } from "node:test";
import assert from "node:assert/strict";
import { createLook, easeOut, waveLook, WAVE_MS } from "../src/pill/sonarMath.ts";

test("waves are staggered evenly across the loop", () => {
  const look = createLook();
  assert.ok(waveLook(0, 0, 3, 0.6, look));
  assert.equal(waveLook(WAVE_MS / 3 - 1, 1, 3, 0.6, look), false);
  assert.ok(waveLook(WAVE_MS / 3, 1, 3, 0.6, look));
  assert.equal(look.rise, 0, "wave 1 starts at the bar line when its turn comes");
  assert.equal(waveLook((2 * WAVE_MS) / 3 - 1, 2, 3, 0.6, look), false);
});

test("a wave rises, spreads and fades to 0, then loops", () => {
  const look = createLook();
  waveLook(0, 0, 3, 0.6, look);
  assert.ok(look.rise === 0 && look.opacity === 0 && Math.abs(look.spread - 0.4) < 1e-9);
  let prevRise = 0, prevSpread = 0;
  for (let t = WAVE_MS * 0.1; t < WAVE_MS; t += WAVE_MS / 20) {
    waveLook(t, 0, 3, 0.6, look);
    assert.ok(look.rise > prevRise && look.spread > prevSpread && look.spread <= 1 && look.opacity <= 0.6);
    prevRise = look.rise; prevSpread = look.spread;
  }
  waveLook(WAVE_MS - 1e-6, 0, 3, 0.6, look);
  assert.ok(look.opacity < 1e-6 && look.rise > 0.999);
  waveLook(WAVE_MS, 0, 3, 0.6, look);
  assert.equal(look.rise, 0, "loops back to the bar line");
});

test("easing", () => {
  assert.equal(easeOut(0), 0);
  assert.equal(easeOut(1), 1);
  assert.ok(easeOut(0.5) > 0.5, "ease-out front-loads the motion");
});
