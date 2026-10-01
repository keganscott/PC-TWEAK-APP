import { describe, expect, it } from "vitest";

import { createMockBackend, type MockOptions } from "../services/mockIpc";
import { BUS_LIMIT, createAppStore, type State } from "./store";

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
    expect(await store.actions.saveSettings({ language: "technical", rigClassOverride: "high" })).toBe(false);
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
