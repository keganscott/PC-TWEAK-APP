import { describe, expect, it } from "vitest";

import type { PlayReport } from "../generated/PlayReport";
import type { ThrottleSeen } from "../generated/ThrottleSeen";
import { playStatus } from "../generated/fixtures";
import { reportNotes, reportSummary, sameReport } from "./playReport";

const report = (samples: number, seen: ThrottleSeen[], counts: { heat?: number; hardware?: number } = {}): PlayReport => ({
  game: "fortnite",
  startedUnixMs: 0,
  endedUnixMs: 60_000,
  gpuThrottle: { state: "yes", value: { samples, seen } },
  heatReadings: counts.heat ?? 0,
  hardwareReadings: counts.hardware ?? 0,
  gpuHottestC: { state: "yes", value: 70 },
  temperatureMissed: null,
});

describe("reportNotes", () => {
  it("names heat first, then the power limit, with the counts", () => {
    const notes = reportNotes(playStatus.lastSession!);
    expect(notes.map((n) => n.tone)).toEqual(["warn", "info"]);
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
    expect(reportSummary(playStatus.lastSession!)).toBe("Slowed for heat in 48 of 800 readings.");
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
