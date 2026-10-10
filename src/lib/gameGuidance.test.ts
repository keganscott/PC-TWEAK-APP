import { describe, expect, it } from "vitest";

import * as fx from "../generated/fixtures";
import { GAME_GUIDANCE } from "./gameGuidance";

describe("game guidance", () => {
  it("covers the plan's three games and three featured shooters, and only games the engine knows", () => {
    // The others have no checked advice yet, so their cards show none.
    const known = fx.systemAudit.antiCheat!.perGame.map((g) => g.gameId);
    expect(known.length).toBeGreaterThan(0);
    for (const id of Object.keys(GAME_GUIDANCE)) expect(known, id).toContain(id);
    expect(Object.keys(GAME_GUIDANCE).sort()).toEqual(["apex", "cod", "fortnite", "minecraft", "roblox", "valorant"]);
  });

  it("advice always names its source, and no source means no advice", () => {
    for (const [id, g] of Object.entries(GAME_GUIDANCE)) {
      expect(g.source === null, id).toBe(g.steps.length === 0);
    }
  });
});
