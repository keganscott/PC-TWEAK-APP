// The one seam between the UI and the engine. Inside Tauri it is the typed IPC
// client (`src/ipc.ts`); in a plain browser (`vite dev`, tests) it is the
// SAMPLE mock. Views and the store only ever see this interface.

import type { UnlistenFn } from "@tauri-apps/api/event";

import { engine, onProgress, proof } from "../ipc";

import type { AreaSize } from "../generated/AreaSize";
import type { CleanupArea } from "../generated/CleanupArea";
import type { CleanupReport } from "../generated/CleanupReport";
import type { Comparison } from "../generated/Comparison";
import type { ContextInfo } from "../generated/ContextInfo";
import type { GameInfo } from "../generated/GameInfo";
import type { JournalEntry } from "../generated/JournalEntry";
import type { JournalView } from "../generated/JournalView";
import type { Progress } from "../generated/Progress";
import type { ProofRun } from "../generated/ProofRun";
import type { ProofSession } from "../generated/ProofSession";
import type { ProofSessionSummary } from "../generated/ProofSessionSummary";
import type { RestoreOutcome } from "../generated/RestoreOutcome";
import type { RevertResult } from "../generated/RevertResult";
import type { Settings } from "../generated/Settings";
import type { Side } from "../generated/Side";
import type { StandbyPurge } from "../generated/StandbyPurge";
import type { SystemAudit } from "../generated/SystemAudit";
import type { TweakView } from "../generated/TweakView";

export interface Backend {
  /** True for the mock: every screen must then show the SAMPLE label. */
  readonly sample: boolean;
  context(): Promise<ContextInfo>;
  listTweaks(): Promise<TweakView[]>;
  listGames(): Promise<GameInfo[]>;
  getSettings(): Promise<Settings>;
  setSettings(settings: Settings): Promise<Settings>;
  selectTargetGame(gameId: string | null): Promise<TweakView[]>;
  rescan(): Promise<TweakView[]>;
  auditSystem(): Promise<SystemAudit>;
  createRestorePoint(): Promise<RestoreOutcome>;
  applyTweak(id: string): Promise<JournalEntry[]>;
  revertTweak(id: string): Promise<JournalEntry[]>;
  revertAll(): Promise<RevertResult[]>;
  listJournal(): Promise<JournalView>;
  purgeStandbyMemory(): Promise<StandbyPurge>;
  cleanupMeasure(): Promise<AreaSize[]>;
  cleanupRun(areas: CleanupArea[]): Promise<CleanupReport>;
  proofBegin(exe: string, gameId: string | null, gameBuild: string | null): Promise<ProofSession>;
  proofCapture(sessionId: string, side: Side, seconds: number, delaySeconds: number): Promise<ProofRun>;
  proofCompare(sessionId: string): Promise<Comparison>;
  proofSessions(): Promise<ProofSessionSummary[]>;
  proofRuns(sessionId: string): Promise<ProofRun[]>;
  onProgress(handler: (p: Progress) => void): Promise<UnlistenFn>;
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** The real engine over Tauri IPC. */
export function tauriBackend(): Backend {
  return {
    sample: false,
    ...engine,
    proofBegin: proof.beginSession,
    proofCapture: proof.capture,
    proofCompare: proof.compare,
    proofSessions: proof.listSessions,
    proofRuns: proof.runs,
    onProgress,
  };
}

/** Builds that may fall back to the SAMPLE mock: `vite dev`, and the end-to-end
 * test build (`VITE_SAMPLE=1`). A release build has neither, so the mock is
 * dropped from it entirely and the app refuses to run outside its window. */
const SAMPLE_ALLOWED = import.meta.env.DEV || import.meta.env.VITE_SAMPLE === "1";

export async function defaultBackend(): Promise<Backend> {
  if (isTauri()) return tauriBackend();
  if (SAMPLE_ALLOWED) {
    const { createMockBackend } = await import("./mockIpc");
    return createMockBackend({ latencyMs: 250 });
  }
  throw new Error("PeakTweaks' interface only runs inside the PeakTweaks app.");
}
