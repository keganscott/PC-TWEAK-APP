// What the graphics card did during the last game (plan 6.2 item 6: GPU
// throttling, advice only). The engine counts, in readings taken every few
// seconds while the game ran, the reasons NVIDIA's driver gave for holding the
// card's clocks down (`play.rs` `PlayReport`). This turns them into plain
// notes. The causes named are the ones NVIDIA's `nvml.h` gives for each reason
// (NVIDIA/go-nvml gen/nvml/nvml.h, checked 2026-10-10).

import type { PlayReport } from "../generated/PlayReport";
import type { Tone } from "../components/ui/primitives";

export interface ReportNote {
  tone: Tone;
  title: string;
  text: string;
}

/** "48 of 800 readings". */
function of(n: number, total: number): string {
  return `${n.toLocaleString()} of ${total.toLocaleString()} reading${total === 1 ? "" : "s"}`;
}

/** The notes for one report, most important first. Nothing is rounded up to
 * a problem: a reason seen in no reading gives no note. */
export function reportNotes(report: PlayReport): ReportNote[] {
  const t = report.gpuThrottle;
  if (t.state === "no") {
    return [
      {
        tone: "neutral",
        title: "No NVIDIA graphics card, so there are no graphics card readings.",
        text: "PeakTweaks reads NVIDIA cards only for now; AMD and Intel graphics are not read.",
      },
    ];
  }
  if (t.state === "unknown") {
    return [{ tone: "neutral", title: "The graphics card could not be read.", text: t.reason }];
  }
  const { samples, seen } = t.value;
  const notes: ReportNote[] = [];
  // Counted per reading by the engine: one with both heat reasons is one.
  const heat = report.heatReadings;
  if (heat > 0) {
    notes.push({
      tone: "warn",
      title: `The graphics card held its clocks down because of heat in ${of(heat, samples)}.`,
      text:
        "The card or its driver does this to keep it under its maximum operating temperature. Dust in the card's " +
        "fans and heatsink, or too little air moving through the case, are common causes, so cleaning them and " +
        "checking the case fans is worth doing.",
    });
  }
  const hardware = report.hardwareReadings;
  if (hardware > 0) {
    notes.push({
      tone: "warn",
      title: `The graphics card's hardware slowed itself down in ${of(hardware, samples)}.`,
      text:
        "NVIDIA names three causes: a temperature too high, a brake signal from the power supply, or power draw " +
        "above the card's protection limit. It can also show briefly while the card changes clock speed, so a few " +
        "readings alone are not a sign of a fault. If it keeps happening, check the card's power cables and the " +
        "power supply.",
    });
  }
  const power = seen.find((s) => s.reason === "software_power_cap")?.samples ?? 0;
  if (power > 0) {
    notes.push({
      tone: "info",
      title: `The driver kept the card within its power limit in ${of(power, samples)}.`,
      text: "In a demanding game that is common, and on its own it is not a sign of a fault.",
    });
  }
  if (notes.length === 0) {
    notes.push({
      tone: "ok",
      title: `No slowdown for heat or power in any of ${samples.toLocaleString()} reading${samples === 1 ? "" : "s"}.`,
      text: "The driver never held the card's clocks down for either while the game ran.",
    });
  }
  return notes;
}

/** One short line for a report in a list of earlier games: the most
 * important note's finding, without the advice. */
export function reportSummary(report: PlayReport): string {
  const t = report.gpuThrottle;
  if (t.state === "no") return "No NVIDIA graphics card to read.";
  if (t.state === "unknown") return "The graphics card could not be read.";
  const { samples } = t.value;
  if (report.heatReadings > 0) return `Slowed for heat in ${of(report.heatReadings, samples)}.`;
  if (report.hardwareReadings > 0) return `Hardware slowdown in ${of(report.hardwareReadings, samples)}.`;
  return `No slowdown for heat or the card's hardware in ${samples === 1 ? "the one reading" : `any of ${samples.toLocaleString()} readings`}.`;
}

/** The same game session: the history keeps the report `lastSession` also
 * holds. */
export function sameReport(a: PlayReport, b: PlayReport): boolean {
  return a.game === b.game && a.startedUnixMs === b.startedUnixMs && a.endedUnixMs === b.endedUnixMs;
}
