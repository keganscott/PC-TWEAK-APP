import { describe, expect, it } from "vitest";

import type { PlayReport } from "../generated/PlayReport";
import type { ThrottleSeen } from "../generated/ThrottleSeen";
import { playStatus } from "../generated/fixtures";
import { cleanLine, loadLine, reportNotes, reportSummary, sameReport } from "./playReport";

const report = (samples: number, seen: ThrottleSeen[], counts: { heat?: number; hardware?: number } = {}): PlayReport => ({
  game: "fortnite",
  startedUnixMs: 0,
  endedUnixMs: 60_000,
  gpuThrottle: { state: "yes", value: { samples, seen } },
  heatReadings: counts.heat ?? 0,
  hardwareReadings: counts.hardware ?? 0,
  gpuHottestC: { state: "yes", value: 70 },
  temperatureMissed: null,
  gpuBusyAverage: { state: "unknown", reason: "not read" },
  cpuBusyAverage: { state: "unknown", reason: "not read" },
  memoryPeak: { state: "unknown", reason: "not read" },
  memoryCleans: 0,
  memoryCleanedBytes: 0,
});

describe("reportNotes", () => {
  it("names heat first, then the power limit, with the counts, then nearly full memory", () => {
    const notes = reportNotes(playStatus.lastSession!);
    expect(notes.map((n) => n.tone)).toEqual(["warn", "info", "warn"]);
    expect(notes[2]!.title).toBe("Memory was nearly full: up to 91% in use.");
    expect(notes[0]!.title).toBe("The graphics card held its clocks down because of heat in 48 of 800 readings.");
    expect(notes[1]!.title).toBe("The driver kept the card within its power limit in 760 of 800 readings.");
  });

  it("takes the heat count per reading from the engine, not from the reasons", () => {
    // Software heat in 3 readings and hardware heat in 2 others: 5 readings.
    const notes = reportNotes(
      report(
        10,
        [
          { reason: "software_thermal_slowdown", samples: 3 },
          { reason: "hardware_thermal_slowdown", samples: 2 },
        ],
        { heat: 5 },
      ),
    );
    expect(notes).toHaveLength(1);
    expect(notes[0]!.title).toContain("in 5 of 10 readings");
  });

  it("says nothing was seen when only harmless reasons were", () => {
    const notes = reportNotes(report(40, [{ reason: "gpu_idle", samples: 40 }]));
    expect(notes).toEqual([expect.objectContaining({ tone: "ok", title: "No slowdown for heat or power in any of 40 readings." })]);
  });

  it("flags the card's own hardware slowdown", () => {
    const notes = reportNotes(report(100, [{ reason: "hardware_power_brake", samples: 5 }], { hardware: 5 }));
    expect(notes[0]).toEqual(expect.objectContaining({ tone: "warn" }));
    expect(notes[0]!.title).toContain("hardware slowed itself down in 5 of 100 readings");
  });

  it("keeps an unread card unknown, with the reason, and says when there is no NVIDIA card", () => {
    const unknown = reportNotes({ ...report(1, []), gpuThrottle: { state: "unknown", reason: "nvml.dll was not found" } });
    expect(unknown).toEqual([expect.objectContaining({ tone: "neutral", text: "nvml.dll was not found" })]);
    const none = reportNotes({ ...report(1, []), gpuThrottle: { state: "no", reason: "NVML reports no NVIDIA GPU" } });
    expect(none[0]!.title).toBe("No NVIDIA graphics card, so there are no graphics card readings.");
  });
});

describe("reportSummary", () => {
  it("leads with heat, then the card's hardware, and otherwise says none was seen", () => {
    expect(reportSummary(playStatus.lastSession!)).toBe("Slowed for heat in 48 of 800 readings. Memory up to 91% in use.");
    expect(reportSummary(report(100, [{ reason: "hardware_slowdown", samples: 2 }], { hardware: 2 }))).toBe(
      "Hardware slowdown in 2 of 100 readings.",
    );
    expect(reportSummary(report(600, [{ reason: "software_power_cap", samples: 50 }]))).toBe(
      "No slowdown for heat or the card's hardware in any of 600 readings.",
    );
    expect(reportSummary({ ...report(1, []), gpuThrottle: { state: "unknown", reason: "x" } })).toBe(
      "The graphics card could not be read.",
    );
  });

  it("finds the last session in the history", () => {
    expect(sameReport(playStatus.history.at(-1)!, playStatus.lastSession!)).toBe(true);
    expect(sameReport(playStatus.history[0]!, playStatus.lastSession!)).toBe(false);
  });
});

describe("loadLine", () => {
  it("says how busy the card and processor were and how full memory got", () => {
    expect(loadLine(playStatus.lastSession!)).toBe(
      "Graphics card busy 97% of the time on average, processor 41% and memory at most 91% in use (15 GB of 16 GB).",
    );
  });

  it("leaves out what was not read, and is null when nothing was", () => {
    expect(loadLine({ ...report(1, []), cpuBusyAverage: { state: "yes", value: 30 } })).toBe("Processor busy 30% on average.");
    expect(loadLine(report(1, []))).toBeNull();
    expect(loadLine(playStatus.history[0]!)).toBeNull();
  });

  it("gives no memory note below nearly full", () => {
    const roomy = { ...report(10, []), memoryPeak: { state: "yes" as const, value: { totalBytes: 100, availableBytes: 11, cachedBytes: 0 } } };
    expect(reportNotes(roomy).some((n) => n.title.startsWith("Memory"))).toBe(false);
    const full = { ...roomy, memoryPeak: { state: "yes" as const, value: { totalBytes: 100, availableBytes: 10, cachedBytes: 0 } } };
    expect(reportNotes(full).at(-1)!.title).toBe("Memory was nearly full: up to 90% in use.");
  });

  it("says what Clean memory during games did, and nothing when it did not clean", () => {
    expect(cleanLine(report(10, []))).toBeNull();
    expect(cleanLine({ ...report(10, []), memoryCleans: 1, memoryCleanedBytes: 3 * 1024 ** 3 })).toBe(
      "Clean memory during games emptied the standby list once, letting go of 3.0 GB of files kept in memory.",
    );
    expect(cleanLine({ ...report(10, []), memoryCleans: 2, memoryCleanedBytes: 6 * 1024 ** 3 })).toMatch(/list 2 times, letting go of 6\.0 GB/);
  });
});
