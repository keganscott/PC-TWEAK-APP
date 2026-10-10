import { describe, expect, it } from "vitest";

import type { PlayReport } from "../generated/PlayReport";
import type { ThrottleSeen } from "../generated/ThrottleSeen";
import { playStatus } from "../generated/fixtures";
import { reportNotes } from "./playReport";

const report = (samples: number, seen: ThrottleSeen[]): PlayReport => ({
  game: "fortnite",
  startedUnixMs: 0,
  endedUnixMs: 60_000,
  gpuThrottle: { state: "yes", value: { samples, seen } },
  gpuHottestC: { state: "yes", value: 70 },
});

describe("reportNotes", () => {
  it("names heat first, then the power limit, with the counts", () => {
    const notes = reportNotes(playStatus.lastSession!);
    expect(notes.map((n) => n.tone)).toEqual(["warn", "info"]);
    expect(notes[0]!.title).toBe("The graphics card held its clocks down because of heat in 48 of 800 readings.");
    expect(notes[1]!.title).toBe("The driver kept the card within its power limit in 760 of 800 readings.");
  });

  it("counts a reading with both heat reasons once", () => {
    const notes = reportNotes(
      report(10, [
        { reason: "software_thermal_slowdown", samples: 3 },
        { reason: "hardware_thermal_slowdown", samples: 2 },
      ]),
    );
    expect(notes).toHaveLength(1);
    expect(notes[0]!.title).toContain("in 3 of 10 readings");
  });

  it("says nothing was seen when only harmless reasons were", () => {
    const notes = reportNotes(report(40, [{ reason: "gpu_idle", samples: 40 }]));
    expect(notes).toEqual([expect.objectContaining({ tone: "ok", title: "No slowdown for heat or power in any of 40 readings." })]);
  });

  it("flags the card's own hardware slowdown", () => {
    const notes = reportNotes(report(100, [{ reason: "hardware_power_brake", samples: 5 }]));
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
