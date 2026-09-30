// Typed client for the Rust engine. Every type comes from src/generated (ts-rs),
// so a change to a Rust type that this file does not follow fails `tsc`.
//
// The engine builds its own environment, license and restore-gate state; there
// is deliberately no function here that sends any of that. A Rust test
// (src-tauri/src/command_audit.rs) fails if a command exists that this file
// does not wrap.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Comparison } from "./generated/Comparison";
import type { ContextInfo } from "./generated/ContextInfo";
import type { EngineError } from "./generated/EngineError";
import type { GameInfo } from "./generated/GameInfo";
import type { JournalEntry } from "./generated/JournalEntry";
import type { JournalView } from "./generated/JournalView";
import type { Progress } from "./generated/Progress";
import type { ProofRun } from "./generated/ProofRun";
import type { ProofSession } from "./generated/ProofSession";
import type { ProofSessionSummary } from "./generated/ProofSessionSummary";
import type { RestoreOutcome } from "./generated/RestoreOutcome";
import type { RevertResult } from "./generated/RevertResult";
import type { Settings } from "./generated/Settings";
import type { Side } from "./generated/Side";
import type { SystemAudit } from "./generated/SystemAudit";
import type { TweakView } from "./generated/TweakView";

/** What a failed command throws: the engine's structured error, not a string. */
export class EngineFault extends Error {
  constructor(readonly error: EngineError) {
    super(describe(error));
    this.name = "EngineFault";
  }
}

function isEngineError(e: unknown): e is EngineError {
  return typeof e === "object" && e !== null && typeof (e as { kind?: unknown }).kind === "string";
}

/** Tauri rejects with whatever the command returned in `Err`, or a string for
 * failures before it (unknown command, permission denied). Normalise both. */
export function asEngineError(e: unknown): EngineError {
  if (isEngineError(e)) return e;
  return { kind: "internal", detail: typeof e === "string" ? e : e instanceof Error ? e.message : String(e) };
}

function describe(e: EngineError): string {
  switch (e.kind) {
    case "blocked":
      return e.reason.message;
    case "internal":
    case "user_context_unresolved":
    case "registry":
    case "win32":
    case "storage":
    case "insecure_storage":
    case "context_violation":
    case "command":
    case "wmi":
      return `${e.kind}: ${e.detail}`;
    default:
      return e.kind;
  }
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (e) {
    throw new EngineFault(asEngineError(e));
  }
}

export const engine = {
  context: () => call<ContextInfo>("engine_context"),
  listTweaks: () => call<TweakView[]>("list_tweaks"),
  listGames: () => call<GameInfo[]>("list_games"),
  /** Preferences: rig-class override and plain/technical wording. Never a gate or licence. */
  getSettings: () => call<Settings>("get_settings"),
  /** Replace the preferences; resolves to what is now stored. */
  setSettings: (settings: Settings) => call<Settings>("set_settings", { settings }),
  /** Pick a known game, or `null` to clear. The id is validated in Rust. */
  selectTargetGame: (gameId: string | null) => call<TweakView[]>("select_target_game", { gameId }),
  /** Re-run every probe (nothing cached) and return the refreshed list. */
  rescan: () => call<TweakView[]>("rescan"),
  /** Hardware, security state, restore state and anti-cheat readiness. */
  auditSystem: () => call<SystemAudit>("audit_system"),
  /** Turn on System Protection if needed, create a restore point, prove it exists. */
  createRestorePoint: () => call<RestoreOutcome>("create_restore_point"),
  applyTweak: (id: string) => call<JournalEntry[]>("apply_tweak", { id }),
  revertTweak: (id: string) => call<JournalEntry[]>("revert_tweak", { id }),
  revertAll: () => call<RevertResult[]>("revert_all"),
  listJournal: () => call<JournalView>("list_journal"),
};

/** Measure whether a change did anything. Every number comes from stored runs. */
export const proof = {
  /** Start a before/after comparison. The free plan allows one. */
  beginSession: (exe: string, gameId: string | null, gameBuild: string | null) =>
    call<ProofSession>("proof_begin_session", { exe, gameId, gameBuild }),
  /** Capture one run. Takes `delaySeconds + seconds`; listen to `onProgress`. */
  capture: (sessionId: string, side: Side, seconds: number, delaySeconds: number) =>
    call<ProofRun>("proof_capture", { sessionId, side, seconds, delaySeconds }),
  /** Better / no measurable change / worse. Show `headline`; do not reword it. */
  compare: (sessionId: string) => call<Comparison>("proof_compare", { sessionId }),
  listSessions: () => call<ProofSessionSummary[]>("proof_list_sessions"),
  runs: (sessionId: string) => call<ProofRun[]>("proof_runs", { sessionId }),
};

/** Subscribe to progress events from long-running commands. */
export function onProgress(handler: (p: Progress) => void): Promise<UnlistenFn> {
  return listen<Progress>("engine://progress", (event) => handler(event.payload));
}
