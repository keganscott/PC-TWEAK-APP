import { describe, expect, it } from "vitest";
import { CHECK_MS, mouseRate } from "./mouseRate";

/** Reports `gapMs` apart from `from` to `to` (ms). */
const steady = (gapMs: number, from = 0, to = CHECK_MS) => {
  const out: number[] = [];
  for (let t = from; t < to; t += gapMs) out.push(t);
  return out;
};

describe("the mouse's report rate (Tools > Mouse report rate)", () => {
  it("reads a steady 1000 a second as 1000, matching that setting", () => {
    expect(mouseRate(steady(1))).toEqual({ perSecond: 1000, reports: 4000, setting: 1000 });
  });

  it("reads 125, 500 and 8000 a second as their settings", () => {
    expect(mouseRate(steady(8))?.setting).toBe(125);
    expect(mouseRate(steady(2))?.setting).toBe(500);
    expect(mouseRate(steady(0.125))?.perSecond).toBe(8000);
  });

  it("takes a busy slice, so pauses in the moving do not pull the figure down", () => {
    // Moving for a second, still for two, moving again for one.
    const times = [...steady(1, 0, 1000), ...steady(1, 3000, 4000)];
    expect(mouseRate(times)).toMatchObject({ perSecond: 1000, setting: 1000 });
  });

  it("names no setting for a figure between them", () => {
    expect(mouseRate(steady(1000 / 700))).toMatchObject({ perSecond: 700, setting: null });
  });

  it("says nothing when the mouse hardly moved", () => {
    expect(mouseRate(steady(50))).toBeNull();
    expect(mouseRate([])).toBeNull();
  });
});
