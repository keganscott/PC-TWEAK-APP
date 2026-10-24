import { describe, expect, it } from "vitest";

import { sparkPoints } from "./LiveDashboard";

describe("sparkPoints", () => {
  it("puts the newest reading at the right edge and scales 0-100 into the graph", () => {
    expect(sparkPoints([0, 100], 3)).toEqual(["50,29 100,1"]);
  });

  it("leaves a gap where a reading was not taken", () => {
    expect(sparkPoints([50, null, 50, 50], 4)).toEqual(["0,15", "66.67,15 100,15"]);
  });

  it("caps readings outside 0-100", () => {
    expect(sparkPoints([150], 2)).toEqual(["100,1"]);
  });
});
