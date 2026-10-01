// Application state. One external store, read through `useSyncExternalStore`.
//
// Rules this file keeps:
// - State is replaced, never mutated, and only the slice that changed gets a
//   new reference. Select a slice (`s => s.tweaks`), not a fresh object built
//   in the selector, or React re-renders forever (plan section 7).
// - Every engine call that can overlap another of its kind is tagged. A reply
//   that is no longer the latest is dropped, so a slow answer never overwrites
//   a newer one (R20).
// - Every action records how it ended (`Op`), including failure with the
//   engine's own error. Nothing fails silently (R20).
// - Nothing here decides what the engine allows. The store asks; the engine
//   answers, including "blocked".

import type { Comparison } from "../generated/Comparison";
import type { ContextInfo } from "../generated/ContextInfo";
import type { EngineError } from "../generated/EngineError";
import type { GameInfo } from "../generated/GameInfo";
import type { JournalView } from "../generated/JournalView";
import type { Progress } from "../generated/Progress";
import type { ProofRun } from "../generated/ProofRun";
import type { ProofSession } from "../generated/ProofSession";
import type { ProofSessionSummary } from "../generated/ProofSessionSummary";
import type { RestoreOutcome } from "../generated/RestoreOutcome";
import type { RevertResult } from "../generated/RevertResult";
import type { Settings } from "../generated/Settings";
import type { Side } from "../generated/Side";
import type { SystemAudit } from "../generated/SystemAudit";
import type { TweakView } from "../generated/TweakView";
import { toEngineError } from "../lib/errors";
import type { Backend } from "../services/backend";

export type Op<T = null> =
  | { status: "idle" }
  | { status: "running" }
  | { status: "done"; value: T }
  | { status: "failed"; error: EngineError };

const IDLE = { status: "idle" } as const;
const RUNNING = { status: "running" } as const;

export interface BusEntry extends Progress {
  id: number;
  at: number;
}

/** What the result card after a change shows (plan section 7). */
export interface ChangeResult {
  kind: "apply" | "revert" | "revert_all";
  at: number;
  tweakIds: string[];
  failed: RevertResult[];
}

export interface ProofState {
  sessions: ProofSessionSummary[];
  runs: Readonly<Record<string, ProofRun[]>>;
  comparisons: Readonly<Record<string, Op<Comparison>>>;
  beginOp: Op<ProofSession>;
  /** Per comparison. The engine records one capture at a time; `capturingSession` says which. */
  captureOps: Readonly<Record<string, Op<ProofRun>>>;
  capturingSession: string | null;
  /** The last failure to load sessions or runs, until a load succeeds. */
  loadError: EngineError | null;
}

export interface State {
  sample: boolean;
  boot: { status: "loading" } | { status: "ready" } | { status: "failed"; error: EngineError };
  context: ContextInfo | null;
  settings: Settings | null;
  games: GameInfo[];
  targetGame: string | null;
  tweaks: TweakView[];
  audit: SystemAudit | null;
  journal: JournalView | null;
  auditOp: Op;
  targetOp: Op;
  settingsOp: Op;
  restoreOp: Op<RestoreOutcome>;
  /** Activity-log id at the start of the current restore attempt, so its
   * progress text never shows an earlier attempt's messages. */
  restoreSinceBusId: number;
  /** A re-read after a change failed: what is on screen may be out of date. */
  refreshError: EngineError | null;
  tweakOps: Readonly<Record<string, Op<"apply" | "revert">>>;
  revertAllOp: Op<RevertResult[]>;
  lastChange: ChangeResult | null;
  proof: ProofState;
  bus: BusEntry[];
}

export const BUS_LIMIT = 200;

export function initialState(sample: boolean): State {
  return {
    sample,
    boot: { status: "loading" },
    context: null,
    settings: null,
    games: [],
    targetGame: null,
    tweaks: [],
    audit: null,
    journal: null,
    auditOp: IDLE,
    targetOp: IDLE,
    settingsOp: IDLE,
    restoreOp: IDLE,
    restoreSinceBusId: 0,
    refreshError: null,
    tweakOps: {},
    revertAllOp: IDLE,
    lastChange: null,
    proof: { sessions: [], runs: {}, comparisons: {}, beginOp: IDLE, captureOps: {}, capturingSession: null, loadError: null },
    bus: [],
  };
}

export type AppStore = ReturnType<typeof createAppStore>;

export function createAppStore(backend: Backend, now: () => number = Date.now) {
  let state = initialState(backend.sample);
  const listeners = new Set<() => void>();
  const latest = new Map<string, number>();
  let busId = 0;
  let listening: Promise<() => void> | null = null;

  const set = (update: (s: State) => State) => {
    const next = update(state);
    if (next === state) return;
    state = next;
    for (const l of listeners) l();
  };

  /** Start a tagged request on `channel`; the returned check says whether it is still the newest. */
  const tag = (channel: string) => {
    const t = (latest.get(channel) ?? 0) + 1;
    latest.set(channel, t);
    return () => latest.get(channel) === t;
  };

  const failed = (e: unknown): { status: "failed"; error: EngineError } => ({ status: "failed", error: toEngineError(e) });

  // ---- refreshes -----------------------------------------------------------

  const refreshFailed = (e: unknown) => set((s) => ({ ...s, refreshError: toEngineError(e) }));
  const refreshOk = () => set((s) => (s.refreshError ? { ...s, refreshError: null } : s));

  /** Re-read the tweak list. Failures are recorded in `refreshError`, not thrown. */
  async function refreshTweaks(source: () => Promise<TweakView[]> = backend.listTweaks) {
    const current = tag("tweaks");
    try {
      const tweaks = await source();
      if (current()) {
        set((s) => ({ ...s, tweaks }));
        refreshOk();
      }
    } catch (e) {
      if (current()) refreshFailed(e);
    }
  }

  async function refreshAudit() {
    const current = tag("audit");
    // A target-game pick made while this audit runs is newer than what the
    // audit will report, so the audit must not undo it.
    const targetPickAtStart = latest.get("target") ?? 0;
    set((s) => ({ ...s, auditOp: RUNNING }));
    try {
      const audit = await backend.auditSystem();
      if (current()) {
        const targetUnchanged = (latest.get("target") ?? 0) === targetPickAtStart;
        set((s) => ({
          ...s,
          audit,
          targetGame: targetUnchanged ? audit.env.targetGame : s.targetGame,
          auditOp: { status: "done", value: null },
        }));
      }
    } catch (e) {
      if (current()) set((s) => ({ ...s, auditOp: failed(e) }));
    }
  }

  async function refreshJournal() {
    const current = tag("journal");
    try {
      const journal = await backend.listJournal();
      if (current()) {
        set((s) => ({ ...s, journal }));
        refreshOk();
      }
    } catch (e) {
      if (current()) refreshFailed(e);
    }
  }

  /** After anything that changed the PC: re-read what the engine now reports. */
  async function refreshAfterChange() {
    await Promise.allSettled([refreshTweaks(), refreshAudit(), refreshJournal()]);
  }

  // ---- actions -------------------------------------------------------------

  const actions = {
    async boot() {
      set((s) => ({ ...s, boot: { status: "loading" } }));
      try {
        // One subscription for the store's lifetime, even when boot() overlaps
        // itself (React StrictMode runs mount effects twice in development).
        listening ??= backend.onProgress((p) => {
          busId += 1;
          const entry: BusEntry = { ...p, id: busId, at: now() };
          set((s) => ({ ...s, bus: [...s.bus, entry].slice(-BUS_LIMIT) }));
        });
        await listening;
        const [context, settings, games, tweaks, journal, sessions] = await Promise.all([
          backend.context(),
          backend.getSettings(),
          backend.listGames(),
          backend.listTweaks(),
          backend.listJournal(),
          backend.proofSessions(),
        ]);
        set((s) => ({
          ...s,
          context,
          settings,
          games,
          tweaks,
          journal,
          proof: { ...s.proof, sessions },
          boot: { status: "ready" },
        }));
      } catch (e) {
        set((s) => ({ ...s, boot: { status: "failed", error: toEngineError(e) } }));
        return;
      }
      // The audit probes the machine and can take seconds; the shell shows
      // skeletons until it lands instead of holding the whole boot.
      void refreshAudit();
    },

    refreshAudit,

    async rescan() {
      const current = tag("rescan");
      await refreshTweaks(backend.rescan);
      if (current()) await refreshAudit();
    },

    async selectTargetGame(gameId: string | null) {
      const current = tag("target");
      // This reply carries a tweak list, so it also takes the "tweaks" channel:
      // an older list still in flight must not overwrite it.
      const tweaksCurrent = tag("tweaks");
      const previous = state.targetGame;
      set((s) => ({ ...s, targetGame: gameId, targetOp: RUNNING }));
      try {
        const tweaks = await backend.selectTargetGame(gameId);
        if (!current()) return;
        set((s) => ({ ...s, ...(tweaksCurrent() ? { tweaks } : {}), targetOp: { status: "done", value: null } }));
        await refreshAudit();
      } catch (e) {
        if (current()) set((s) => ({ ...s, targetGame: previous, targetOp: failed(e) }));
      }
    },

    /** Resolves to true once the engine has stored the settings. */
    async saveSettings(settings: Settings): Promise<boolean> {
      const current = tag("settings");
      set((s) => ({ ...s, settingsOp: RUNNING }));
      try {
        const saved = await backend.setSettings(settings);
        if (!current()) return false;
        set((s) => ({ ...s, settings: saved, settingsOp: { status: "done", value: null } }));
        await refreshAudit();
        return true;
      } catch (e) {
        if (current()) set((s) => ({ ...s, settingsOp: failed(e) }));
        return false;
      }
    },

    async createRestorePoint() {
      if (state.restoreOp.status === "running") return;
      set((s) => ({ ...s, restoreOp: RUNNING, restoreSinceBusId: busId }));
      try {
        const outcome = await backend.createRestorePoint();
        set((s) => ({ ...s, restoreOp: { status: "done", value: outcome } }));
      } catch (e) {
        set((s) => ({ ...s, restoreOp: failed(e) }));
      }
      await refreshAfterChange();
    },

    async applyTweak(id: string) {
      await change(id, "apply");
    },

    async revertTweak(id: string) {
      await change(id, "revert");
    },

    async revertAll() {
      if (state.revertAllOp.status === "running") return;
      set((s) => ({ ...s, revertAllOp: RUNNING }));
      try {
        const results = await backend.revertAll();
        set((s) => ({
          ...s,
          revertAllOp: { status: "done", value: results },
          lastChange: {
            kind: "revert_all",
            at: now(),
            tweakIds: results.filter((r) => r.ok).map((r) => r.tweakId),
            failed: results.filter((r) => !r.ok),
          },
        }));
      } catch (e) {
        set((s) => ({ ...s, revertAllOp: failed(e) }));
      }
      await refreshAfterChange();
    },

    dismissChange() {
      set((s) => (s.lastChange ? { ...s, lastChange: null } : s));
    },

    clearTweakOp(id: string) {
      set((s) => {
        if (!(id in s.tweakOps)) return s;
        const { [id]: _drop, ...rest } = s.tweakOps;
        return { ...s, tweakOps: rest };
      });
    },

    clearBus() {
      set((s) => (s.bus.length ? { ...s, bus: [] } : s));
    },

    // ---- proof -------------------------------------------------------------

    async loadSessions() {
      const current = tag("sessions");
      try {
        const sessions = await backend.proofSessions();
        if (current()) setProof({ sessions, loadError: null });
      } catch (e) {
        if (current()) setProof({ loadError: toEngineError(e) });
      }
    },

    async loadRuns(sessionId: string) {
      const current = tag(`runs:${sessionId}`);
      try {
        const runs = await backend.proofRuns(sessionId);
        if (current()) set((s) => ({ ...s, proof: { ...s.proof, runs: { ...s.proof.runs, [sessionId]: runs }, loadError: null } }));
      } catch (e) {
        if (current()) setProof({ loadError: toEngineError(e) });
      }
    },

    async beginSession(exe: string, gameId: string | null, gameBuild: string | null): Promise<ProofSession | null> {
      setProof({ beginOp: RUNNING });
      try {
        const session = await backend.proofBegin(exe, gameId, gameBuild);
        setProof({ beginOp: { status: "done", value: session } });
        await actions.loadSessions();
        return session;
      } catch (e) {
        setProof({ beginOp: failed(e) });
        return null;
      }
    },

    async capture(sessionId: string, side: Side, seconds: number, delaySeconds: number) {
      if (state.proof.capturingSession !== null) return;
      const setCapture = (op: Op<ProofRun>, capturing: string | null) =>
        set((s) => ({
          ...s,
          proof: { ...s.proof, capturingSession: capturing, captureOps: { ...s.proof.captureOps, [sessionId]: op } },
        }));
      setCapture(RUNNING, sessionId);
      try {
        const run = await backend.proofCapture(sessionId, side, seconds, delaySeconds);
        setCapture({ status: "done", value: run }, null);
        // The stored verdict no longer covers every run; it must be compared again.
        set((s) => {
          if (!(sessionId in s.proof.comparisons)) return s;
          const { [sessionId]: _stale, ...rest } = s.proof.comparisons;
          return { ...s, proof: { ...s.proof, comparisons: rest } };
        });
      } catch (e) {
        setCapture(failed(e), null);
      }
      await Promise.allSettled([actions.loadRuns(sessionId), actions.loadSessions()]);
    },

    async compare(sessionId: string) {
      const current = tag(`compare:${sessionId}`);
      setComparison(sessionId, RUNNING);
      try {
        const value = await backend.proofCompare(sessionId);
        if (current()) setComparison(sessionId, { status: "done", value });
      } catch (e) {
        if (current()) setComparison(sessionId, failed(e));
      }
    },
  };

  function setProof(patch: Partial<ProofState>) {
    set((s) => ({ ...s, proof: { ...s.proof, ...patch } }));
  }

  function setComparison(sessionId: string, op: Op<Comparison>) {
    set((s) => ({ ...s, proof: { ...s.proof, comparisons: { ...s.proof.comparisons, [sessionId]: op } } }));
  }

  async function change(id: string, kind: "apply" | "revert") {
    if (state.tweakOps[id]?.status === "running") return;
    const setOp = (op: Op<"apply" | "revert">) => set((s) => ({ ...s, tweakOps: { ...s.tweakOps, [id]: op } }));
    setOp(RUNNING);
    try {
      await (kind === "apply" ? backend.applyTweak(id) : backend.revertTweak(id));
      setOp({ status: "done", value: kind });
      set((s) => ({ ...s, lastChange: { kind, at: now(), tweakIds: [id], failed: [] } }));
    } catch (e) {
      setOp(failed(e));
    }
    await refreshAfterChange();
  }

  return {
    getState: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    actions,
    dispose() {
      void listening?.then((off) => off());
      listening = null;
      listeners.clear();
    },
  };
}
