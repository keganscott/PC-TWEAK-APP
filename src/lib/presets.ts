import type { TweakView } from "../generated/TweakView";
import { APPEARANCE } from "../store/store";

/**
 * Presets (Kegan, 2026-10-10: "There should also be presets"): named sets of
 * Tools changes applied together after one review. Which changes each holds is
 * Claude's pick (NOTES N116); a preset never adds a change the engine does not
 * offer, and the engine still checks each one (restore point, tier, game).
 *
 * DECISIONS 15.22 keeps changes with a cost line out of a one-click set because
 * a one-click set shows no lines. A preset is two clicks: its review lists every
 * change with its cost line, and Advanced ones need a tick that those were read.
 */
export type PresetId = "basics" | "privacy" | "competitive";

export interface Preset {
  id: PresetId;
  name: string;
  /** What it is, in one line, with no promise of a result. */
  blurb: string;
  /** Does this change belong to the preset? */
  holds: (t: TweakView) => boolean;
}

/** Safe, no cost line, not look-and-feel: the same set as Home's basic changes. */
const basic = (t: TweakView) => t.safety === "safe" && t.category !== APPEARANCE && !t.tradeoff;

/** The game-related changes the Competitive preset adds to the basics. Per-game
 * changes (priority, fullscreen, graphics chip) stay with each game, and the
 * Extreme one (csrss) stays a choice of its own. */
const COMPETITIVE = new Set([
  "input.mouseaccel",
  "input.accessibilitykeys",
  "gaming.gamemode",
  "gaming.capture",
  "gaming.windowedgames",
  "display.gpuscheduling",
  "display.mpo",
  "display.refresh_max",
  "system.powerthrottling",
  "scheduling.systemresponsiveness",
  "scheduling.gamestask",
  "scheduling.foreground",
  "network.throttling",
  "network.nagle",
  "network.tcp",
  "network.prefercable",
  "network.qos.games",
  "power.plan",
  "nvidia.lowlatency",
  "nvidia.powermax",
  "nvidia.shadercache",
  "amd.antilag",
]);

export const PRESETS: readonly Preset[] = [
  {
    id: "basics",
    name: "Basics",
    blurb: "Well-supported Windows settings with nothing to weigh up. The same set Home offers.",
    holds: basic,
  },
  {
    id: "competitive",
    name: "Competitive",
    blurb: "The basics plus game, network, power and graphics driver settings chosen for competitive play.",
    holds: (t) => basic(t) || COMPETITIVE.has(t.id),
  },
  {
    id: "privacy",
    name: "Privacy",
    blurb: "Fewer suggestions, less tracking for ads and less data sent to Microsoft.",
    holds: (t) => t.category === "privacy" && t.safety === "safe",
  },
];

export interface PresetPlan {
  /** Changes to make: not in effect yet (or set back since), and offered here. */
  toApply: TweakView[];
  /** Already in effect, by PeakTweaks or set before. */
  inPlace: TweakView[];
  /** The engine does not offer them here now (this PC, its plan, the main game). */
  notHere: TweakView[];
}

/**
 * The per-game tools that make up one game's pro setup (NOTES N134): start
 * priority, fullscreen optimizations, the high-performance graphics chip,
 * and the game's own V-Sync setting where PeakTweaks has one. The shared
 * game-traffic priority (`network.qos.games`) covers every game at once, so
 * it stays in Tools, not in a single game's setup.
 */
export function gameToolIds(gameId: string): string[] {
  return [
    `priority.ifeo.${gameId}`,
    `gaming.fullscreen.${gameId}`,
    `gpu.choice.${gameId}`,
    `game.${gameId}.vsync`,
  ];
}

/** Sort one game's pro setup by what applying it would do to each tool. */
export function planGameBundle(gameId: string, tweaks: readonly TweakView[]): PresetPlan {
  const ids = new Set(gameToolIds(gameId));
  return sortPlan((t) => ids.has(t.id), tweaks);
}

/** Sort a preset's changes by what applying it would do to each. */
export function planPreset(preset: Preset, tweaks: readonly TweakView[]): PresetPlan {
  return sortPlan(preset.holds, tweaks);
}

function sortPlan(holds: (t: TweakView) => boolean, tweaks: readonly TweakView[]): PresetPlan {
  const plan: PresetPlan = { toApply: [], inPlace: [], notHere: [] };
  for (const t of tweaks) {
    if (!holds(t)) continue;
    const s = t.state.status;
    if (s === "applied" || s === "foreign") plan.inPlace.push(t);
    else if (t.blocked || s === "blocked" || s === "unknown") plan.notHere.push(t);
    else plan.toApply.push(t);
  }
  return plan;
}

/** The review asks for a tick when any change to make is Advanced. */
export function needsTick(plan: PresetPlan): boolean {
  return plan.toApply.some((t) => t.safety !== "safe");
}
