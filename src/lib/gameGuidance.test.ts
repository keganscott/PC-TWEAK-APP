import { describe, expect, it } from "vitest";

import * as fx from "../generated/fixtures";
import { GAME_GUIDANCE } from "./gameGuidance";

describe("game guidance", () => {
  it("covers every game the engine knows, and only those", () => {
    const known = fx.systemAudit.antiCheat!.perGame.map((g) => g.gameId).sort();
    expect(known.length).toBeGreaterThan(0);
    expect(Object.keys(GAME_GUIDANCE).sort()).toEqual(known);
  });

  it("advice always names its source, and no source means no advice", () => {
    for (const [id, g] of Object.entries(GAME_GUIDANCE)) {
      expect(g.source === null, id).toBe(g.steps.length === 0);
    }
  });
});
