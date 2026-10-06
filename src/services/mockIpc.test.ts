import { describe, expect, it } from "vitest";

import type { EngineError } from "../generated/EngineError";
import * as fx from "../generated/fixtures";
import { EngineFault } from "../ipc";
import { explain } from "../lib/errors";
import { createMockBackend } from "./mockIpc";

const kindOf = async (p: Promise<unknown>) => {
  try {
    await p;
    return null;
  } catch (e) {
    return e instanceof EngineFault ? e.error.kind : "not an EngineFault";
  }
};

describe("the SAMPLE mock keeps the engine's rules", () => {
  it("says it is sample data", () => {
    expect(createMockBackend().sample).toBe(true);
  });

  it("refuses changes without a restore point, as the engine does", async () => {
    const b = createMockBackend();
    expect(await kindOf(b.applyTweak("fixture.default"))).toBe("blocked");
    await b.createRestorePoint();
    expect(await kindOf(b.applyTweak("fixture.default"))).toBeNull();
  });

  it("cannot undo what was never applied, and rejects unknown ids", async () => {
    const b = createMockBackend({ gateOpen: true });
    expect(await kindOf(b.revertTweak("fixture.default"))).toBe("no_journal_entry");
    expect(await kindOf(b.applyTweak("nope"))).toBe("unknown_tweak");
    expect(await kindOf(b.selectTargetGame("nope"))).toBe("unknown_game");
  });

  it("journals every change and every restore point", async () => {
    const b = createMockBackend();
    const start = (await b.listJournal()).records.length;
    await b.createRestorePoint();
    await b.applyTweak("fixture.default");
    const journal = await b.listJournal();
    // As the engine: the restore-frequency change (write, commit), the restore
    // point, then the apply (write, commit).
    expect(journal.records.length).toBe(start + 5);
    expect(journal.records.some((r) => r.record === "restore_point")).toBe(true);
    const applied = journal.applied.map((c) => [c.tweakId, c.kind]);
    expect(applied).toContainEqual(["fixture.default", "catalogue"]);
    expect(applied).toContainEqual(["system.restore.frequency", "internal"]);

    // Undo all reverts everything listed, the engine's own change included.
    const results = await b.revertAll();
    expect(results.map((r) => r.tweakId).sort()).toEqual(journal.applied.map((c) => c.tweakId).sort());
    expect((await b.listJournal()).applied).toEqual([]);
  });

  it("reports progress to subscribers until they unsubscribe", async () => {
    const b = createMockBackend();
    const seen: string[] = [];
    const off = await b.onProgress((p) => seen.push(p.stage));
    await b.createRestorePoint();
    off();
    await b.createRestorePoint();
    expect(seen).toEqual(["restore_check", "restore_create", "restore_verify"]);
  });

  it("keeps the seeded proof session consistent with its summary", async () => {
    const b = createMockBackend();
    const [summary] = await b.proofSessions();
    const runs = await b.proofRuns(summary!.session.sessionId);
    expect(runs.filter((r) => r.side === "before").length).toBe(summary!.beforeRuns);
    expect(runs.filter((r) => r.side === "after").length).toBe(summary!.afterRuns);
  });
});

const ALL_KINDS: Record<EngineError["kind"], true> = {
  not_elevated: true,
  user_context_unresolved: true,
  user_hive_not_loaded: true,
  registry: true,
  unsupported_value_type: true,
  win32: true,
  storage: true,
  insecure_storage: true,
  no_journal_entry: true,
  blocked: true,
  unknown_tweak: true,
  unknown_game: true,
  context_violation: true,
  command: true,
  wmi: true,
  internal: true,
  already_running: true,
};

describe("error wording", () => {
  it("every engine error kind gets a plain sentence", () => {
    const kinds = new Set<string>();
    for (const e of fx.engineErrors) {
      const text = explain(e);
      if (e.kind === "blocked") {
        // A block is worded by the engine itself; shown as given.
        expect(text.title).toBe(e.reason.message);
      } else {
        expect(text.title.length).toBeGreaterThan(5);
      }
      expect(text.title).not.toMatch(/undefined|\[object/);
      kinds.add(e.kind);
    }
    // The fixtures hold every variant (a Rust test enforces that); this list is
    // typed so a new engine error kind fails `tsc` until it is added here.
    expect([...kinds].sort()).toEqual(Object.keys(ALL_KINDS).sort());
  });
});
