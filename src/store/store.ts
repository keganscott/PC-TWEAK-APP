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

import type { AreaSize } from "../generated/AreaSize";
import type { CleanupArea } from "../generated/CleanupArea";
import type { CleanupReport } from "../generated/CleanupReport";
import type { Comparison } from "../generated/Comparison";
import type { ContextInfo } from "../generated/ContextInfo";
import type { DriveOptimization } from "../generated/DriveOptimization";
import type { EngineError } from "../generated/EngineError";
import type { GameInfo } from "../generated/GameInfo";
import type { JournalView } from "../generated/JournalView";
import type { MsiDeviceList } from "../generated/MsiDeviceList";
import type { NetworkCheck } from "../generated/NetworkCheck";
import type { PlayStatus } from "../generated/PlayStatus";
import type { Progress } from "../generated/Progress";
import type { ProofRun } from "../generated/ProofRun";
import type { ProofSession } from "../generated/ProofSession";
import type { ProofSessionSummary } from "../generated/ProofSessionSummary";
import type { RestoreOutcome } from "../generated/RestoreOutcome";
import type { RevertResult } from "../generated/RevertResult";
import type { Settings } from "../generated/Settings";
import type { Side } from "../generated/Side";
import type { LiveReadings } from "../generated/LiveReadings";
import type { StandbyPurge } from "../generated/StandbyPurge";
import type { StartupList } from "../generated/StartupList";
import type { SystemAudit } from "../generated/SystemAudit";
import type { TweakView } from "../generated/TweakView";
import { explain, toEngineError } from "../lib/errors";
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
  /** "Apply all basic changes", "Apply recommended" and the result card's Undo. */
  applyManyOp: Op<null>;
  /** "Empty the standby list" (catalogue E6); keeps the last result for its card. */
  standbyOp: Op<StandbyPurge>;
  /** "Clear out junk files" (catalogue H28): what each area holds now. */
  cleanupSizesOp: Op<AreaSize[]>;
  /** The last cleanup, for its card. */
  cleanupOp: Op<CleanupReport>;
  /** "Optimize the Windows drive" (catalogue H29); keeps the last run for its card. */
  driveOp: Op<DriveOptimization>;
  /** "Check the connection" (catalogue E4); keeps the last check for its card. */
  netcheckOp: Op<NetworkCheck>;
  /** "Play" on a game found in a Steam library, per game id. */
  launchOps: Readonly<Record<string, Op>>;
  lastChange: ChangeResult | null;
  proof: ProofState;
  /** The game watcher (catalogue step 5); null until it first answers. */
  play: PlayStatus | null;
  /** Startup apps (H12), once the Tools section has asked. Kept on screen
   * while it is read again; `startupOp` says how the latest read went. */
  startup: StartupList | null;
  startupOp: Op;
  /** MSI mode per device (H6), once the Advanced devices section has asked. */
  msi: MsiDeviceList | null;
  msiOp: Op;
  /** Home's live readings: the newest answer, kept while the next is read. */
  live: LiveReadings | null;
  liveOp: Op;
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
    applyManyOp: IDLE,
    standbyOp: IDLE,
    cleanupSizesOp: IDLE,
    cleanupOp: IDLE,
    driveOp: IDLE,
    netcheckOp: IDLE,
    launchOps: {},
    lastChange: null,
    proof: { sessions: [], runs: {}, comparisons: {}, beginOp: IDLE, captureOps: {}, capturingSession: null, loadError: null },
    play: null,
    startup: null,
    startupOp: IDLE,
    msi: null,
    msiOp: IDLE,
    live: null,
    liveOp: IDLE,
    bus: [],
  };
}

/**
 * The changes "Apply all basic changes" (Home) and "Apply recommended" make: safe-tier,
 * not yet in effect, not refused by the engine. A setting the PC already has
 * (foreign) or one changed after we applied it (drifted) is left alone.
 * DECISIONS 15.22: "recommended" stands in for evidence grades A and B until
 * the tweak dictionary gives grades (N2). Look-and-feel changes are left out,
 * and so are changes with a cost line: a one-click set shows no lines, so
 * those are applied one at a time in Tools, where the line is read first.
 */
export function recommendedIds(tweaks: readonly TweakView[]): string[] {
  return tweaks
    .filter(
      (t) =>
        t.safety === "safe" &&
        t.category !== APPEARANCE &&
        !t.tradeoff &&
        !t.blocked &&
        t.state.status === "default",
    )
    .map((t) => t.id);
}

/**
 * The basic changes Home recommends: what `recommendedIds` would apply, plus
 * those already in effect (applied here, or set before), so Home can show how many are in place. Not yet made first,
 * then the larger effect first, then by name.
 */
export function basicTweaks(tweaks: readonly TweakView[]): TweakView[] {
  const offered = new Set(recommendedIds(tweaks));
  const inPlace = (t: TweakView) => t.state.status === "applied" || t.state.status === "foreign";
  return tweaks
    .filter((t) => t.safety === "safe" && t.category !== APPEARANCE && !t.tradeoff && (offered.has(t.id) || inPlace(t)))
    .sort(
      (a, b) =>
        Number(inPlace(a)) - Number(inPlace(b)) ||
        Number(b.impact === "extreme") - Number(a.impact === "extreme") ||
        a.name.localeCompare(b.name),
    );
}

/**
 * The drift check: changes PeakTweaks made that Windows no longer has (a
 * Windows update or another program set them back). `again` are the ones
 * applied again in one click: those that ask for no confirmation in Tools
 * (DECISIONS 15.22) and that the engine does not refuse. The rest are
 * listed and re-applied one at a time in Tools.
 */
export function driftedTweaks(tweaks: readonly TweakView[]): { all: TweakView[]; again: string[] } {
  const all = tweaks.filter((t) => t.state.status === "drifted");
  const again = all.filter((t) => !t.blocked && !(t.tradeoff && t.safety !== "safe")).map((t) => t.id);
  return { all, again };
}

/** Look-and-feel changes: one click each in Tools, never part of a one-click
 * set, so the safe set never changes how someone's desktop looks. */
export const APPEARANCE = "appearance";

/** Long work the engine runs one at a time (`Activity` in commands.rs). */
export type LongWork = "proof" | "cleanup" | "drive";

/** The long work running now other than `own`, which would make the engine
 * refuse `own`. */
export function otherLongWork(s: State, own: LongWork): LongWork | null {
  if (own !== "proof" && s.proof.capturingSession !== null) return "proof";
  if (own !== "cleanup" && s.cleanupOp.status === "running") return "cleanup";
  if (own !== "drive" && s.driveOp.status === "running") return "drive";
  return null;
}

export type AppStore = ReturnType<typeof createAppStore>;

export function createAppStore(backend: Backend, now: () => number = Date.now) {
  let state = initialState(backend.sample);
  const listeners = new Set<() => void>();
  const latest = new Map<string, number>();
  let busId = 0;
  let listening: Promise<() => void> | null = null;
  let watchingPlay: Promise<() => void> | null = null;
  let watchingSettings: Promise<() => void> | null = null;
  /** Counts settings saved from the tray, so a window save that was in
   * flight across one re-reads what the engine kept last. */
  let traySaves = 0;

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

  /** The watcher's status. An event is newer than any reply still in flight,
   * so it takes the same channel. Gaming Mode starting or ending changes what
   * Backups lists, so the change record is read again then. */
  function showPlay(play: PlayStatus) {
    tag("play");
    const wasActive = state.play?.gamingModeActive;
    set((s) => ({ ...s, play }));
    if (wasActive !== undefined && wasActive !== play.gamingModeActive) void refreshJournal();
  }

  /** Settings saved from the tray icon. Not tagged: a save from the window
   * still in flight keeps its own spinner and result, and re-reads the
   * settings when it lands (the event and its reply can arrive in either
   * order). Gaming Mode turned off puts its changes back, which changes what
   * Backups lists. */
  function showSettings(settings: Settings) {
    traySaves += 1;
    const before = state.settings;
    set((s) => ({ ...s, settings }));
    if (before && before.gamingMode !== settings.gamingMode) void Promise.allSettled([refreshAudit(), refreshJournal()]);
  }

  async function refreshPlay() {
    const current = tag("play");
    try {
      const play = await backend.playStatus();
      if (current()) set((s) => ({ ...s, play }));
    } catch {
      // Advisory: the section shows nothing until the watcher answers.
    }
  }

  /** What each junk-file area holds now. The newest answer wins. */
  async function refreshCleanupSizes() {
    const current = tag("cleanup_sizes");
    set((s) => ({ ...s, cleanupSizesOp: RUNNING }));
    try {
      const sizes = await backend.cleanupMeasure();
      if (current()) set((s) => ({ ...s, cleanupSizesOp: { status: "done", value: sizes } }));
    } catch (e) {
      if (current()) set((s) => ({ ...s, cleanupSizesOp: failed(e) }));
    }
  }

  /** The startup apps and their switches. The newest answer wins. */
  async function refreshStartup() {
    const current = tag("startup");
    set((s) => ({ ...s, startupOp: RUNNING }));
    try {
      const startup = await backend.listStartupApps();
      if (current()) set((s) => ({ ...s, startup, startupOp: { status: "done", value: null } }));
    } catch (e) {
      if (current()) set((s) => ({ ...s, startupOp: failed(e) }));
    }
  }

  /** The graphics and network devices and their MSI mode. The newest answer wins. */
  async function refreshMsi() {
    const current = tag("msi");
    set((s) => ({ ...s, msiOp: RUNNING }));
    try {
      const msi = await backend.listMsiDevices();
      if (current()) set((s) => ({ ...s, msi, msiOp: { status: "done", value: null } }));
    } catch (e) {
      if (current()) set((s) => ({ ...s, msiOp: failed(e) }));
    }
  }

  /** After anything that changed the PC: re-read what the engine now reports.
   * The startup and device lists only once something has shown them.
   *
   * The tool list comes first, on its own: it is what the user is looking at,
   * and the engine answers one request at a time, so a list queued behind the
   * audit (seconds of probing) left a card showing its old state with its
   * button live, and a second click applied or undid again (Kegan's
   * "two clicks", 2026-10-09). `listed` runs once the new list is in. */
  async function refreshAfterChange(listed?: () => void) {
    await refreshTweaks();
    listed?.();
    await Promise.allSettled([
      refreshAudit(),
      refreshJournal(),
      ...(state.startupOp.status === "idle" ? [] : [refreshStartup()]),
      ...(state.msiOp.status === "idle" ? [] : [refreshMsi()]),
    ]);
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
        watchingPlay ??= backend.onPlay(showPlay);
        watchingSettings ??= backend.onSettings(showSettings);
        await Promise.all([listening, watchingPlay, watchingSettings]);
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
      void refreshPlay();
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
    /** Save `settings`, made from `base` (the settings the caller showed;
     * the store's own by default). Only fields changed from `base` are
     * saved over what the engine has now. */
    async saveSettings(settings: Settings, base?: Settings): Promise<boolean> {
      const current = tag("settings");
      set((s) => ({ ...s, settingsOp: RUNNING }));
      try {
        const before = state.settings;
        const trayBefore = traySaves;
        // Saved against what this window showed, so a Gaming Mode switch
        // made from the tray meanwhile is kept, not put back.
        let saved = await backend.setSettings(settings, base ?? before ?? undefined);
        if (traySaves !== trayBefore) saved = await backend.getSettings();
        if (!current()) return false;
        set((s) => ({ ...s, settings: saved, settingsOp: { status: "done", value: null } }));
        // Turning Gaming Mode off mid-game puts its changes back at once.
        await Promise.allSettled([refreshAudit(), ...(before?.gamingMode !== saved.gamingMode ? [refreshJournal()] : [])]);
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

    /** Apply each in turn; one failure does not stop the rest. */
    async applyMany(ids: readonly string[]) {
      await many(ids, "apply");
    },

    /** Home's one click on a first run: a restore point, then the basic
     * changes still to make, read again after it (the engine checks the gate
     * on each change as always). Nothing is applied if the point failed. */
    async restoreThenApply() {
      await actions.createRestorePoint();
      if (state.restoreOp.status !== "done") return;
      await many(recommendedIds(state.tweaks), "apply");
    },

    /** Undo each in turn (the result card's "Undo these"). */
    async revertMany(ids: readonly string[]) {
      await many(ids, "revert");
    },

    /** Empty Windows' standby list. Changes no setting, so nothing to undo; the change record gains a line. */
    async purgeStandby() {
      if (state.standbyOp.status === "running") return;
      set((s) => ({ ...s, standbyOp: RUNNING }));
      try {
        const result = await backend.purgeStandbyMemory();
        set((s) => ({ ...s, standbyOp: { status: "done", value: result } }));
      } catch (e) {
        set((s) => ({ ...s, standbyOp: failed(e) }));
      }
      // The engine keeps a line in the change record whether it worked or not.
      await refreshJournal();
    },

    /** Echoes to the router and two public DNS servers. Changes nothing and
     * leaves no line in the change record. */
    async checkConnection() {
      if (state.netcheckOp.status === "running") return;
      set((s) => ({ ...s, netcheckOp: RUNNING }));
      try {
        const result = await backend.checkConnection();
        set((s) => ({ ...s, netcheckOp: { status: "done", value: result } }));
      } catch (e) {
        set((s) => ({ ...s, netcheckOp: failed(e) }));
      }
    },

    /** Ask Steam to start a game found in a Steam library. Changes nothing
     * and leaves no line in the change record. */
    async launchGame(gameId: string) {
      if (state.launchOps[gameId]?.status === "running") return;
      const put = (op: Op) => set((s) => ({ ...s, launchOps: { ...s.launchOps, [gameId]: op } }));
      put(RUNNING);
      try {
        await backend.launchGame(gameId);
        put({ status: "done", value: null });
      } catch (e) {
        put(failed(e));
      }
    },

    /** Run Windows' own drive optimisation. Changes no setting, so nothing to undo. */
    async optimizeDrive() {
      if (state.driveOp.status === "running") return;
      set((s) => ({ ...s, driveOp: RUNNING }));
      try {
        const result = await backend.optimizeDrive();
        set((s) => ({ ...s, driveOp: { status: "done", value: result } }));
      } catch (e) {
        set((s) => ({ ...s, driveOp: failed(e) }));
      }
      await refreshJournal();
    },

    /** Read the startup apps. Reads only; a switch is turned with applyTweak / revertTweak. */
    async loadStartup() {
      await refreshStartup();
    },

    /** Read the graphics and network devices. Reads only; MSI mode is set with applyTweak / revertTweak. */
    async loadMsi() {
      await refreshMsi();
    },

    /** Read the live processor, memory and GPU readings once. Reads only. The
     * newest answer wins; one already being read is not asked for again. */
    async readLive() {
      if (state.liveOp.status === "running") return;
      const current = tag("live");
      set((s) => ({ ...s, liveOp: RUNNING }));
      try {
        const live = await backend.liveReadings();
        if (current()) set((s) => ({ ...s, live, liveOp: { status: "done", value: null } }));
      } catch (e) {
        if (current()) set((s) => ({ ...s, liveOp: failed(e) }));
      }
    },

    /** Look at what each junk-file area holds. Reads only. */
    async measureCleanup() {
      await refreshCleanupSizes();
    },

    /** Delete the junk in `areas`, then look again. Cannot be undone; the card asks first. */
    async runCleanup(areas: readonly CleanupArea[]) {
      if (areas.length === 0 || state.cleanupOp.status === "running") return;
      set((s) => ({ ...s, cleanupOp: RUNNING }));
      try {
        const report = await backend.cleanupRun([...areas]);
        set((s) => ({ ...s, cleanupOp: { status: "done", value: report } }));
      } catch (e) {
        set((s) => ({ ...s, cleanupOp: failed(e) }));
      }
      await Promise.allSettled([refreshCleanupSizes(), refreshJournal()]);
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

  async function many(ids: readonly string[], kind: "apply" | "revert") {
    if (state.applyManyOp.status === "running" || ids.length === 0) return;
    set((s) => ({ ...s, applyManyOp: RUNNING }));
    const done: string[] = [];
    const failures: RevertResult[] = [];
    // Each card keeps its spinner until the new list shows how it ended.
    const ended: Record<string, Op<"apply" | "revert">> = {};
    for (const id of ids) {
      set((s) => ({ ...s, tweakOps: { ...s.tweakOps, [id]: RUNNING } }));
      try {
        await (kind === "apply" ? backend.applyTweak(id) : backend.revertTweak(id));
        done.push(id);
        ended[id] = { status: "done", value: kind };
      } catch (e) {
        const op = failed(e);
        failures.push({ tweakId: id, ok: false, error: explain(op.error).title });
        ended[id] = op;
      }
    }
    await refreshAfterChange(() =>
      set((s) => ({
        ...s,
        tweakOps: { ...s.tweakOps, ...ended },
        applyManyOp: { status: "done", value: null },
        lastChange: { kind, at: now(), tweakIds: done, failed: failures },
      })),
    );
  }

  async function change(id: string, kind: "apply" | "revert") {
    if (state.tweakOps[id]?.status === "running") return;
    const setOp = (op: Op<"apply" | "revert">) => set((s) => ({ ...s, tweakOps: { ...s.tweakOps, [id]: op } }));
    setOp(RUNNING);
    let ended: Op<"apply" | "revert">;
    try {
      await (kind === "apply" ? backend.applyTweak(id) : backend.revertTweak(id));
      ended = { status: "done", value: kind };
    } catch (e) {
      ended = failed(e);
    }
    // The spinner stays until the card can show the result (refreshAfterChange).
    await refreshAfterChange(() =>
      set((s) => ({
        ...s,
        tweakOps: { ...s.tweakOps, [id]: ended },
        ...(ended.status === "done" ? { lastChange: { kind, at: now(), tweakIds: [id], failed: [] } } : {}),
      })),
    );
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
      void watchingPlay?.then((off) => off());
      void watchingSettings?.then((off) => off());
      listening = null;
      watchingPlay = null;
      watchingSettings = null;
      listeners.clear();
    },
  };
}
