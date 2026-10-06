// SAMPLE backend for `vite dev` and tests, outside Tauri. Never used inside the
// app: `defaultBackend()` picks the real IPC whenever Tauri is present.
//
// Every value starts from `src/generated/fixtures.ts`, which is real engine
// output checked against the generated types, so the shapes cannot drift from
// the engine. The mock keeps the engine's rules so screens behave the same:
// nothing applies without a restore point, a blocked tweak stays blocked,
// revert needs a journal entry, and failures arrive as `EngineFault`s.
// `backend.sample` is true, and the UI shows a SAMPLE banner for it.

import type { UnlistenFn } from "@tauri-apps/api/event";

import * as fx from "../generated/fixtures";
import type { AppliedChange } from "../generated/AppliedChange";
import type { BlockedReason } from "../generated/BlockedReason";
import type { EngineError } from "../generated/EngineError";
import type { JournalEntry } from "../generated/JournalEntry";
import type { Progress } from "../generated/Progress";
import type { ProofRun } from "../generated/ProofRun";
import type { ProofSessionSummary } from "../generated/ProofSessionSummary";
import type { Record as JournalRecord } from "../generated/Record";
import type { Settings } from "../generated/Settings";
import type { SystemAudit } from "../generated/SystemAudit";
import type { TweakView } from "../generated/TweakView";
import { EngineFault } from "../ipc";
import type { Backend } from "./backend";

export interface MockOptions {
  /** Delay before every reply, in ms. Lets tests and demos see loading states. */
  latencyMs?: number;
  /** Per-command delay overrides, e.g. to make one request slower than the next. */
  latencyFor?: (command: string, args: unknown[]) => number | undefined;
  /** Start with a verified restore point (the gate open). Default: closed. */
  gateOpen?: boolean;
  /** Make the next call to `command` fail with `error`. */
  failures?: Partial<Record<keyof Backend, EngineError>>;
  /** Start as a first launch: the welcome has not been seen. Default: seen. */
  firstRun?: boolean;
}

const clone = <T>(v: T): T => structuredClone(v);

const SAMPLE_NAMES: Record<string, { name: string; summary: string }> = {
  "fixture.default": { name: "Sample setting A", summary: "SAMPLE: a change that is not applied yet." },
  "fixture.applied": { name: "Sample setting B", summary: "SAMPLE: a change PeakTweaks applied." },
  "fixture.foreign": { name: "Sample setting C", summary: "SAMPLE: already set by something other than PeakTweaks." },
  "fixture.blocked": { name: "Sample setting D", summary: "SAMPLE: not available on this PC." },
  "fixture.unknown": { name: "Sample setting E", summary: "SAMPLE: its current state could not be read." },
  "fixture.drifted": { name: "Sample setting F", summary: "SAMPLE: applied, then changed outside PeakTweaks." },
};

function sampleTweaks(): TweakView[] {
  return clone(fx.tweakViews as TweakView[]).map((t) => ({
    ...t,
    name: SAMPLE_NAMES[t.id]?.name ?? `Sample ${t.id}`,
    summary: SAMPLE_NAMES[t.id]?.summary ?? "SAMPLE",
    category: t.id === "fixture.blocked" ? "Input" : "System",
    target: `HKEY_LOCAL_MACHINE\\SOFTWARE\\PeakTweaks\\Sample\\${t.id}`,
  }));
}

function blocked(reason: BlockedReason): EngineFault {
  return new EngineFault({ kind: "blocked", reason });
}

export function createMockBackend(options: MockOptions = {}): Backend {
  const failures = { ...options.failures };
  const listeners = new Set<(p: Progress) => void>();
  let tweaks = sampleTweaks();
  let settings: Settings = { ...(clone(fx.systemAudit.settings) as Settings), welcomeSeen: !options.firstRun };
  let targetGame: string | null = null;
  let gateOpen = options.gateOpen ?? false;
  let seq = 100;
  const records: JournalRecord[] = clone(fx.journalView.records) as JournalRecord[];
  const sessions: ProofSessionSummary[] = clone(fx.proofSessions) as ProofSessionSummary[];
  const runs = new Map<string, ProofRun[]>();
  // The seeded session holds the runs its summary and the sample comparison name.
  {
    const seeded = fx.proofSessions[0]!;
    const ids = [...fx.comparison.beforeRunIds, ...fx.comparison.afterRunIds];
    runs.set(
      seeded.session.sessionId,
      ids.map((runId, i) => ({
        ...(clone(fx.proofRun) as ProofRun),
        runId,
        side: i < fx.comparison.beforeRunIds.length ? "before" : "after",
        index: (i % fx.comparison.beforeRunIds.length) + 1,
      })),
    );
  }

  const emit = (stage: string, message: string, tweakId: string | null = null) => {
    for (const l of listeners) l({ stage, message, tweakId });
  };

  async function reply<T>(command: keyof Backend, args: unknown[], produce: () => T): Promise<T> {
    const ms = options.latencyFor?.(command, args) ?? options.latencyMs ?? 0;
    if (ms > 0) await new Promise((r) => setTimeout(r, ms));
    const failure = failures[command];
    if (failure) {
      delete failures[command];
      throw new EngineFault(failure);
    }
    return produce();
  }

  const audit = (): SystemAudit => {
    const a = clone(fx.systemAudit) as SystemAudit;
    a.env.restoreGateOpen = gateOpen;
    a.env.targetGame = targetGame;
    if (a.env.restore) a.env.restore.gateOpen = gateOpen;
    a.settings = clone(settings);
    a.effectiveRigClass = settings.rigClassOverride ?? a.effectiveRigClass;
    return a;
  };

  const write = (tweakId: string, action: "apply" | "revert"): JournalEntry => {
    seq += 1;
    const entry: JournalEntry = {
      ...(clone(fx.journalView.records[0]) as JournalEntry),
      seq,
      txId: seq,
      unixMs: Date.now(),
      tweakId,
      action,
    };
    records.push({ record: "write", ...entry });
    seq += 1;
    records.push({ record: "commit", seq, txId: entry.txId, unixMs: Date.now(), tweakId, action });
    return entry;
  };

  // The engine's own restore-frequency change (an internal tweak): made with
  // the first restore point, listed in Backups, undoable like any change.
  const INTERNAL = fx.journalView.applied.find((c) => c.kind === "internal")!;
  let internalApplied = false;
  const outstanding = (): AppliedChange[] => [
    ...tweaks
      .filter((t) => t.state.status === "applied" || t.state.status === "drifted")
      .map((t): AppliedChange => ({ tweakId: t.id, name: t.name, kind: "catalogue" })),
    ...(internalApplied ? [clone(INTERNAL)] : []),
  ];

  const find = (id: string): TweakView => {
    const t = tweaks.find((x) => x.id === id);
    if (!t) throw new EngineFault({ kind: "unknown_tweak", tweakId: id });
    return t;
  };

  const setState = (id: string, state: TweakView["state"]) => {
    tweaks = tweaks.map((t) => (t.id === id ? { ...t, state } : t));
  };

  return {
    sample: true,
    context: () => reply("context", [], () => clone(fx.contextInfo)),
    listTweaks: () => reply("listTweaks", [], () => clone(tweaks)),
    listGames: () =>
      reply("listGames", [], () => [
        { id: "fortnite", name: "Fortnite" },
        { id: "minecraft", name: "Minecraft" },
        { id: "roblox", name: "Roblox" },
      ]),
    getSettings: () => reply("getSettings", [], () => clone(settings)),
    setSettings: (s) =>
      reply("setSettings", [s], () => {
        settings = clone(s);
        return clone(settings);
      }),
    selectTargetGame: (gameId) =>
      reply("selectTargetGame", [gameId], () => {
        if (gameId !== null && !["fortnite", "minecraft", "roblox"].includes(gameId)) {
          throw new EngineFault({ kind: "unknown_game", gameId });
        }
        targetGame = gameId;
        return clone(tweaks);
      }),
    rescan: () => reply("rescan", [], () => clone(tweaks)),
    auditSystem: () => reply("auditSystem", [], audit),
    createRestorePoint: () =>
      reply("createRestorePoint", [], () => {
        emit("restore_check", "Checking System Protection");
        emit("restore_create", "Creating the restore point (this can take a minute)");
        emit("restore_verify", "Checking Windows recorded it");
        gateOpen = true;
        if (!internalApplied) {
          write(INTERNAL.tweakId, "apply");
          internalApplied = true;
        }
        seq += 1;
        records.push({ record: "restore_point", ...clone(fx.journalView.records[3]), seq, unixMs: Date.now() } as JournalRecord);
        return clone(fx.restoreOutcome);
      }),
    applyTweak: (id) =>
      reply("applyTweak", [id], () => {
        const t = find(id);
        if (t.state.status === "blocked") throw blocked(t.state.reason);
        if (t.blocked) throw blocked(t.blocked);
        if (!gateOpen) {
          throw blocked({
            code: "no_restore_point",
            trigger: null,
            message: "There is no verified restore point, so there is nothing to roll back to.",
          });
        }
        emit("apply", "Applying", id);
        const entry = write(id, "apply");
        setState(id, { status: "applied" });
        return [entry];
      }),
    revertTweak: (id) =>
      reply("revertTweak", [id], () => {
        if (id === INTERNAL.tweakId && internalApplied) {
          emit("revert", "Undoing", id);
          internalApplied = false;
          return [write(id, "revert")];
        }
        const t = find(id);
        // As in the engine: anything with an apply still on record can be undone.
        if (t.state.status !== "applied" && t.state.status !== "drifted") {
          throw new EngineFault({ kind: "no_journal_entry", tweakId: id });
        }
        emit("revert", "Undoing", id);
        const entry = write(id, "revert");
        setState(id, { status: "default" });
        return [entry];
      }),
    revertAll: () =>
      reply("revertAll", [], () =>
        outstanding().map((c) => {
          write(c.tweakId, "revert");
          if (c.kind === "internal") internalApplied = false;
          else setState(c.tweakId, { status: "default" });
          return { tweakId: c.tweakId, ok: true, error: null };
        }),
      ),
    listJournal: () =>
      reply("listJournal", [], () => ({
        applied: outstanding(),
        records: clone(records),
        warnings: clone(fx.journalView.warnings),
        offlineError: null,
      })),
    proofBegin: (exe, gameId, gameBuild) =>
      reply("proofBegin", [exe, gameId, gameBuild], () => {
        const now = Date.now();
        const session = { ...clone(fx.proofSessions[0]!.session), sessionId: `session-${now}`, createdUnixMs: now, exe, gameId, gameBuild };
        sessions.unshift({ session, beforeRuns: 0, afterRuns: 0 });
        return clone(session);
      }),
    proofCapture: (sessionId, side, seconds, delaySeconds) =>
      reply("proofCapture", [sessionId, side, seconds, delaySeconds], () => {
        const summary = sessions.find((s) => s.session.sessionId === sessionId);
        if (!summary) throw new EngineFault({ kind: "internal", detail: `no proof session ${sessionId}` });
        emit("proof_capture", `Recording ${side} run`);
        const list = runs.get(sessionId) ?? [];
        const index = list.filter((r) => r.side === side).length + 1;
        // As the engine: each run records the catalogue changes applied now.
        const appliedTweaks = outstanding()
          .filter((c) => c.kind === "catalogue")
          .map((c) => c.tweakId);
        const run: ProofRun = {
          ...clone(fx.proofRun),
          runId: `run-${Date.now()}-${list.length}`,
          sessionId,
          side,
          index,
          seconds,
          delaySeconds,
          appliedTweaks,
        };
        runs.set(sessionId, [...list, run]);
        if (side === "before") summary.beforeRuns += 1;
        else summary.afterRuns += 1;
        return clone(run);
      }),
    proofCompare: (sessionId) => reply("proofCompare", [sessionId], () => clone(fx.comparison)),
    proofSessions: () => reply("proofSessions", [], () => clone(sessions)),
    proofRuns: (sessionId) => reply("proofRuns", [sessionId], () => clone(runs.get(sessionId) ?? [])),
    onProgress: async (handler): Promise<UnlistenFn> => {
      listeners.add(handler);
      return () => listeners.delete(handler);
    },
  };
}
