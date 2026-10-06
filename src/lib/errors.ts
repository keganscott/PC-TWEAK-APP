// Turns the engine's structured errors into words a user can act on. The raw
// detail is kept for the technical register and for support.

import type { EngineError } from "../generated/EngineError";
import { asEngineError, EngineFault } from "../ipc";
import { blockedHint } from "./blocked";

export interface ErrorText {
  /** One plain sentence: what happened. */
  title: string;
  /** What the user can do next, when there is something. */
  hint: string | null;
  /** The engine's own words, for the technical view and bug reports. */
  detail: string | null;
}

export function toEngineError(e: unknown): EngineError {
  return e instanceof EngineFault ? e.error : asEngineError(e);
}

export function explain(error: EngineError): ErrorText {
  switch (error.kind) {
    case "blocked":
      return { title: error.reason.message, hint: blockedHint(error.reason), detail: error.reason.trigger };
    case "not_elevated":
      return {
        title: "PeakTweaks is not running as administrator.",
        hint: "Close it and start it again, then accept the Windows prompt.",
        detail: null,
      };
    case "user_context_unresolved":
      return {
        title: "PeakTweaks could not tell which Windows account to change.",
        hint: "Sign in to Windows normally and start PeakTweaks from that account.",
        detail: error.detail,
      };
    case "user_hive_not_loaded":
      return {
        title: "The settings for your Windows account are not loaded right now.",
        hint: "Sign out and back in, then try again.",
        detail: error.sid,
      };
    case "registry":
      return { title: "Windows refused a settings change.", hint: "Nothing was left half-done.", detail: `${error.path}: ${error.detail}` };
    case "unsupported_value_type":
      return {
        title: "A setting is stored in a form PeakTweaks does not change.",
        hint: "It was left exactly as it was.",
        detail: `${error.path}\\${error.value} (type ${error.vtype})`,
      };
    case "win32":
      return { title: "A Windows call failed.", hint: null, detail: `${error.call} (${error.code}): ${error.detail}` };
    case "storage":
      return { title: "PeakTweaks could not save its records.", hint: "Check free disk space, then try again.", detail: `${error.path}: ${error.detail}` };
    case "insecure_storage":
      return {
        title: "PeakTweaks' data folder is not protected, so it will not use it.",
        hint: "An administrator should delete the folder named below; PeakTweaks recreates it safely.",
        detail: `${error.path}: ${error.detail}`,
      };
    case "no_journal_entry":
      return { title: "There is nothing to undo for this change.", hint: null, detail: error.tweakId };
    case "unknown_tweak":
      return { title: "That change is not in this version of PeakTweaks.", hint: null, detail: error.tweakId };
    case "unknown_game":
      return { title: "That game is not on PeakTweaks' list.", hint: null, detail: error.gameId };
    case "context_violation":
      return { title: "PeakTweaks stopped a change that went outside what it may touch.", hint: null, detail: error.detail };
    case "command":
      return { title: `${error.what} did not finish.`, hint: null, detail: error.detail };
    case "wmi":
      return {
        title: error.timedOut ? "Windows took too long to answer." : "Windows could not answer a question about this PC.",
        hint: error.timedOut ? "Try again in a moment." : null,
        detail: `${error.namespace}: ${error.detail}`,
      };
    case "internal":
      return { title: "Something went wrong inside PeakTweaks.", hint: "Try again. If it keeps happening, restart PeakTweaks.", detail: error.detail };
    case "already_running":
      return {
        title: "PeakTweaks is already open.",
        hint: "Close this window and use the one that is already open. If the field-check tool is running, let it finish first. Only one copy runs at a time so the record of changes stays correct.",
        detail: null,
      };
  }
}

export function explainUnknown(e: unknown): ErrorText {
  return explain(toEngineError(e));
}
