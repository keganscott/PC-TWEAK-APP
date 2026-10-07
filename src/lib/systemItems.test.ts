import { describe, expect, it } from "vitest";

import { describeEffect, describeItem, describeState } from "./systemItems";

describe("system item wording", () => {
  it("names each kind the way the engine does", () => {
    expect(describeItem({ kind: "service", name: "WSearch" })).toBe("service WSearch");
    expect(describeItem({ kind: "nvidia_setting", profile: "", setting: 0x1057eb71 })).toBe(
      "NVIDIA setting 0x1057EB71 (global profile)",
    );
    expect(describeState({ state: "list", items: [] })).toBe("automatic");
    expect(describeState({ state: "service", start: "delayed_automatic", running: true })).toBe(
      "delayed automatic, running",
    );
    expect(describeEffect({ effect: "refresh_policy" })).toBe("refresh Windows policy");
  });
});
