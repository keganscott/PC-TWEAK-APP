// The gentle reminders on Home (game plan new idea 9): a junk cleanup once a
// month, and a graphics driver that has not been updated in six months. Worked
// out from what the engine already reported (the change record and the
// hardware scan); nothing here changes anything. Each one can be put off for a
// month ("Not now"), and all of them turned off in Settings.

import type { JournalView } from "../generated/JournalView";
import type { Settings } from "../generated/Settings";
import type { SystemAudit } from "../generated/SystemAudit";

export const DAY_MS = 24 * 60 * 60 * 1000;
/** How long after the last junk cleanup the next one is suggested. */
export const CLEANUP_EVERY_DAYS = 30;
/** How old a graphics driver's date is before it is mentioned. */
export const DRIVER_OLD_DAYS = 180;
/** How long "Not now" puts a reminder off. */
export const SNOOZE_DAYS = 30;

/** The graphics card makers whose drivers are mentioned, by PCI vendor id. */
const MAKERS: Readonly<Record<number, string>> = {
  0x10de: "NVIDIA",
  0x1002: "AMD",
  0x8086: "Intel",
};

export type Reminder =
  | { kind: "cleanup"; lastUnixMs: number | null }
  | { kind: "driver"; card: string; maker: string; date: string; days: number };

/** Unix ms for midnight UTC on a `YYYY-MM-DD` date, or null for anything else. */
export function dateMs(date: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!m) return null;
  const ms = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return Number.isNaN(ms) ? null : ms;
}

/** When the junk files were last cleared, from the change record. */
export function lastCleanup(journal: JournalView): number | null {
  let last: number | null = null;
  for (const r of journal.records) {
    if (r.record === "action" && r.action === "cleanup" && r.done !== null && (last === null || r.unixMs > last)) {
      last = r.unixMs;
    }
  }
  return last;
}

/**
 * The reminders due now. `audit` and `journal` are null until the engine has
 * answered; a reminder that depends on one waits for it rather than guessing.
 */
export function dueReminders(
  settings: Settings | null,
  audit: SystemAudit | null,
  journal: JournalView | null,
  nowMs: number,
): Reminder[] {
  if (!settings || settings.remindersOff) return [];
  const snoozed = (until: number | null) => until !== null && nowMs < until;
  const out: Reminder[] = [];

  if (journal && !snoozed(settings.cleanupReminderSnoozedUntil)) {
    const last = lastCleanup(journal);
    if (last === null || nowMs - last >= CLEANUP_EVERY_DAYS * DAY_MS) out.push({ kind: "cleanup", lastUnixMs: last });
  }

  const drivers = audit?.env.hardware?.gpuDrivers;
  if (drivers?.state === "yes" && !snoozed(settings.driverReminderSnoozedUntil)) {
    // A card NVIDIA no longer makes game drivers for has its own finding.
    const legacy = audit?.scan.findings.some((f) => f.id === "gpu.driver_branch" && f.status === "attention") ?? false;
    for (const d of drivers.value) {
      const maker = d.vendorId === null ? undefined : MAKERS[d.vendorId];
      const at = d.driverDate === null ? null : dateMs(d.driverDate);
      if (!maker || at === null || (legacy && maker === "NVIDIA")) continue;
      const days = Math.floor((nowMs - at) / DAY_MS);
      if (days >= DRIVER_OLD_DAYS) out.push({ kind: "driver", card: d.name, maker, date: d.driverDate!, days });
    }
  }
  return out;
}

/** The settings after "Not now" on one kind of reminder. */
export function snooze(settings: Settings, kind: Reminder["kind"], nowMs: number): Settings {
  const until = nowMs + SNOOZE_DAYS * DAY_MS;
  return kind === "cleanup" ? { ...settings, cleanupReminderSnoozedUntil: until } : { ...settings, driverReminderSnoozedUntil: until };
}
