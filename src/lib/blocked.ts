// What a user can do about each reason the engine refuses a change. Keyed by the
// engine's `BlockedCode` so nothing is inferred from its message text, which is
// for display only. `satisfies` makes a new code a type error until it has a
// line here.

import type { BlockedCode } from "../generated/BlockedCode";
import type { BlockedReason } from "../generated/BlockedReason";

export const BLOCKED_HINT = {
  anti_cheat_requirement:
    "The anti-cheat of the game you picked needs this setting left as it is. The Games tab shows what it requires.",
  anti_cheat_eligibility:
    "Changing this could stop the game you picked from letting you play. The Games tab shows its requirements.",
  hardware_unsupported: "This PC's hardware does not support this change, so there is nothing to do.",
  hardware_counterproductive: "PeakTweaks does not offer this change on hardware like this PC's.",
  os_version_unsupported: "This version of Windows does not support this change.",
  conflicting_tweak: "Another applied change conflicts with this one. Undo that one first from Backups.",
  insufficient_privilege: "This needs administrator rights. Close PeakTweaks, start it again and accept the Windows prompt.",
  no_restore_point: "Make a restore point on Home first. Changes unlock as soon as Windows confirms it.",
  unsafe_for_device: "PeakTweaks does not make this change on this kind of device.",
  tier_required: "This change is not included in your current plan.",
} as const satisfies Record<BlockedCode, string>;

export function blockedHint(reason: BlockedReason): string {
  return BLOCKED_HINT[reason.code];
}
