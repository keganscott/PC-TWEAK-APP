// A plain-text summary of this PC and what PeakTweaks changed, for the user
// to paste into a message when asking for help. It is only ever put on the
// clipboard by the user's click; nothing is sent anywhere. No account names,
// SIDs, paths or serial numbers go in it.

import type { AppliedChange } from "../generated/AppliedChange";
import type { Comparison } from "../generated/Comparison";
import type { HardwareReport } from "../generated/HardwareReport";
import type { Probe } from "../generated/Probe";
import type { TweakView } from "../generated/TweakView";
import { formatGiB, formatNumber } from "./format";

export function summaryText(
  hardware: HardwareReport | null,
  applied: readonly AppliedChange[],
  tweaks: readonly TweakView[],
  at: Date,
  sample = false,
  build: string = __APP_BUILD__,
): string {
  const read = <T>(p: Probe<T> | undefined, show: (v: T) => string) =>
    !p ? "not read yet" : p.state === "yes" ? show(p.value) : "not read";
  const drifted = tweaks.filter((t) => t.state.status === "drifted");
  const list = (names: string[]) => (names.length ? names.map((n) => `- ${n}`) : ["- none"]);
  return [
    ...(sample ? ["SAMPLE DATA: made up for testing, not a real PC"] : []),
    `PeakTweaks summary, ${at.toISOString().slice(0, 10)}`,
    `PeakTweaks ${build}`,
    "",
    "This PC",
    `Windows: ${read(hardware?.os, (os) => `${os.caption} (build ${os.build})`)}`,
    `Processor: ${read(hardware?.cpu, (c) => `${c.name.trim()}, ${c.cores} cores, ${c.logicalProcessors} threads`)}`,
    `Graphics: ${read(hardware?.gpus, (gs) => (gs.length ? gs.map((g) => `${g.name} (${formatGiB(g.dedicatedVramBytes)})`).join("; ") : "none found"))}`,
    `Memory: ${read(hardware?.memory, (m) => formatGiB(m.installedBytes))}`,
    `Display: ${read(hardware?.display, (d) => `${d.width} x ${d.height} at ${d.currentHz} Hz`)}`,
    "",
    `Changes in place (${applied.length})`,
    ...list(applied.map((c) => c.name)),
    ...(drifted.length
      ? ["", `Set back since PeakTweaks changed them (${drifted.length})`, ...list(drifted.map((t) => t.name))]
      : []),
  ].join("\n");
}

/**
 * One comparison from the Proof tab as plain text, for the user to share: the
 * engine's headline word for word, the medians behind it and the ids of the
 * stored runs, so every number in it can be traced to a recording. Put on the
 * clipboard only by the user's click.
 */
export function comparisonText(
  program: string,
  comparison: Comparison,
  at: Date,
  sample = false,
  build: string = __APP_BUILD__,
): string {
  const figure = (m: Comparison["average"]) =>
    // copy-lint-allow: labels for the medians of the stored runs listed below
    `${m.metric === "avg_fps" ? "Average FPS" : "1% low FPS"}: before ${formatNumber(m.beforeMedian)}, after ${formatNumber(m.afterMedian)} (a difference counts above ${formatNumber(m.threshold)})`;
  return [
    ...(sample ? ["SAMPLE DATA: made up for testing, not a real PC"] : []),
    `PeakTweaks comparison, ${program}, ${at.toISOString().slice(0, 10)}`,
    "",
    comparison.headline,
    "",
    figure(comparison.average),
    figure(comparison.lows),
    ...(comparison.warnings.length ? ["", "Trust these numbers less:", ...comparison.warnings.map((w) => `- ${w}`)] : []),
    "",
    `Runs before (${comparison.beforeRunIds.length}): ${comparison.beforeRunIds.join(", ")}`,
    `Runs after (${comparison.afterRunIds.length}): ${comparison.afterRunIds.join(", ")}`,
    `PeakTweaks ${build}`,
  ].join("\n");
}
