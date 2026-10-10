// The check after a restart on Home (game plan new idea 5). When Windows has
// started since a change that waits for a restart was applied, Home says
// whether everything PeakTweaks applied is still in place, names the changes
// that waited for the restart (the ones to undo first if something is off),
// and is closed once per restart. Worked out from what the engine reported:
// when Windows started (`ContextInfo.bootedUnixMs`), the change record and
// each change's state as read now.

import type { ContextInfo } from "../generated/ContextInfo";
import type { JournalView } from "../generated/JournalView";
import type { Settings } from "../generated/Settings";
import type { TweakView } from "../generated/TweakView";

/** The boot time is the clock less Windows' tick count, so it moves a little
 * between reads; two within this are the same start. */
export const SAME_BOOT_MS = 60_000;

export interface RestartCheck {
  booted: number;
  /** Applied before this start and take effect only after a restart. */
  waited: { id: string; name: string }[];
  /** Applied changes that do not read as PeakTweaks left them. */
  setBack: { id: string; name: string }[];
  unreadable: { id: string; name: string }[];
}

/** When each change was last applied, from the change record. */
function appliedAt(journal: JournalView): Map<string, number> {
  const at = new Map<string, number>();
  for (const r of journal.records) {
    if ((r.record === "write" || r.record === "change") && r.action === "apply") {
      at.set(r.tweakId, Math.max(at.get(r.tweakId) ?? 0, r.unixMs));
    }
  }
  return at;
}

/** The check to show now, or null. */
export function restartCheck(
  context: ContextInfo | null,
  settings: Settings | null,
  tweaks: readonly TweakView[],
  journal: JournalView | null,
): RestartCheck | null {
  const booted = context?.bootedUnixMs ?? null;
  if (booted === null || !settings || !journal) return null;
  const seen = settings.restartCheckSeenBoot;
  if (seen !== null && Math.abs(seen - booted) < SAME_BOOT_MS) return null;

  const at = appliedAt(journal);
  const applied = journal.applied.flatMap((a) => {
    const t = tweaks.find((x) => x.id === a.tweakId);
    return t ? [t] : [];
  });
  const pick = (t: TweakView) => ({ id: t.id, name: t.name });
  const waited = applied.filter((t) => t.requiresReboot && (at.get(t.id) ?? Infinity) < booted).map(pick);
  if (waited.length === 0) return null;
  return {
    booted,
    waited,
    setBack: applied.filter((t) => t.state.status === "drifted" || t.state.status === "default").map(pick),
    unreadable: applied.filter((t) => t.state.status === "unknown").map(pick),
  };
}
