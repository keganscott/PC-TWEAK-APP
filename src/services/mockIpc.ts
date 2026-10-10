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
import type { ActionDone } from "../generated/ActionDone";
import type { AppliedChange } from "../generated/AppliedChange";
import type { AreaCleanup } from "../generated/AreaCleanup";
import type { AreaSize } from "../generated/AreaSize";
import type { CleanupArea } from "../generated/CleanupArea";
import type { CleanupReport } from "../generated/CleanupReport";
import type { BlockedReason } from "../generated/BlockedReason";
import type { EngineError } from "../generated/EngineError";
import type { JournalEntry } from "../generated/JournalEntry";
import type { LiveReadings } from "../generated/LiveReadings";
import type { MsiDeviceList } from "../generated/MsiDeviceList";
import type { PlayStatus } from "../generated/PlayStatus";
import type { Progress } from "../generated/Progress";
import type { ProofRun } from "../generated/ProofRun";
import type { ProofSessionSummary } from "../generated/ProofSessionSummary";
import type { Record as JournalRecord } from "../generated/Record";
import type { Settings } from "../generated/Settings";
import type { StartupApp } from "../generated/StartupApp";
import type { StartupList } from "../generated/StartupList";
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
  /** The known game the SAMPLE watcher sees running. Default: none. */
  playing?: string;
  /** The SAMPLE PC is on Wi-Fi only while that game runs. */
  onWifi?: boolean;
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

/** One startup entry after `id`, one of its two switches, took `state`. As the
 * engine reads them: both switches look at the same value in Windows, so the
 * other one follows. */
function startupSwitched(app: StartupApp, id: string, state: TweakView["state"]): StartupApp {
  if (app.tweak.id === id) {
    const off = state.status === "applied" || state.status === "foreign";
    const turnOn: TweakView["state"] = off ? { status: "default" } : { status: "foreign" };
    return { ...app, tweak: { ...app.tweak, state }, turnOn: { ...app.turnOn, state: turnOn } };
  }
  if (app.turnOn.id === id) {
    const on = state.status === "applied";
    // A protected program's turning-off switch stays blocked either way.
    const tweak: TweakView["state"] =
      app.tweak.state.status === "blocked" ? app.tweak.state : on ? { status: "default" } : { status: "foreign" };
    return { ...app, turnOn: { ...app.turnOn, state }, tweak: { ...app.tweak, state: tweak } };
  }
  return app;
}

function blocked(reason: BlockedReason): EngineFault {
  return new EngineFault({ kind: "blocked", reason });
}

export function createMockBackend(options: MockOptions = {}): Backend {
  const failures = { ...options.failures };
  const listeners = new Set<(p: Progress) => void>();
  let tweaks = sampleTweaks();
  // Startup apps (H12): each entry's switch is a change like any other, so
  // apply, undo, Undo all and Backups treat it as one.
  let startup: StartupList = clone(fx.startupList) as StartupList;
  // How many live readings were asked for (they move a little each time).
  let live = 0;
  // SAMPLE files kept in memory: down after a clean, then filling again.
  const CACHED_FULL = 6 * 2 ** 30;
  let cached = CACHED_FULL;
  // MSI mode per device (H6): changes like any other, as startup switches.
  let msi: MsiDeviceList = clone(fx.msiDevices) as MsiDeviceList;
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

  // Junk cleaner (H28): what an area still holds after the SAMPLE cleanup ran
  // there (the files "in use"). A second run deletes nothing more.
  const remaining = new Map<CleanupArea, { bytes: number; files: number }>();
  const cleanupSizes = (): AreaSize[] =>
    clone(fx.cleanupSizes as AreaSize[]).map((s) => ({ ...s, ...remaining.get(s.area) }));
  const nothingDone = (area: CleanupArea): AreaCleanup => ({
    area,
    removedBytes: 0,
    removedFiles: 0,
    leftBytes: 0,
    leftFiles: 0,
    leftExamples: [],
    skipped: [],
  });

  // One-time actions leave a line in the change record, as in the engine
  // (failures too there; here a failure is thrown before anything runs).
  const recordAction = (done: ActionDone) => {
    seq += 1;
    records.push({ record: "action", seq, unixMs: Date.now(), action: done.action, done, error: null });
  };

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
    ...[...tweaks, ...startup.apps.flatMap((a) => [a.tweak, a.turnOn]), ...msi.devices.map((d) => d.tweak)]
      .filter((t) => t.state.status === "applied" || t.state.status === "drifted")
      .map((t): AppliedChange => ({ tweakId: t.id, name: t.name, kind: "catalogue" })),
    ...(internalApplied ? [clone(INTERNAL)] : []),
  ];

  // The game watcher (catalogue step 5). The SAMPLE one reports a status only:
  // Gaming Mode's changes are not added to the SAMPLE Backups list. As in the
  // engine, they need a restore point.
  const playListeners = new Set<(p: PlayStatus) => void>();
  let historyForgotten = false;
  const playStatus = (): PlayStatus => {
    const game = options.playing ?? null;
    const wanted = game !== null && settings.gamingMode;
    return {
      ...clone(fx.playStatus),
      game,
      gamingModeActive: wanted && gateOpen,
      timerHeld: game !== null && settings.gameTimer ? fx.playStatus.timerHeld : null,
      problem: wanted && !gateOpen ? "Gaming Mode is not fully on: There is no verified restore point, so there is nothing to roll back to." : null,
      onWifi: game !== null && (options.onWifi ?? false),
      ...(historyForgotten ? { lastSession: null, history: [] } : {}),
    };
  };
  let shownPlay = JSON.stringify(playStatus());
  /** As the watcher: an event only when what it would show changed. */
  const emitPlay = () => {
    const now = playStatus();
    if (JSON.stringify(now) === shownPlay) return;
    shownPlay = JSON.stringify(now);
    for (const l of playListeners) l(clone(now));
  };

  const find = (id: string): TweakView => {
    const t =
      tweaks.find((x) => x.id === id) ??
      startup.apps.find((a) => a.tweak.id === id)?.tweak ??
      startup.apps.find((a) => a.turnOn.id === id)?.turnOn ??
      msi.devices.find((d) => d.tweak.id === id)?.tweak;
    if (!t) throw new EngineFault({ kind: "unknown_tweak", tweakId: id });
    return t;
  };

  const setState = (id: string, state: TweakView["state"]) => {
    tweaks = tweaks.map((t) => (t.id === id ? { ...t, state } : t));
    startup = {
      ...startup,
      apps: startup.apps.map((a) => startupSwitched(a, id, state)),
    };
    msi = {
      ...msi,
      devices: msi.devices.map((d) => (d.tweak.id === id ? { ...d, tweak: { ...d.tweak, state } } : d)),
    };
  };

  return {
    sample: true,
    context: () => reply("context", [], () => clone(fx.contextInfo)),
    listTweaks: () => reply("listTweaks", [], () => clone(tweaks)),
    listGames: () => reply("listGames", [], () => clone(fx.games)),
    getSettings: () => reply("getSettings", [], () => clone(settings)),
    setSettings: (s, base) =>
      reply("setSettings", [s, base], () => {
        // As the engine does (`Settings::with_changes`): only what changed
        // from `base` goes over what is saved now.
        if (base) {
          const next = clone(settings) as Record<string, unknown>;
          const keys = new Set([...Object.keys(base), ...Object.keys(s)]);
          for (const k of keys) {
            const want = (s as Record<string, unknown>)[k];
            if (JSON.stringify(want) !== JSON.stringify((base as Record<string, unknown>)[k])) next[k] = clone(want ?? null);
          }
          settings = next as Settings;
        } else {
          settings = clone(s);
        }
        emitPlay();
        return clone(settings);
      }),
    selectTargetGame: (gameId) =>
      reply("selectTargetGame", [gameId], () => {
        if (gameId !== null && !fx.games.some((g) => g.id === gameId)) {
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
        emitPlay();
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
    purgeStandbyMemory: () =>
      reply("purgeStandbyMemory", [], () => {
        emit("standby", "Emptying the standby list");
        const result = { ...clone(fx.standbyPurge), unixMs: Date.now() };
        cached = result.after.cachedBytes;
        recordAction({ action: "purge_standby", cachedBefore: result.before.cachedBytes, cachedAfter: result.after.cachedBytes });
        return result;
      }),
    cleanupMeasure: () => reply("cleanupMeasure", [], cleanupSizes),
    cleanupRun: (areas) =>
      reply("cleanupRun", [areas], () => {
        emit("cleanup", "Clearing junk files");
        const report: CleanupReport = { areas: [], unixMs: Date.now() };
        for (const size of cleanupSizes()) {
          if (!areas.includes(size.area)) continue;
          const sample = (fx.cleanupReport.areas as AreaCleanup[]).find((a) => a.area === size.area);
          const again = remaining.has(size.area);
          const done: AreaCleanup =
            sample && !again
              ? clone(sample)
              : again
                ? { ...nothingDone(size.area), leftBytes: size.bytes, leftFiles: size.files, skipped: size.skipped }
                : { ...nothingDone(size.area), removedBytes: size.bytes, removedFiles: size.files, skipped: size.skipped };
          remaining.set(size.area, { bytes: done.leftBytes, files: done.leftFiles });
          report.areas.push(done);
        }
        const total = (pick: (a: AreaCleanup) => number) => report.areas.reduce((sum, a) => sum + pick(a), 0);
        recordAction({
          action: "cleanup",
          areas: report.areas.map((a) => a.area),
          removedBytes: total((a) => a.removedBytes),
          removedFiles: total((a) => a.removedFiles),
          leftFiles: total((a) => a.leftFiles),
        });
        return report;
      }),
    listStartupApps: () => reply("listStartupApps", [], () => clone(startup)),
    listMsiDevices: () => reply("listMsiDevices", [], () => clone(msi)),
    liveReadings: () =>
      reply("liveReadings", [], (): LiveReadings => {
        // SAMPLE numbers that move a little, so the tiles can be seen updating.
        live += 1;
        cached = Math.min(CACHED_FULL, cached + 0.15 * 2 ** 30);
        const wobble = (base: number) => base + ((live * 7) % 9) - 4;
        return {
          cpuBusyPercent: { state: "yes", value: wobble(23) },
          memory: { state: "yes", value: { totalBytes: 16 * 2 ** 30, availableBytes: 6.9 * 2 ** 30, cachedBytes: cached } },
          gpus: {
            state: "yes",
            value: [
              {
                name: { state: "yes", value: "Sample graphics card" },
                busyPercent: { state: "yes", value: wobble(41) },
                temperatureC: { state: "yes", value: wobble(58) },
                memoryUsedBytes: { state: "yes", value: 3.2 * 2 ** 30 },
                memoryTotalBytes: { state: "yes", value: 8 * 2 ** 30 },
              },
            ],
          },
          unixMs: Date.now(),
        };
      }),
    checkConnection: () =>
      reply("checkConnection", [], () => {
        emit("netcheck", "Checking the connection");
        return { ...clone(fx.networkCheck), unixMs: Date.now() };
      }),
    forgetPlayHistory: () =>
      reply("forgetPlayHistory", [], () => {
        historyForgotten = true;
        emitPlay();
        return playStatus();
      }),
    launchGame: (gameId) =>
      reply("launchGame", [gameId], () => {
        if (!fx.games.some((g) => g.id === gameId)) throw new EngineFault({ kind: "unknown_game", gameId });
        const install = audit().env.gameInstalls?.find((i) => i.gameId === gameId);
        if (install?.steamApp == null) {
          throw new EngineFault({
            kind: "command",
            what: "Steam",
            exitCode: null,
            detail: install ? `${install.name} was not found in a Steam library` : `${gameId} was not found on this PC`,
          });
        }
        // SAMPLE: nothing is started.
        return null;
      }),
    optimizeDrive: () =>
      reply("optimizeDrive", [], () => {
        emit("drive", "Optimizing the Windows drive");
        const result = { ...clone(fx.driveOptimization), unixMs: Date.now() };
        recordAction({ action: "optimize_drive", drive: result.drive, seconds: result.seconds });
        return result;
      }),
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
    playStatus: () => reply("playStatus", [], playStatus),
    onPlay: async (handler): Promise<UnlistenFn> => {
      playListeners.add(handler);
      return () => playListeners.delete(handler);
    },
    // The SAMPLE app has no tray icon, so nothing changes settings from outside.
    onSettings: async (): Promise<UnlistenFn> => () => {},
  };
}
