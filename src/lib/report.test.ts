import { describe, expect, it } from "vitest";
import { summaryText } from "./report";
import type { HardwareReport } from "../generated/HardwareReport";
import type { TweakView } from "../generated/TweakView";

const no = { state: "unknown", reason: "x" } as const;
const hardware = {
  os: {
    state: "yes",
    value: {
      caption: "Microsoft Windows 11 Pro",
      build: 26100,
      isServer: false,
    },
  },
  cpu: {
    state: "yes",
    value: {
      name: "AMD Ryzen 7 7800X3D ",
      vendor: "AMD",
      cores: 8,
      logicalProcessors: 16,
    },
  },
  memory: {
    state: "yes",
    value: { installedBytes: 32 * 1024 ** 3, sticks: [], channels: no },
  },
  gpus: {
    state: "yes",
    value: [
      {
        name: "NVIDIA GeForce RTX 4070",
        vendorId: 0x10de,
        dedicatedVramBytes: 12 * 1024 ** 3,
        sharedMemoryBytes: 0,
        isSoftware: false,
      },
    ],
  },
  gpuDrivers: no,
  bootDisk: no,
  display: no,
  isLaptop: no,
  rigClass: no,
} as unknown as HardwareReport;

describe("summaryText", () => {
  it("lists this PC, the changes in place and the ones set back", () => {
    const tweaks = [
      { id: "a", name: "Game Mode", state: { status: "drifted" } },
      { id: "b", name: "Other", state: { status: "applied" } },
    ] as unknown as TweakView[];
    const text = summaryText(
      hardware,
      [{ tweakId: "b", name: "Other", kind: "registry" } as never],
      tweaks,
      new Date("2026-10-09T12:00:00Z"),
      false,
      "0.1.0, build 1a2b3c4",
    );
    expect(text).toBe(
      [
        "PeakTweaks summary, 2026-10-09",
        "PeakTweaks 0.1.0, build 1a2b3c4",
        "",
        "This PC",
        "Windows: Microsoft Windows 11 Pro (build 26100)",
        "Processor: AMD Ryzen 7 7800X3D, 8 cores, 16 threads",
        "Graphics: NVIDIA GeForce RTX 4070 (12 GB)",
        "Memory: 32 GB",
        "Display: not read",
        "",
        "Changes in place (1)",
        "- Other",
        "",
        "Set back since PeakTweaks changed them (1)",
        "- Game Mode",
      ].join("\n"),
    );
  });

  it("says what was not read instead of guessing", () => {
    const text = summaryText(null, [], [], new Date(0));
    expect(text).toContain("Windows: not read yet");
    expect(text).toContain("Changes in place (0)\n- none");
    expect(text).not.toContain("Set back");
  });
});
