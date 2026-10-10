import { describe, expect, it } from "vitest";

import { busText } from "./ExecutionBus";

describe("the activity log as text", () => {
  it("names the build, then each line with its time", () => {
    const at = Date.parse("2026-10-10T08:00:00Z");
    expect(
      busText(
        [
          { id: 1, at, stage: "apply", tweakId: "game_mode", message: "Saved a backup" },
          { id: 2, at, stage: "scan", tweakId: null, message: "Done" },
        ],
        "0.1.0, build 1a2b3c4",
      ),
    ).toBe(
      [
        "PeakTweaks 0.1.0, build 1a2b3c4",
        "2026-10-10T08:00:00.000Z apply [game_mode] Saved a backup",
        "2026-10-10T08:00:00.000Z scan Done",
      ].join("\n"),
    );
  });
});
