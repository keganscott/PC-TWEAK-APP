// What the graphics card did during the last game (plan 6.2 item 6: GPU
// throttling, advice only). The engine counts, in readings taken every few
// seconds while the game ran, the reasons NVIDIA's driver gave for holding the
// card's clocks down (`play.rs` `PlayReport`). This turns them into plain
// notes. The causes named are the ones NVIDIA's `nvml.h` gives for each reason
// (NVIDIA/go-nvml gen/nvml/nvml.h, checked 2026-10-10).

import type { PlayReport } from "../generated/PlayReport";
import type { PlayStatus } from "../generated/PlayStatus";
import type { Tone } from "../components/ui/primitives";
import { formatGiB } from "./format";

export interface ReportNote {
  tone: Tone;
  title: string;
  text: string;
}

/** "48 of 800 readings". */
function of(n: number, total: number): string {
  return `${n.toLocaleString()} of ${total.toLocaleString()} reading${total === 1 ? "" : "s"}`;
}

/** Memory in use at the fullest reading, as a whole percentage. */
export function memoryPeakPercent(report: PlayReport): number | null {
  const m = report.memoryPeak;
  if (m.state !== "yes" || m.value.totalBytes === 0) return null;
  return Math.round(((m.value.totalBytes - m.value.availableBytes) / m.value.totalBytes) * 100);
}

/** At or above this share in use, memory counts as nearly full. Mine
 * (NOTES N112): Windows sets no such line. */
export const MEMORY_NEARLY_FULL = 90;

/** "Graphics card busy 97% of the time on average, processor 41% and memory
 * at most 91% in use (15 GB of 16 GB)." Only the readings that were taken; null
 * when none was. */
export function loadLine(report: PlayReport): string | null {
  const parts: string[] = [];
  // copy-lint-allow: a reading taken while the game ran, not a promise
  if (report.gpuBusyAverage.state === "yes") parts.push(`graphics card busy ${report.gpuBusyAverage.value}% of the time on average`);
  if (report.cpuBusyAverage.state === "yes") {
    // copy-lint-allow: a reading taken while the game ran, not a promise
    parts.push(`${parts.length ? "processor" : "processor busy"} ${report.cpuBusyAverage.value}%${parts.length ? "" : " on average"}`);
  }
  const peak = memoryPeakPercent(report);
  if (peak !== null && report.memoryPeak.state === "yes") {
    const m = report.memoryPeak.value;
    // copy-lint-allow: a reading taken while the game ran, not a promise
    parts.push(`memory at most ${peak}% in use (${formatGiB(m.totalBytes - m.availableBytes)} of ${formatGiB(m.totalBytes)})`);
  }
  if (parts.length === 0) return null;
  const line = parts.length === 1 ? parts[0]! : `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`;
  return `${line.charAt(0).toUpperCase()}${line.slice(1)}.`;
}

/** The note for memory, when it was nearly full. */
function memoryNote(report: PlayReport): ReportNote[] {
  const peak = memoryPeakPercent(report);
  if (peak === null || peak < MEMORY_NEARLY_FULL) return [];
  return [
    {
      tone: "warn",
      // copy-lint-allow: a reading taken while the game ran, not a promise
      title: `Memory was nearly full: up to ${peak}% in use.`,
      text:
        "When memory runs short, Windows keeps part of what programs hold on the drive instead, which takes much " +
        "longer to reach. Closing programs you do not need before playing leaves more for the game.",
    },
  ];
}

/** The notes for one report, most important first. Nothing is rounded up to
 * a problem: a reason seen in no reading gives no note. */
export function reportNotes(report: PlayReport): ReportNote[] {
  return [...gpuNotes(report), ...memoryNote(report)];
}

function gpuNotes(report: PlayReport): ReportNote[] {
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
  const peak = memoryPeakPercent(report);
  // copy-lint-allow: a reading taken while the game ran, not a promise
  const memory = peak !== null && peak >= MEMORY_NEARLY_FULL ? ` Memory up to ${peak}% in use.` : "";
  return gpuSummary(report) + memory;
}

function gpuSummary(report: PlayReport): string {
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

/** Every report the watcher has, oldest first: the kept history, plus this
 * run's last game when it could not be saved into it. */
export function playedReports(play: PlayStatus | null | undefined): PlayReport[] {
  const history = play?.history ?? [];
  const last = play?.lastSession ?? null;
  return last && !history.some((r) => sameReport(r, last)) ? [...history, last] : history;
}
