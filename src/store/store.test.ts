import { describe, expect, it } from "vitest";

import type { MsiDeviceList } from "../generated/MsiDeviceList";
import type { PlayStatus } from "../generated/PlayStatus";
import type { StartupList } from "../generated/StartupList";
import { createMockBackend, type MockOptions } from "../services/mockIpc";
import { basicTweaks, BUS_LIMIT, createAppStore, driftedTweaks, otherLongWork, recommendedIds, type State } from "./store";

async function booted(options: MockOptions = {}) {
  const backend = createMockBackend(options);
  const store = createAppStore(backend);
  await store.actions.boot();
  await settle(store);
  return { store, backend };
}

/** Wait until no audit refresh is in flight. */
async function settle(store: { getState(): State }) {
  for (let i = 0; i < 50 && store.getState().auditOp.status === "running"; i += 1) {
    await new Promise((r) => setTimeout(r, 5));
  }
}

describe("boot", () => {
  it("loads everything and then the audit", async () => {
    const { store } = await booted();
    const s = store.getState();
    expect(s.boot.status).toBe("ready");
    expect(s.sample).toBe(true);
    expect(s.context?.elevated).toBe(true);
    expect(s.tweaks.length).toBeGreaterThan(0);
    expect(s.audit?.scan.findings.length).toBeGreaterThan(0);
  });

  it("records a failure with the engine's error, and a retry recovers (R20)", async () => {
    const backend = createMockBackend({ failures: { listTweaks: { kind: "storage", path: "C:\\x", detail: "disk full" } } });
    const store = createAppStore(backend);
    await store.actions.boot();
    const failed = store.getState().boot;
    expect(failed.status).toBe("failed");
    expect(failed.status === "failed" && failed.error.kind).toBe("storage");

    await store.actions.boot();
    expect(store.getState().boot.status).toBe("ready");
  });
});

describe("races (R20)", () => {
  it("a slow reply for an older target-game pick never overwrites a newer one", async () => {
    const { store } = await booted({
      latencyFor: (command, args) => (command === "selectTargetGame" ? (args[0] === "fortnite" ? 60 : 5) : undefined),
    });
    const slow = store.actions.selectTargetGame("fortnite");
    const fast = store.actions.selectTargetGame("minecraft");
    await Promise.all([slow, fast]);
    await settle(store);
    expect(store.getState().targetGame).toBe("minecraft");
    expect(store.getState().targetOp.status).toBe("done");
  });

  it("a failed target-game pick goes back to the previous choice and says why", async () => {
    const { store } = await booted();
    await store.actions.selectTargetGame("fortnite");
    await settle(store);
    await store.actions.selectTargetGame("not-a-game");
    const s = store.getState();
    expect(s.targetGame).toBe("fortnite");
    expect(s.targetOp.status === "failed" && s.targetOp.error.kind).toBe("unknown_game");
  });
});

describe("changes", () => {
  it("apply is refused by the engine without a restore point, then works after one", async () => {
    const { store } = await booted();
    await store.actions.applyTweak("fixture.default");
    const refused = store.getState().tweakOps["fixture.default"];
    expect(refused?.status === "failed" && refused.error.kind).toBe("blocked");

    await store.actions.createRestorePoint();
    expect(store.getState().restoreOp.status).toBe("done");
    expect(store.getState().audit?.env.restoreGateOpen).toBe(true);

    await store.actions.applyTweak("fixture.default");
    const s = store.getState();
    expect(s.tweakOps["fixture.default"]?.status).toBe("done");
    expect(s.tweaks.find((t) => t.id === "fixture.default")?.state.status).toBe("applied");
    expect(s.lastChange).toMatchObject({ kind: "apply", tweakIds: ["fixture.default"], failed: [] });
  });

  it("a card keeps its spinner until the new list shows the change, so a second click cannot undo it", async () => {
    // Kegan's "two clicks" (2026-10-09): the engine answered the apply, the
    // spinner stopped, and the card showed its old state with Apply live
    // until a slow list came back.
    const slowList = { listTweaks: 0 };
    const { store, backend } = await booted({
      gateOpen: true,
      latencyFor: (command) => (command === "listTweaks" ? slowList.listTweaks : undefined),
    });
    slowList.listTweaks = 40;
    let applies = 0;
    const applyTweak = backend.applyTweak;
    backend.applyTweak = (id) => ((applies += 1), applyTweak(id));

    const first = store.actions.applyTweak("fixture.default");
    await new Promise((r) => setTimeout(r, 20));
    // The engine has answered; the list has not.
    expect(store.getState().tweaks.find((t) => t.id === "fixture.default")?.state.status).toBe("default");
    expect(store.getState().tweakOps["fixture.default"]?.status).toBe("running");
    await store.actions.applyTweak("fixture.default"); // the impatient second click
    await first;

    expect(applies).toBe(1);
    const s = store.getState();
    expect(s.tweakOps["fixture.default"]?.status).toBe("done");
    expect(s.tweaks.find((t) => t.id === "fixture.default")?.state.status).toBe("applied");
    expect(s.lastChange).toMatchObject({ kind: "apply", tweakIds: ["fixture.default"] });
  });

  it("a blocked tweak stays blocked, with the engine's reason", async () => {
    const { store } = await booted({ gateOpen: true });
    await store.actions.applyTweak("fixture.blocked");
    const op = store.getState().tweakOps["fixture.blocked"];
    expect(op?.status === "failed" && op.error.kind === "blocked" && op.error.reason.message).toBeTruthy();
  });

  it("undo-all failures are reported, never swallowed (R20)", async () => {
    const { store } = await booted({ failures: { revertAll: { kind: "internal", detail: "boom" } } });
    await store.actions.revertAll();
    const op = store.getState().revertAllOp;
    expect(op.status === "failed" && op.error).toEqual({ kind: "internal", detail: "boom" });
  });

  it("undo-all reverts what is applied and reports it on the result card", async () => {
    const { store } = await booted({ gateOpen: true });
    await store.actions.revertAll();
    const s = store.getState();
    expect(s.revertAllOp.status).toBe("done");
    expect(s.tweaks.some((t) => t.state.status === "applied")).toBe(false);
    expect(s.lastChange?.kind).toBe("revert_all");
  });

  it("a failed settings save reports false and keeps the stored settings", async () => {
    const { store } = await booted({ failures: { setSettings: { kind: "storage", path: "p", detail: "d" } } });
    const before = store.getState().settings;
    expect(await store.actions.saveSettings({ language: "technical", rigClassOverride: "high", welcomeSeen: true, gamingMode: false, gameTimer: false })).toBe(false);
    expect(store.getState().settings).toBe(before);
    expect(store.getState().settingsOp.status).toBe("failed");
  });
});

describe("selectors (plan section 7)", () => {
  it("unrelated updates keep every other slice's reference", async () => {
    const { store } = await booted();
    const before = store.getState();
    store.actions.dismissChange(); // no-op: same state object
    expect(store.getState()).toBe(before);

    await store.actions.selectTargetGame("roblox");
    await settle(store);
    const after = store.getState();
    expect(after.journal).toBe(before.journal);
    expect(after.games).toBe(before.games);
    expect(after.proof).toBe(before.proof);
  });
});

describe("activity log", () => {
  it("keeps the newest entries only", async () => {
    const { store } = await booted({ gateOpen: true });
    for (let i = 0; i < BUS_LIMIT + 20; i += 1) {
      await store.actions.createRestorePoint();
    }
    const bus = store.getState().bus;
    expect(bus.length).toBe(BUS_LIMIT);
    expect(bus[bus.length - 1]!.id).toBeGreaterThan(BUS_LIMIT);
  });
});

// ---------------------------------------------------------------------------
// Regressions from the frontend code review (DECISIONS.md 15.12)
// ---------------------------------------------------------------------------

import type { Backend } from "../services/backend";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe("review regressions", () => {
  it("an older tweak list in flight never overwrites the list a target-game pick returned", async () => {
    const base = createMockBackend();
    const backend: Backend = {
      ...base,
      rescan: async () => {
        await sleep(60);
        return (await base.listTweaks()).map((t) => ({ ...t, name: "OLD" }));
      },
      selectTargetGame: async (id) => (await base.selectTargetGame(id)).map((t) => ({ ...t, name: "NEW" })),
    };
    const store = createAppStore(backend);
    await store.actions.boot();
    const rescan = store.actions.rescan();
    await store.actions.selectTargetGame("fortnite");
    await rescan;
    expect(new Set(store.getState().tweaks.map((t) => t.name))).toEqual(new Set(["NEW"]));
  });

  it("overlapping boots subscribe to progress once", async () => {
    const base = createMockBackend();
    let subscriptions = 0;
    const store = createAppStore({ ...base, onProgress: (h) => ((subscriptions += 1), base.onProgress(h)) });
    await Promise.all([store.actions.boot(), store.actions.boot()]);
    expect(subscriptions).toBe(1);
    await store.actions.createRestorePoint();
    const stages = store.getState().bus.map((e) => e.stage);
    expect(stages.filter((s) => s === "restore_check").length).toBe(1);
  });

  it("a failed re-read after a change is recorded, not dropped", async () => {
    const base = createMockBackend({ gateOpen: true });
    let failJournal = false;
    const store = createAppStore({
      ...base,
      listJournal: () => (failJournal ? Promise.reject({ kind: "storage", path: "j", detail: "gone" }) : base.listJournal()),
    });
    await store.actions.boot();
    failJournal = true;
    await store.actions.applyTweak("fixture.default");
    expect(store.getState().refreshError).toEqual({ kind: "storage", path: "j", detail: "gone" });
    failJournal = false;
    await store.actions.rescan();
    await store.actions.applyTweak("fixture.foreign");
    expect(store.getState().refreshError).toBeNull();
  });

  it("a new run clears the old verdict, and capture state belongs to its own comparison", async () => {
    const { store } = await booted();
    const sessionId = store.getState().proof.sessions[0]!.session.sessionId;
    await store.actions.compare(sessionId);
    expect(store.getState().proof.comparisons[sessionId]?.status).toBe("done");

    await store.actions.capture(sessionId, "after", 10, 0);
    const p = store.getState().proof;
    expect(p.comparisons[sessionId]).toBeUndefined();
    expect(p.captureOps[sessionId]?.status).toBe("done");
    expect(p.captureOps["some-other-session"]).toBeUndefined();
    expect(p.capturingSession).toBeNull();
  });

  it("an audit that started before a target-game pick does not undo the pick", async () => {
    const base = createMockBackend({
      latencyFor: (command) => (command === "auditSystem" ? 20 : command === "selectTargetGame" ? 80 : undefined),
    });
    const store = createAppStore(base);
    await store.actions.boot(); // leaves the first audit in flight
    const pick = store.actions.selectTargetGame("fortnite");
    await sleep(40); // the first audit (target: none) has landed by now
    expect(store.getState().targetGame).toBe("fortnite");
    await pick;
    await settle(store);
    expect(store.getState().targetGame).toBe("fortnite");
  });

  it("a restore attempt remembers where its own progress messages start", async () => {
    const { store } = await booted();
    await store.actions.createRestorePoint();
    const firstAttemptEnd = store.getState().bus.at(-1)!.id;
    const second = store.actions.createRestorePoint();
    expect(store.getState().restoreSinceBusId).toBe(firstAttemptEnd);
    await second;
  });
});

describe("apply the safe set", () => {
  it("applies every recommended change not yet applied, lists them, and undoes them together", async () => {
    const { store } = await booted({ gateOpen: true });
    const ids = recommendedIds(store.getState().tweaks);
    expect(ids).toEqual(["fixture.default"]);

    await store.actions.applyMany(ids);
    await settle(store);
    let s = store.getState();
    expect(s.applyManyOp.status).toBe("done");
    expect(s.lastChange).toMatchObject({ kind: "apply", tweakIds: ["fixture.default"], failed: [] });
    expect(s.tweaks.find((t) => t.id === "fixture.default")?.state.status).toBe("applied");
    expect(recommendedIds(s.tweaks)).toEqual([]);

    await store.actions.revertMany(["fixture.default"]);
    await settle(store);
    s = store.getState();
    expect(s.lastChange).toMatchObject({ kind: "revert", tweakIds: ["fixture.default"], failed: [] });
    expect(s.tweaks.find((t) => t.id === "fixture.default")?.state.status).toBe("default");
  });

  it("keeps going past a change that fails and reports it", async () => {
    const { store } = await booted({ gateOpen: true });
    await store.actions.applyMany(["fixture.blocked", "fixture.default"]);
    await settle(store);
    const change = store.getState().lastChange!;
    expect(change.tweakIds).toEqual(["fixture.default"]);
    expect(change.failed.map((f) => f.tweakId)).toEqual(["fixture.blocked"]);
  });
});

describe("recommended changes", () => {
  it("leave out look-and-feel changes, which stay one click each in Tools", async () => {
    const { store } = await booted({ gateOpen: true });
    const tweaks = store.getState().tweaks.map((t) => (t.id === "fixture.default" ? { ...t, category: "appearance" } : t));
    expect(recommendedIds(tweaks)).toEqual([]);
  });

  it("leave out changes with a cost, which are applied one at a time where their line shows", async () => {
    const { store } = await booted({ gateOpen: true });
    const tweaks = store
      .getState()
      .tweaks.map((t) => (t.id === "fixture.default" ? { ...t, tradeoff: "Uses more power." } : t));
    expect(recommendedIds(tweaks)).toEqual([]);
  });
});

describe("basic changes and the drift check", () => {
  it("list the basic changes not yet made first, then those in place", async () => {
    const { store } = await booted({ gateOpen: true });
    expect(basicTweaks(store.getState().tweaks).map((t) => t.id)).toEqual([
      "fixture.default",
      "fixture.applied",
      "fixture.foreign",
    ]);
  });

  it("apply again in one click only what Tools would apply without a confirmation", async () => {
    const { store } = await booted({ gateOpen: true });
    const drifted = store.getState().tweaks.find((t) => t.id === "fixture.drifted")!;
    expect(driftedTweaks([drifted])).toEqual({ all: [drifted], again: ["fixture.drifted"] });
    const advanced = { ...drifted, safety: "moderate" as const, tradeoff: "Uses more power." };
    expect(driftedTweaks([advanced])).toEqual({ all: [advanced], again: [] });
  });
});

describe("one-time actions", () => {
  it("empties the standby list and keeps the before and after for the card", async () => {
    const { store } = await booted();
    await store.actions.purgeStandby();
    const op = store.getState().standbyOp;
    expect(op.status).toBe("done");
    if (op.status !== "done") return;
    expect(op.value.before.cachedBytes).toBeGreaterThan(op.value.after.cachedBytes);
  });

  it("checks the connection and keeps each target's figures for the card", async () => {
    const { store } = await booted();
    await store.actions.checkConnection();
    const op = store.getState().netcheckOp;
    expect(op.status).toBe("done");
    if (op.status !== "done") return;
    expect(op.value.results.map((r) => r.target)).toEqual(["router", "cloudflare", "google"]);
    expect(op.value.reading).toBe("loss_past_router");
  });

  it("a refused connection check is reported with the engine's reason", async () => {
    const { store } = await booted({
      failures: { checkConnection: { kind: "internal", detail: "a Proof recording is running" } },
    });
    await store.actions.checkConnection();
    expect(store.getState().netcheckOp).toMatchObject({
      status: "failed",
      error: { kind: "internal", detail: "a Proof recording is running" },
    });
  });

  it("a refused purge is reported with the engine's reason, not dropped", async () => {
    const { store } = await booted({ failures: { purgeStandbyMemory: { kind: "internal", detail: "no privilege" } } });
    await store.actions.purgeStandby();
    expect(store.getState().standbyOp).toMatchObject({ status: "failed", error: { kind: "internal", detail: "no privilege" } });
  });

  it("clears only the chosen junk areas, then looks again", async () => {
    const { store } = await booted();
    await store.actions.measureCleanup();
    const before = store.getState().cleanupSizesOp;
    expect(before.status).toBe("done");
    await store.actions.runCleanup(["windows_temp", "user_temp"]);
    const op = store.getState().cleanupOp;
    if (op.status !== "done") throw new Error(`cleanup ${op.status}`);
    expect(op.value.areas.map((a) => a.area)).toEqual(["user_temp", "windows_temp"]);
    const after = store.getState().cleanupSizesOp;
    if (after.status !== "done" || before.status !== "done") throw new Error("sizes not read");
    const files = (sizes: typeof after.value, area: string) => sizes.find((s) => s.area === area)?.files;
    expect(files(after.value, "windows_temp")).toBe(0);
    expect(files(after.value, "user_temp")).toBe(op.value.areas[0]!.leftFiles);
    expect(files(after.value, "crash_dumps")).toBe(files(before.value, "crash_dumps"));
  });

  it("a refused cleanup is reported with the engine's reason, and nothing chosen does nothing", async () => {
    const { store } = await booted({ failures: { cleanupRun: { kind: "internal", detail: "a Proof recording is running" } } });
    await store.actions.runCleanup([]);
    expect(store.getState().cleanupOp.status).toBe("idle");
    await store.actions.runCleanup(["user_temp"]);
    expect(store.getState().cleanupOp).toMatchObject({ status: "failed", error: { detail: "a Proof recording is running" } });
  });

  it("optimizes the drive and keeps Windows' report for the card", async () => {
    const { store } = await booted();
    await store.actions.optimizeDrive();
    const op = store.getState().driveOp;
    if (op.status !== "done") throw new Error(`drive ${op.status}`);
    expect(op.value.drive).toBe("C:");
    expect(op.value.report.at(-1)).toBe("The operation completed successfully.");
  });

  it("a failed drive optimization keeps Windows' reason", async () => {
    const error = {
      kind: "command",
      what: "Drive optimization",
      exitCode: -1_996_488_662,
      detail: "defrag C: /O: The operation requested is not supported by the hardware backing the volume. (0x8900002A)",
    } as const;
    const { store } = await booted({ failures: { optimizeDrive: error } });
    await store.actions.optimizeDrive();
    expect(store.getState().driveOp).toEqual({ status: "failed", error });
  });

  it("each one-time action adds a line to the change record", async () => {
    const { store } = await booted();
    const actions = () => (store.getState().journal?.records ?? []).flatMap((r) => (r.record === "action" ? [r.action] : []));
    const before = actions().length;
    await store.actions.purgeStandby();
    await store.actions.runCleanup(["user_temp"]);
    await store.actions.optimizeDrive();
    expect(actions().slice(before)).toEqual(["purge_standby", "cleanup", "optimize_drive"]);
  });

  it("long work waits for other long work, as the engine runs one at a time", async () => {
    const { store } = await booted({ latencyFor: (command) => (command === "optimizeDrive" ? 20 : undefined) });
    const running = store.actions.optimizeDrive();
    expect(otherLongWork(store.getState(), "cleanup")).toBe("drive");
    expect(otherLongWork(store.getState(), "proof")).toBe("drive");
    expect(otherLongWork(store.getState(), "drive")).toBeNull();
    await running;
    expect(otherLongWork(store.getState(), "cleanup")).toBeNull();
  });
});

describe("while you play (catalogue step 5)", () => {
  const play = async (options: MockOptions = {}) => {
    const r = await booted(options);
    for (let i = 0; i < 50 && r.store.getState().play === null; i += 1) await new Promise((res) => setTimeout(res, 5));
    return r;
  };

  it("loads what the watcher sees after boot", async () => {
    const { store } = await play();
    expect(store.getState().play).toMatchObject({ game: null, gamingModeActive: false, timerHeld: null });
    expect(store.getState().play?.watched).toEqual(["fortnite", "roblox", "minecraft"]);
  });

  it("follows the watcher's events: Gaming Mode on while a game runs, then the change record is read again", async () => {
    const { store } = await play({ playing: "fortnite", gateOpen: true });
    const journal = store.getState().journal;
    const settings = store.getState().settings!;
    expect(await store.actions.saveSettings({ ...settings, gamingMode: true, gameTimer: true })).toBe(true);
    expect(store.getState().play).toMatchObject({ game: "fortnite", gamingModeActive: true, timerHeld: 5000, problem: null });
    for (let i = 0; i < 50 && store.getState().journal === journal; i += 1) await new Promise((res) => setTimeout(res, 5));
    expect(store.getState().journal).not.toBe(journal);
  });

  it("says why Gaming Mode is not on when there is no restore point", async () => {
    const { store } = await play({ playing: "roblox" });
    const settings = store.getState().settings!;
    await store.actions.saveSettings({ ...settings, gamingMode: true });
    expect(store.getState().play?.gamingModeActive).toBe(false);
    expect(store.getState().play?.problem).toMatch(/restore point/);
    await store.actions.createRestorePoint();
    expect(store.getState().play).toMatchObject({ gamingModeActive: true, problem: null });
  });

  it("a watcher event wins over an older status reply still in flight", async () => {
    const mock = createMockBackend({ playing: "fortnite", gateOpen: true });
    const stale = await mock.playStatus();
    let release = () => {};
    const backend: Backend = { ...mock, playStatus: () => new Promise<PlayStatus>((r) => (release = () => r(stale))) };
    const store = createAppStore(backend);
    await store.actions.boot();
    await backend.setSettings({ ...store.getState().settings!, gamingMode: true });
    release();
    await new Promise((res) => setTimeout(res, 0));
    expect(stale.gamingModeActive).toBe(false);
    expect(store.getState().play?.gamingModeActive).toBe(true);
  });
});

describe("startup apps (catalogue H12)", () => {
  const CHAT = "startup.user_run:Sample chat app";
  const app = (s: State, id: string) => s.startup?.apps.find((a) => a.tweak.id === id);

  it("is read only once something asks, then lists every entry with its switch", async () => {
    const { store } = await booted();
    expect(store.getState().startup).toBeNull();
    expect(store.getState().startupOp.status).toBe("idle");
    await store.actions.loadStartup();
    expect(store.getState().startupOp.status).toBe("done");
    expect(store.getState().startup?.apps.map((a) => a.name)).toEqual([
      "Sample chat app",
      "Sample game launcher",
      "Sample notes",
      "Sample updater",
      "SecurityHealth",
    ]);
  });

  it("turning one off is a change like any other: in Backups, undone by Undo all, and the list follows", async () => {
    const { store } = await booted({ gateOpen: true });
    await store.actions.loadStartup();
    await store.actions.applyTweak(CHAT);
    expect(store.getState().tweakOps[CHAT]).toEqual({ status: "done", value: "apply" });
    expect(app(store.getState(), CHAT)?.tweak.state.status).toBe("applied");
    expect(store.getState().journal?.applied.map((c) => c.tweakId)).toContain(CHAT);

    await store.actions.revertAll();
    expect(app(store.getState(), CHAT)?.tweak.state.status).toBe("default");
    expect(store.getState().journal?.applied.map((c) => c.tweakId)).not.toContain(CHAT);
  });

  it("keeps the engine's refusals: no restore point, and Windows Security", async () => {
    const { store } = await booted();
    await store.actions.loadStartup();
    await store.actions.applyTweak(CHAT);
    expect(store.getState().tweakOps[CHAT]).toMatchObject({ status: "failed", error: { kind: "blocked", reason: { code: "no_restore_point" } } });

    await store.actions.createRestorePoint();
    const security = "startup.machine_run:SecurityHealth";
    await store.actions.applyTweak(security);
    expect(store.getState().tweakOps[security]).toMatchObject({ status: "failed", error: { kind: "blocked", reason: { code: "protected_program" } } });
    expect(app(store.getState(), security)?.tweak.state.status).toBe("blocked");
  });

  it("an older list still in flight does not overwrite a newer one", async () => {
    const mock = createMockBackend({ gateOpen: true });
    const stale = await mock.listStartupApps();
    let release = () => {};
    let calls = 0;
    const backend: Backend = {
      ...mock,
      listStartupApps: () => {
        calls += 1;
        return calls === 1 ? new Promise<StartupList>((r) => (release = () => r(stale))) : mock.listStartupApps();
      },
    };
    const store = createAppStore(backend);
    await store.actions.boot();
    const first = store.actions.loadStartup();
    await store.actions.applyTweak(CHAT);
    expect(app(store.getState(), CHAT)?.tweak.state.status).toBe("applied");
    release();
    await first;
    expect(app(store.getState(), CHAT)?.tweak.state.status).toBe("applied");
  });
});

describe("MSI mode per device (catalogue H6)", () => {
  const GPU = "msi.PCI\\VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1\\4&2b0b1f0c&0&0008";
  const device = (s: State, id: string) => s.msi?.devices.find((d) => d.tweak.id === id);

  it("is read only once something asks, graphics card first", async () => {
    const { store } = await booted();
    expect(store.getState().msi).toBeNull();
    expect(store.getState().msiOp.status).toBe("idle");
    await store.actions.loadMsi();
    expect(store.getState().msiOp.status).toBe("done");
    expect(store.getState().msi?.devices.map((d) => [d.class, d.tweak.state.status])).toEqual([
      ["display", "default"],
      ["net", "foreign"],
    ]);
  });

  it("setting it is a change like any other: in Backups, undone by Undo all, and the list follows", async () => {
    const { store } = await booted({ gateOpen: true });
    await store.actions.loadMsi();
    await store.actions.applyTweak(GPU);
    expect(device(store.getState(), GPU)?.tweak.state.status).toBe("applied");
    expect(store.getState().journal?.applied.map((c) => c.tweakId)).toContain(GPU);

    await store.actions.revertAll();
    expect(device(store.getState(), GPU)?.tweak.state.status).toBe("default");
  });

  it("an older list still in flight does not overwrite a newer one", async () => {
    const mock = createMockBackend({ gateOpen: true });
    const stale = await mock.listMsiDevices();
    let release = () => {};
    let calls = 0;
    const backend: Backend = {
      ...mock,
      listMsiDevices: () => {
        calls += 1;
        return calls === 1 ? new Promise<MsiDeviceList>((r) => (release = () => r(stale))) : mock.listMsiDevices();
      },
    };
    const store = createAppStore(backend);
    await store.actions.boot();
    const first = store.actions.loadMsi();
    await store.actions.applyTweak(GPU);
    expect(device(store.getState(), GPU)?.tweak.state.status).toBe("applied");
    release();
    await first;
    expect(device(store.getState(), GPU)?.tweak.state.status).toBe("applied");
  });

  it("a failed read keeps the engine's error", async () => {
    const { store } = await booted({ failures: { listMsiDevices: { kind: "internal", detail: "no answer" } } });
    await store.actions.loadMsi();
    expect(store.getState().msiOp).toMatchObject({ status: "failed", error: { kind: "internal" } });
  });
});
