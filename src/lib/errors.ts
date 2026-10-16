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
      // Also a failed Undo, which may have put back some values: those still
      // changed stay on the record, so pressing Undo again finishes it.
      return {
        title: "Windows refused a settings change.",
        hint: "Anything it had changed was put back, or is still on PeakTweaks' record so Undo can put it back.",
        detail: `${error.path}: ${error.detail}`,
      };
    case "unsupported_value_type":
      return {
        title: "A setting is stored in a form PeakTweaks does not change.",
        hint: "It was left exactly as it was.",
        detail: `${error.path}\\${error.value} (type ${error.vtype})`,
      };
    case "win32":
      return { ...win32Text(error.code), detail: `${error.call} (${error.code}): ${error.detail}` };
    case "storage":
      return { title: "PeakTweaks could not save its records.", hint: "Check free disk space, then try again.", detail: `${error.path}: ${error.detail}` };
    case "insecure_storage":
      return {
        title: "PeakTweaks' data folder is not protected, so it will not use it.",
        hint: "An administrator should delete the folder named below; PeakTweaks recreates it safely.",
        detail: `${error.path}: ${error.detail}`,
      };
    case "settings_file":
      return {
        title: "PeakTweaks could not change a game's settings file.",
        hint: "Close the game, then try again.",
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
      return { ...commandText(error.what, error.detail), detail: `${error.what}: ${error.detail}` };
    case "wmi":
      return {
        title: error.timedOut ? "Windows took too long to answer." : "Windows could not answer a question about this PC.",
        hint: error.timedOut ? "Try again in a moment." : null,
        detail: `${error.namespace}: ${error.detail}`,
      };
    case "internal":
      return { title: "Something went wrong inside PeakTweaks.", hint: "Try again. If it keeps happening, restart PeakTweaks.", detail: error.detail };
    case "partly_applied":
      return {
        title: "This change was made only in part, and PeakTweaks could not put that part back.",
        hint: "Press Undo on it, here or in Backups: PeakTweaks' record kept each part it changed.",
        detail: `${error.tweakId}: ${error.detail}; putting it back: ${error.undoDetail}`,
      };
    case "already_running":
      return {
        title: "PeakTweaks is already open.",
        hint: "Close this window and use the one that is already open. If the field-check tool is running, let it finish first. Only one copy runs at a time so the record of changes stays correct.",
        detail: null,
      };
  }
}

// The engine names the step that failed (`what`) in its own terms, often a
// Windows command; these turn the common ones into a sentence and a next step.
// Anything not listed keeps the engine's name, so nothing is ever hidden.
function commandText(what: string, detail: string): Omit<ErrorText, "detail"> {
  const d = detail.toLowerCase();
  const timedOut = d.includes("and was stopped");
  const busy = "Windows was busy and did not answer in time. Try again in a moment.";
  if (/restore|checkpoint-computer/i.test(what)) {
    return {
      title: "Windows did not make a restore point.",
      hint: timedOut
        ? busy
        : "Check that System Protection is on for drive C: and has disk space, then try again. If a restore point was made recently, Windows may skip a new one.",
    };
  }
  if (what === "NVIDIA settings" || what === "AMD graphics settings") {
    const panel = what === "NVIDIA settings" ? "NVIDIA Control Panel" : "AMD Software";
    return {
      title: `The ${what === "NVIDIA settings" ? "NVIDIA" : "AMD"} driver did not accept the change.`,
      hint: `Try again after restarting PeakTweaks. If it keeps failing, the same setting can be changed in ${panel}.`,
    };
  }
  const service = /^service (.+)$/.exec(what);
  if (service) return { title: `Windows did not change the service ${service[1]}.`, hint: timedOut ? busy : null };
  const task = /^scheduled task (.+)$/.exec(what);
  if (task) return { title: `Windows did not change the scheduled task ${task[1]}.`, hint: timedOut ? busy : null };
  if (what === "Proof store") return { title: "PeakTweaks could not save the test run.", hint: "Check free disk space, then try again." };
  if (what === "Record while I play") {
    return {
      title: "PeakTweaks cannot record this comparison while you play.",
      hint: d.includes("already has")
        ? "That side has its runs. Start a new comparison to record more."
        : "Start a new comparison and pick the game from the list, so PeakTweaks knows which program to record.",
    };
  }
  if (/presentmon|capture|proof run|frame statistics/i.test(what)) {
    return {
      title: "The test run did not finish.",
      hint: d.includes("wrote no data")
        ? "No frames were recorded. Keep the game open and in focus for the whole run, then try again."
        : timedOut
          ? busy
          : null,
    };
  }
  if (what === "Steam") {
    return {
      title: "PeakTweaks could not ask Steam to start the game.",
      hint: "Start it from Steam instead. PeakTweaks only starts games through Steam, never with its own administrator rights.",
    };
  }
  if (what === "Driver page") {
    return {
      title: "PeakTweaks could not open the driver page.",
      hint: "Open your browser and go to the card maker's website instead.",
    };
  }
  if (what === "NVIDIA driver install") {
    if (/signed|signature/.test(d) || d.includes("not a program") || d.includes("not a file")) {
      return {
        title: "PeakTweaks did not install that file.",
        hint: "Choose a driver downloaded from NVIDIA's own website. PeakTweaks only installs files NVIDIA signed.",
      };
    }
    if (timedOut) {
      return {
        title: "NVIDIA's installer ran for an hour and was stopped.",
        hint: "Restart Windows. If the driver is not right afterwards, System Restore puts the old one back.",
      };
    }
    return {
      title: "NVIDIA's installer did not finish.",
      hint: "Restart Windows, then try again. You can also run the file yourself and choose Custom, then Perform a clean installation.",
    };
  }
  if (what === "PowerShell") return { title: "A Windows tool did not finish.", hint: timedOut ? busy : null };
  return { title: `${what} did not finish.`, hint: timedOut ? busy : null };
}

// Win32 codes worth a plain sentence (winerror.h); others stay generic.
function win32Text(code: number): Omit<ErrorText, "detail"> {
  switch (code) {
    case 5:
      return { title: "Windows refused access.", hint: "Another program may be guarding this setting. Try again, or restart Windows first." };
    case 32:
    case 33:
      return { title: "Another program is using a file PeakTweaks needs.", hint: "Close other programs, then try again." };
    case 112:
      return { title: "The disk is full.", hint: "Free some disk space, then try again." };
    case 1460:
      return { title: "Windows took too long to answer.", hint: "Try again in a moment." };
    default:
      return { title: "A Windows call failed.", hint: null };
  }
}

export function explainUnknown(e: unknown): ErrorText {
  return explain(toEngineError(e));
}
