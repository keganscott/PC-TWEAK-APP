import { describe, expect, it } from "vitest";

import { describeEffect, describeItem, describeState } from "./systemItems";

describe("system item wording", () => {
  it("names each kind the way the engine does", () => {
    expect(describeItem({ kind: "service", name: "WSearch" })).toBe("service WSearch");
    expect(describeItem({ kind: "nvidia_setting", profile: "", setting: 0x1057eb71 })).toBe(
      "NVIDIA setting 0x1057EB71 (global profile)",
    );
    expect(describeItem({ kind: "interface_metric", interface: "ab", ipv6: true })).toBe("IPv6 interface metric of adapter ab");
    expect(describeState({ state: "dword", value: 0 }, { kind: "interface_metric", interface: "ab", ipv6: false })).toBe("automatic");
    expect(describeState({ state: "dword", value: 0 })).toBe("0");
    expect(describeState({ state: "absent" }, { kind: "nvidia_setting", profile: "", setting: 0x1057eb71 })).toBe(
      "driver default",
    );
    expect(describeState({ state: "absent" })).toBe("not present");
    expect(describeState({ state: "list", items: [] })).toBe("automatic");
    expect(describeState({ state: "service", start: "delayed_automatic", running: true })).toBe(
      "delayed automatic, running",
    );
    expect(describeEffect({ effect: "refresh_policy" })).toBe("refresh Windows policy");
  });
});
