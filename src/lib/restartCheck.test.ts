import { describe, expect, it } from "vitest";

import type { ContextInfo } from "../generated/ContextInfo";
import type { JournalView } from "../generated/JournalView";
import type { Settings } from "../generated/Settings";
import type { TweakView } from "../generated/TweakView";
import * as fx from "../generated/fixtures";
import { restartCheck } from "./restartCheck";

const BOOT = 1_700_000_100_000;
const context = { ...(fx.contextInfo as ContextInfo), bootedUnixMs: BOOT };
const settings = { ...(fx.systemAudit.settings as Settings), restartCheckSeenBoot: null };
const views = fx.tweakViews as TweakView[];
const tweak = (id: string, requiresReboot: boolean) => ({ ...views.find((t) => t.id === id)!, requiresReboot });

function journal(applied: [string, number][]): JournalView {
  const base = fx.journalView as JournalView;
  return {
    ...base,
    applied: applied.map(([tweakId]) => ({ tweakId, name: tweakId, kind: "catalogue" as const })),
    records: applied.map(([tweakId, unixMs], i) => ({
      ...(base.records.find((r) => r.record === "write") as Extract<JournalView["records"][number], { record: "write" }>),
      seq: i + 1,
      tweakId,
      unixMs,
      action: "apply" as const,
    })),
  };
}

describe("the check after a restart", () => {
  it("shows once Windows has started since a change that waits for a restart was applied", () => {
    const tweaks = [tweak("fixture.applied", true), tweak("fixture.foreign", false)];
    const before = journal([["fixture.applied", BOOT - 60_000], ["fixture.foreign", BOOT - 60_000]]);
    expect(restartCheck(context, settings, tweaks, before)).toEqual({
      booted: BOOT,
      waited: [{ id: "fixture.applied", name: tweaks[0]!.name }],
      setBack: [],
      unreadable: [],
      unchecked: [],
    });
    // Applied after this start: it still waits for the next restart.
    expect(restartCheck(context, settings, tweaks, journal([["fixture.applied", BOOT + 1000]]))).toBeNull();
    // Nothing applied waits for a restart.
    expect(restartCheck(context, settings, [tweak("fixture.applied", false)], before)).toBeNull();
    // Off Windows the engine has no boot time.
    expect(restartCheck({ ...context, bootedUnixMs: null }, settings, tweaks, before)).toBeNull();
  });

  it("names applied changes that are set back or unreadable", () => {
    const tweaks = [tweak("fixture.applied", true), tweak("fixture.drifted", false), tweak("fixture.unknown", false)];
    const all = journal([
      ["fixture.applied", BOOT - 1],
      ["fixture.drifted", BOOT - 1],
      ["fixture.unknown", BOOT - 1],
    ]);
    const check = restartCheck(context, settings, tweaks, all)!;
    expect(check.setBack.map((c) => c.id)).toEqual(["fixture.drifted"]);
    expect(check.unreadable.map((c) => c.id)).toEqual(["fixture.unknown"]);
  });

  it("is closed for this start only, whatever the boot time's small drift", () => {
    const tweaks = [tweak("fixture.applied", true)];
    const before = journal([["fixture.applied", BOOT - 60_000]]);
    expect(restartCheck(context, { ...settings, restartCheckSeenBoot: BOOT - 40 }, tweaks, before)).toBeNull();
    const next = { ...context, bootedUnixMs: BOOT + 3_600_000 };
    const seen = { ...settings, restartCheckSeenBoot: BOOT };
    // The change was shown after the start it waited for; the next start
    // has nothing new to show.
    expect(restartCheck(next, seen, tweaks, before)).toBeNull();
    // Applied after that start, it waited for this one.
    expect(restartCheck(next, seen, tweaks, journal([["fixture.applied", BOOT + 1000]]))).not.toBeNull();
  });

  it("names applied changes it could not check, such as a device not read yet", () => {
    const tweaks = [tweak("fixture.applied", true)];
    const both = journal([["fixture.applied", BOOT - 1], ["msi.dev", BOOT - 1]]);
    expect(restartCheck(context, settings, tweaks, both)!.unchecked).toEqual(["msi.dev"]);
    // Once the devices are read, a device change is checked like the rest.
    const device = { ...tweak("fixture.applied", true), id: "msi.dev", name: "MSI mode: Sample" };
    const check = restartCheck(context, settings, [...tweaks, device], both)!;
    expect(check.unchecked).toEqual([]);
    expect(check.waited.map((c) => c.id)).toEqual(["fixture.applied", "msi.dev"]);
  });
});
