import { describe, expect, it } from "vitest";

import type { JournalView } from "../generated/JournalView";
import type { Settings } from "../generated/Settings";
import type { SystemAudit } from "../generated/SystemAudit";
import * as fx from "../generated/fixtures";
import { DAY_MS, dateMs, dueReminders, lastCleanup, snooze } from "./reminders";

const settings: Settings = { ...(fx.systemAudit.settings as Settings), remindersOff: false };
const journal = fx.journalView as JournalView;
const audit = fx.systemAudit as SystemAudit;
// The sample cleanup ran at this time; its driver is dated 2025-08-20.
const CLEANED = 1_700_000_002_000;
const DRIVER = dateMs("2025-08-20")!;

function withDriver(vendorId: number | null, driverDate: string | null): SystemAudit {
  const hw = audit.env.hardware!;
  const gpuDrivers = { state: "yes" as const, value: [{ name: "Card", vendorId, driverVersion: null, nvidiaVersion: null, driverDate }] };
  return { ...audit, env: { ...audit.env, hardware: { ...hw, gpuDrivers } } };
}

describe("gentle reminders", () => {
  it("suggest a junk cleanup a month after the last one, and before the first", () => {
    expect(lastCleanup(journal)).toBe(CLEANED);
    const kinds = (now: number) => dueReminders(settings, null, journal, now).map((r) => r.kind);
    expect(kinds(CLEANED + 29 * DAY_MS)).toEqual([]);
    expect(kinds(CLEANED + 30 * DAY_MS)).toEqual(["cleanup"]);
    const none: JournalView = { ...journal, records: journal.records.filter((r) => r.record !== "action") };
    expect(dueReminders(settings, null, none, CLEANED)).toEqual([{ kind: "cleanup", lastUnixMs: null }]);
    // A cleanup that failed does not count.
    const failed: JournalView = {
      ...none,
      records: [...none.records, { record: "action", seq: 99, unixMs: CLEANED, action: "cleanup", done: null, error: "no" }],
    };
    expect(lastCleanup(failed)).toBeNull();
    // Nothing until the change record has been read.
    expect(dueReminders(settings, null, null, CLEANED)).toEqual([]);
  });

  it("mention a graphics driver dated more than six months ago, once the scan has read it", () => {
    const driver = (a: SystemAudit, now: number) => dueReminders(settings, a, null, now);
    expect(driver(withDriver(0x1002, "2025-08-20"), DRIVER + 179 * DAY_MS)).toEqual([]);
    expect(driver(withDriver(0x1002, "2025-08-20"), DRIVER + 180 * DAY_MS)).toEqual([
      { kind: "driver", card: "Card", maker: "AMD", date: "2025-08-20", days: 180 },
    ]);
    // An unknown maker, an unreadable date or no answer from the scan: nothing.
    expect(driver(withDriver(0x1234, "2025-08-20"), DRIVER + 400 * DAY_MS)).toEqual([]);
    expect(driver(withDriver(0x10de, null), DRIVER + 400 * DAY_MS)).toEqual([]);
    expect(driver(withDriver(0x10de, "20/08/2025"), DRIVER + 400 * DAY_MS)).toEqual([]);
    const unknown = { ...audit, env: { ...audit.env, hardware: { ...audit.env.hardware!, gpuDrivers: { state: "unknown" as const, reason: "WMI" } } } };
    expect(driver(unknown, DRIVER + 400 * DAY_MS)).toEqual([]);
    // An NVIDIA card that gets no new game drivers has its own finding instead.
    const legacy = withDriver(0x10de, "2025-08-20");
    const findings = legacy.scan.findings.map((f) => (f.id === "gpu.driver_branch" ? { ...f, status: "attention" as const } : f));
    expect(driver({ ...legacy, scan: { ...legacy.scan, findings } }, DRIVER + 400 * DAY_MS)).toEqual([]);
  });

  it("are put off for a month by Not now, and none show when turned off", () => {
    const now = DRIVER + 400 * DAY_MS;
    expect(dueReminders(settings, audit, journal, now).map((r) => r.kind)).toEqual(["cleanup", "driver"]);
    const later = snooze(snooze(settings, "cleanup", now), "driver", now);
    expect(dueReminders(later, audit, journal, now + 29 * DAY_MS)).toEqual([]);
    expect(dueReminders(later, audit, journal, now + 30 * DAY_MS).map((r) => r.kind)).toEqual(["cleanup", "driver"]);
    expect(dueReminders({ ...settings, remindersOff: true }, audit, journal, now)).toEqual([]);
    expect(dueReminders(null, audit, journal, now)).toEqual([]);
  });
});
