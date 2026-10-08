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
    const antiLag = { kind: "amd_setting", gpu: "PCI\\VEN_1002&DEV_73BF", setting: "anti_lag" } as const;
    const vsync = { ...antiLag, setting: "wait_for_vertical_refresh" } as const;
    expect(describeItem(antiLag)).toBe("AMD Radeon Anti-Lag of graphics card PCI\\VEN_1002&DEV_73BF");
    expect(describeState({ state: "dword", value: 1 }, antiLag)).toBe("on");
    expect(describeState({ state: "dword", value: 0 }, vsync)).toBe("always off");
    expect(describeState({ state: "dword", value: 1 }, vsync)).toBe("off unless the game asks");
    expect(describeState({ state: "dword", value: 9 }, vsync)).toBe("9");
    expect(describeState({ state: "dword", value: 1 }, { ...antiLag, setting: "anti_lag_level" })).toBe("Anti-Lag Next");
    expect(describeItem({ ...antiLag, setting: "chill" })).toBe("AMD Radeon Chill of graphics card PCI\\VEN_1002&DEV_73BF");
    expect(describeState({ state: "absent" }, vsync)).toBe("not on this card");
    expect(describeState({ state: "absent" })).toBe("not present");
    expect(describeState({ state: "list", items: [] })).toBe("automatic");
    expect(describeState({ state: "service", start: "delayed_automatic", running: true })).toBe(
      "delayed automatic, running",
    );
    expect(describeEffect({ effect: "refresh_policy" })).toBe("refresh Windows policy");
  });
});
