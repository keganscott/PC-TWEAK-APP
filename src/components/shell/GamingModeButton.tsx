import { Gamepad2, Loader2, Power } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";

import { explain } from "../../lib/errors";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { cx, ErrorCallout, StatusBadge } from "../ui/primitives";
import { RestorePointButton } from "./RestorePointButton";

/**
 * Gaming Mode as one click, at the top of every page and on Home (Kegan,
 * 2026-10-10: "an easy click button at the top and/or home page. It should
 * look visible and have a cool animation when selected"). The same setting as
 * the switch in Tools and the tray: saved through `saveSettings`, and the
 * engine makes its changes only while a known game runs, behind the restore
 * gate like every change.
 */
function useGamingMode() {
  const settings = useStore((s) => s.settings);
  const settingsOp = useStore((s) => s.settingsOp);
  const play = useStore((s) => s.play);
  const games = useStore((s) => s.games);
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const { saveSettings } = useActions();
  const [saving, setSaving] = useState(false);
  const [failedHere, setFailedHere] = useState(false);

  const on = settings?.gamingMode ?? false;
  const toggle = async () => {
    if (!settings) return;
    setSaving(true);
    setFailedHere(false);
    setFailedHere(!(await saveSettings({ ...settings, gamingMode: !on }, settings)));
    setSaving(false);
  };
  const gameName = play?.game ? (games.find((g) => g.id === play.game)?.name ?? play.game) : null;
  return {
    on,
    /** Its changes are in effect for a running game. */
    active: on && (play?.gamingModeActive ?? false),
    gameName,
    problem: on ? (play?.problem ?? null) : null,
    gateOpen,
    ready: !!settings,
    saving,
    disabled: !settings || settingsOp.status === "running",
    error: failedHere && settingsOp.status === "failed" ? settingsOp.error : null,
    toggle,
  };
}

/** Counts each time Gaming Mode comes on (from here, Tools or the tray), so
 * the burst plays once per switch-on and not when a page first shows it. */
function useBurst(on: boolean): number {
  const [burst, setBurst] = useState(0);
  const was = useRef(on);
  useEffect(() => {
    if (on && !was.current) setBurst((n) => n + 1);
    was.current = on;
  }, [on]);
  return burst;
}

/** The one-line state, shared by both buttons. */
function sentence(m: ReturnType<typeof useGamingMode>): string {
  if (!m.on) {
    return "Off. Turn it on and pop-up notifications go off and Windows Search indexing pauses while a game runs.";
  }
  if (m.gateOpen === false) return "On, and waiting for a restore point: it changes nothing until there is one.";
  if (m.active && m.gameName) {
    return `On now for ${m.gameName}. Pop-up notifications are off and Windows Search indexing is paused until it closes.`;
  }
  if (m.problem) return `On, but not in effect: ${m.problem}`;
  return "On. It waits for a game: while one runs, pop-up notifications go off and Windows Search indexing pauses. Both come back when it closes.";
}

/** The pill in the top bar. */
export function GamingModeToggle() {
  const m = useGamingMode();
  const burst = useBurst(m.on);
  if (!m.ready) return null;
  return (
    <div className="flex items-center gap-2">
      {m.error && <StatusBadge tone="bad">Not saved</StatusBadge>}
      <button
        type="button"
        onClick={() => void m.toggle()}
        aria-pressed={m.on}
        aria-busy={m.saving}
        disabled={m.disabled}
        title={sentence(m)}
        className={cx(
          "relative isolate flex items-center gap-2 overflow-hidden rounded-full border py-1.5 pr-1.5 pl-3 text-xs font-bold transition-colors disabled:cursor-not-allowed",
          m.on
            ? "border-lime bg-surface-2 text-ink motion-safe:animate-gm-glow"
            : "border-line-strong bg-surface-1 text-ink-muted hover:border-violet hover:text-ink",
        )}
      >
        {m.active && (
          <span
            aria-hidden
            className="pointer-events-none absolute inset-y-0 left-0 -z-10 w-1/3 bg-linear-to-r from-transparent via-lime/25 to-transparent motion-safe:animate-gm-sheen"
          />
        )}
        {m.saving ? (
          <Loader2 aria-hidden className="size-4 animate-spin" />
        ) : (
          <Gamepad2 aria-hidden className={cx("size-4", m.on && "text-lime")} />
        )}
        <span>Gaming Mode</span>
        <span
          aria-hidden
          className={cx(
            "relative rounded-full px-2 py-0.5 text-[10px] tracking-wider uppercase",
            m.on ? "bg-lime text-black" : "bg-surface-3 text-ink-muted",
          )}
        >
          {burst > 0 && m.on && (
            <span key={burst} className="absolute inset-0 rounded-full bg-lime motion-safe:animate-gm-burst" />
          )}
          <span className="relative">{m.active ? "On now" : m.on ? "On" : "Off"}</span>
        </span>
      </button>
    </div>
  );
}

/** The big switch on Home. */
export function GamingModeCard() {
  const m = useGamingMode();
  const burst = useBurst(m.on);
  const technical = useTechnical();
  const titleId = useId();
  const textId = useId();
  if (!m.ready) return null;
  return (
    <section
      aria-labelledby={titleId}
      className={cx(
        "relative isolate overflow-hidden rounded-2xl border p-5 transition-colors print:hidden",
        m.on ? "border-lime/70 bg-surface-1" : "border-line bg-surface-1",
      )}
    >
      {m.on && (
        <div
          aria-hidden
          className="pointer-events-none absolute inset-0 -z-10 bg-[radial-gradient(ellipse_at_left,rgb(126_59_237/0.35),transparent_60%)]"
        />
      )}
      {m.active && (
        <span
          aria-hidden
          className="pointer-events-none absolute inset-y-0 left-0 -z-10 w-1/4 bg-linear-to-r from-transparent via-lime/10 to-transparent motion-safe:animate-gm-sheen"
        />
      )}
      <div className="flex flex-wrap items-center gap-5">
        <div className="relative size-20 shrink-0">
          {m.on && (
            <span aria-hidden className="absolute -inset-1 overflow-hidden rounded-full">
              <span className="absolute -inset-1/2 bg-[conic-gradient(from_0deg,var(--color-lime),var(--color-violet),transparent_55%,var(--color-lime))] motion-safe:animate-gm-spin" />
            </span>
          )}
          {burst > 0 && m.on && (
            <span
              key={burst}
              aria-hidden
              className="absolute inset-0 rounded-full border-4 border-lime motion-safe:animate-gm-burst"
            />
          )}
          <button
            type="button"
            onClick={() => void m.toggle()}
            aria-pressed={m.on}
            aria-busy={m.saving}
            aria-labelledby={titleId}
            aria-describedby={textId}
            disabled={m.disabled}
            className={cx(
              "relative flex size-20 items-center justify-center rounded-full border-2 transition-colors disabled:cursor-not-allowed",
              m.on
                ? "border-surface-1 bg-lime text-black motion-safe:animate-gm-glow"
                : "border-line-strong bg-surface-2 text-ink-muted hover:border-violet hover:text-ink",
            )}
          >
            {m.saving ? (
              <Loader2 aria-hidden className="size-8 animate-spin" />
            ) : (
              <Power aria-hidden className="size-8" strokeWidth={2.5} />
            )}
          </button>
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id={titleId} className="font-display text-xl font-extrabold tracking-tight">
              Gaming Mode
            </h2>
            {m.active ? (
              <StatusBadge tone="ok">On now</StatusBadge>
            ) : m.on ? (
              <StatusBadge tone="ok">On</StatusBadge>
            ) : (
              <span className="rounded-md border border-line-strong px-2 py-0.5 text-xs font-bold text-ink-muted">Off</span>
            )}
          </div>
          <p id={textId} className="mt-1 text-sm text-ink-muted">
            {sentence(m)}
          </p>
          {m.on && m.gateOpen === false && <RestorePointButton compact variant="secondary" className="mt-3" />}
        </div>
      </div>
      {m.error && (
        <div className="mt-4">
          <ErrorCallout text={explain(m.error)} technical={technical} />
        </div>
      )}
    </section>
  );
}
