// Copy this PC's setup to another PC (game plan new idea 7). The setup is the
// list of catalogue changes PeakTweaks applied here, as a short text to paste
// into PeakTweaks on the other PC: no file is written and nothing is sent
// anywhere. On the other PC each id is matched against its own list; only
// changes found there are offered, each is applied the usual way (the engine
// checks the id, the restore point and the rest), and changes that come with
// something to read first are left for the user to apply in Tools.

import type { BlockedCode } from "../generated/BlockedCode";
import type { TweakView } from "../generated/TweakView";
import { APPEARANCE } from "../store/store";

const KIND = "peaktweaks-setup";
const VERSION = 1;
/** Longer than any real setup; a paste past it is not one. */
const MAX_CHARS = 20_000;
/** Reasons a change can never be made on a PC as it is (as in Tools). */
const NOT_HERE: readonly BlockedCode[] = ["hardware_unsupported", "os_version_unsupported"];

/** The text to copy: the changes PeakTweaks applied on this PC. */
export function setupCode(tweaks: readonly TweakView[]): string {
  const changes = tweaks.filter((t) => t.state.status === "applied").map((t) => t.id);
  return JSON.stringify({ [KIND]: VERSION, changes });
}

/** The change ids in a pasted setup, or why it is not one. */
export function readSetupCode(text: string): { ids: string[] } | { problem: string } {
  const notOne = { problem: "That is not a setup copied from PeakTweaks." };
  if (text.length > MAX_CHARS) return notOne;
  let parsed: unknown;
  try {
    parsed = JSON.parse(text.trim());
  } catch {
    return notOne;
  }
  if (typeof parsed !== "object" || parsed === null) return notOne;
  const o = parsed as Record<string, unknown>;
  if (o[KIND] !== VERSION) {
    return typeof o[KIND] === "number" ? { problem: "That setup is from a newer PeakTweaks. Update this one first." } : notOne;
  }
  const changes = o.changes;
  if (!Array.isArray(changes) || !changes.every((c) => typeof c === "string" && c.length > 0 && c.length <= 200)) return notOne;
  return { ids: [...new Set(changes as string[])] };
}

export interface SetupPlan {
  /** Can be applied together: no warning to read, not blocked, not in place. */
  apply: TweakView[];
  /** Each has a cost line or a warning, changes how Windows looks (one
   * click each, never part of a set), is held back for a reason Tools
   * explains, or its state could not be read, so it is applied from Tools. */
  yourself: TweakView[];
  /** In place on this PC already. */
  already: TweakView[];
  /** Not on this PC's list, or not possible on its hardware or Windows. */
  notHere: string[];
}

/** What a pasted setup would do on this PC. */
export function planSetup(ids: readonly string[], tweaks: readonly TweakView[]): SetupPlan {
  const plan: SetupPlan = { apply: [], yourself: [], already: [], notHere: [] };
  for (const id of ids) {
    const t = tweaks.find((x) => x.id === id);
    // A missing restore point holds every change back; the Apply button
    // waits for one instead.
    const reason = (t?.state.status === "blocked" ? t.state.reason : t?.blocked)?.code;
    const held = reason !== undefined && reason !== "no_restore_point";
    if (!t || (reason && NOT_HERE.includes(reason))) plan.notHere.push(t?.name ?? id);
    else if (t.state.status === "applied" || t.state.status === "foreign") plan.already.push(t);
    else if (held || t.safety !== "safe" || t.tradeoff || t.category === APPEARANCE || t.state.status === "unknown") {
      plan.yourself.push(t);
    }
    else plan.apply.push(t);
  }
  return plan;
}
