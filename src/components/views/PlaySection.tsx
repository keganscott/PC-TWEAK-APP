import { useId, useState, type ReactNode } from "react";

import type { PlayReport } from "../../generated/PlayReport";
import type { Settings } from "../../generated/Settings";
import { explain } from "../../lib/errors";
import { formatDateTime, formatDuration } from "../../lib/format";
import { reportNotes, reportSummary, sameReport } from "../../lib/playReport";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Callout, Card, ErrorCallout, SampleBadge, StatusBadge } from "../ui/primitives";

/** Only Minecraft's Bedrock Edition is recognised (`GAME_PROCESSES` in the
 * engine's `play.rs`): Java Edition's program name is shared by other programs. */
const EDITION: Readonly<Record<string, string>> = { minecraft: "Minecraft (Bedrock Edition)" };

/** "A, B and C". */
export function listWords(words: readonly string[]): string {
  if (words.length < 2) return words.join("");
  return `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`;
}

/** A timer interval in 100 ns units, as "0.5 ms". */
const ms = (units: number) => `${(units / 10_000).toLocaleString(undefined, { maximumFractionDigits: 2 })} ms`;

type Switch = "gamingMode" | "gameTimer";

/** Catalogue step 5: Gaming Mode (H5, H15) and the game timer (H2). The engine
 * makes these changes only while a known game runs and puts them back when it
 * closes; this section is the two switches and what the watcher sees. */
export function PlaySection() {
  const play = useStore((s) => s.play);
  const games = useStore((s) => s.games);
  const settings = useStore((s) => s.settings);
  const settingsOp = useStore((s) => s.settingsOp);
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { saveSettings } = useActions();
  const [saving, setSaving] = useState<Switch | null>(null);
  const [failedHere, setFailedHere] = useState(false);

  const name = (id: string) => games.find((g) => g.id === id)?.name ?? id;
  // This run's last game, or after a restart the newest one kept.
  const history = play?.history ?? [];
  const shownLast = play?.lastSession ?? history.at(-1) ?? null;
  const earlier = history
    .filter((r) => !shownLast || !sameReport(r, shownLast))
    .reverse()
    .slice(0, EARLIER_SHOWN);
  const flip = async (key: Switch, on: boolean) => {
    if (!settings) return;
    setSaving(key);
    setFailedHere(false);
    const next: Settings = { ...settings, [key]: on };
    setFailedHere(!(await saveSettings(next, settings)));
    setSaving(null);
  };
  const busy = settingsOp.status === "running";

  return (
    <section aria-labelledby="cat-play">
      <div className="mb-3">
        <div className="flex flex-wrap items-center gap-2">
          <h2 id="cat-play" className="text-base font-extrabold tracking-tight">
            While you play
          </h2>
          {sample && <SampleBadge />}
        </div>
        <p className="mt-1 text-sm text-ink-muted">
          PeakTweaks looks for a known game every few seconds. The changes below are made only while one runs and are
          put back when it closes.
        </p>
      </div>
      <Card className="flex flex-col gap-4 p-4">
        {play && (
          <p className="text-sm" role="status">
            {play.game ? (
              <>
                <span className="font-bold">{name(play.game)} is running.</span>
                {play.gamingModeActive && " Gaming Mode is on."}
                {play.timerHeld !== null && ` The game timer is held at ${ms(play.timerHeld)}.`}
              </>
            ) : (
              <>Watching for {listWords(play.watched.map((id) => EDITION[id] ?? name(id)))}.</>
            )}
          </p>
        )}
        {play?.game && play.onWifi && (
          <Callout tone="info" title="This game is running over Wi-Fi.">
            Wi-Fi shares the air with other devices and walls get in its way, so it can lose packets where a network cable
            does not. If a cable can reach this PC, plugging it in takes that out of the picture.
          </Callout>
        )}
        {play?.problem && (
          <Callout tone="warn" title="Not everything you turned on is in effect.">
            {play.problem}
          </Callout>
        )}
        <SwitchRow
          label="Gaming Mode"
          checked={settings?.gamingMode ?? false}
          disabled={!settings || busy}
          busy={saving === "gamingMode"}
          onChange={(on) => void flip("gamingMode", on)}
          badge={play?.gamingModeActive ? <StatusBadge tone="ok">On now</StatusBadge> : null}
        >
          Turns off pop-up notifications and pauses Windows Search indexing while a game runs. Both come back when it
          closes, and Backups lists them while they are in effect.
          {gateOpen === false && " Like every change, it needs a restore point first."}
        </SwitchRow>
        <SwitchRow
          label="Game timer"
          checked={settings?.gameTimer ?? false}
          disabled={!settings || busy}
          busy={saving === "gameTimer"}
          onChange={(on) => void flip("gameTimer", on)}
          badge={play?.timerHeld != null ? <StatusBadge tone="ok">On now</StatusBadge> : null}
        >
          Asks Windows for its finest timer while a game runs and lets it go when the game closes. On Windows 11 the
          request reaches the game only with System-wide timer requests turned on (Tools, Advanced). Uses more battery
          on a laptop while a game runs.
        </SwitchRow>
        {failedHere && settingsOp.status === "failed" && <ErrorCallout text={explain(settingsOp.error)} technical={technical} />}
        {shownLast && <LastSession report={shownLast} name={name(shownLast.game)} />}
        {earlier.length > 0 && <EarlierGames reports={earlier} name={name} />}
      </Card>
    </section>
  );
}

/** What the graphics card did during the last game (plan 6.2 item 6,
 * advice only). Read while the game ran; nothing was changed by it. */
/** How many earlier games are listed under the last one. */
const EARLIER_SHOWN = 5;

/** The games before the last one, newest first, one line each. */
function EarlierGames({ reports, name }: { reports: readonly PlayReport[]; name: (id: string) => string }) {
  const headingId = useId();
  return (
    <div className="border-t border-line pt-4" role="group" aria-labelledby={headingId}>
      <h3 id={headingId} className="font-bold">
        Earlier games
      </h3>
      <ul className="mt-2 flex flex-col gap-1.5 text-sm">
        {reports.map((r) => {
          const hottest = r.gpuHottestC;
          return (
            <li key={`${r.game}-${r.startedUnixMs}`}>
              <span className="font-semibold">{name(r.game)}</span>
              <span className="text-ink-muted">
                , {formatDateTime(r.endedUnixMs)}, watched for {formatDuration((r.endedUnixMs - r.startedUnixMs) / 1000)}
                {hottest.state === "yes" && `, hottest ${hottest.value} °C`}. {reportSummary(r)}
              </span>
            </li>
          );
        })}
      </ul>
      <p className="mt-2 text-xs text-ink-faint">The last few games are kept on this PC only.</p>
    </div>
  );
}

function LastSession({ report, name }: { report: PlayReport; name: string }) {
  const headingId = useId();
  const seconds = (report.endedUnixMs - report.startedUnixMs) / 1000;
  const hottest = report.gpuHottestC;
  return (
    <div className="border-t border-line pt-4" role="group" aria-labelledby={headingId}>
      <h3 id={headingId} className="font-bold">
        Last game: {name}
      </h3>
      <p className="mt-1 text-sm text-ink-muted">
        PeakTweaks watched it for {formatDuration(seconds)}, until {formatDateTime(report.endedUnixMs)}, reading the
        graphics card every few seconds and changing nothing.
        {hottest.state === "yes" && ` Hottest reading: ${hottest.value} °C.`}
        {hottest.state === "yes" && report.temperatureMissed && " Some readings had no temperature, so a hotter moment may be missing."}
      </p>
      <div className="mt-3 flex flex-col gap-2">
        {reportNotes(report).map((n) => (
          <Callout key={n.title} tone={n.tone} title={n.title}>
            {n.text}
          </Callout>
        ))}
      </div>
    </div>
  );
}

function SwitchRow({
  label,
  checked,
  disabled,
  busy,
  onChange,
  badge,
  children,
}: {
  label: string;
  checked: boolean;
  disabled: boolean;
  busy: boolean;
  onChange: (on: boolean) => void;
  badge: ReactNode;
  children: ReactNode;
}) {
  const id = useId();
  const hintId = useId();
  return (
    <div className="flex items-start gap-3">
      <input
        id={id}
        type="checkbox"
        role="switch"
        aria-checked={checked}
        aria-describedby={hintId}
        aria-busy={busy}
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-1 size-4 shrink-0 accent-accent"
      />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <label htmlFor={id} className="cursor-pointer font-bold">
            {label}
          </label>
          {badge}
        </div>
        <p id={hintId} className="mt-1 text-sm text-ink-muted">
          {children}
        </p>
      </div>
    </div>
  );
}
