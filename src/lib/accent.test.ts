import { describe, expect, it } from "vitest";

import { ACCENTS } from "./accent";

describe("accent colours", () => {
  it("offers violet first (the default) with a swatch each", () => {
    expect(ACCENTS.at(0)?.id).toBe("violet");
    expect(new Set(ACCENTS.map((a) => a.id)).size).toBe(ACCENTS.length);
    for (const a of ACCENTS) {
      expect(a.swatch).toMatch(/^#[0-9a-f]{6}$/);
      expect(a.name.length).toBeGreaterThan(0);
    }
  });
});
