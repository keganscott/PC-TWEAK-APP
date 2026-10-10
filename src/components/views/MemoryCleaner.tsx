import { MemoryStick, Sparkles } from "lucide-react";
import { useId, useState } from "react";

import type { MemoryUse } from "../../generated/MemoryUse";
import { explain } from "../../lib/errors";
import { formatDateTime, formatNumber } from "../../lib/format";
import { useActions, useLiveReadings, useStore, useTechnical } from "../../store/hooks";
import { Button, cx, ErrorCallout, SampleBadge, StatusBadge } from "../ui/primitives";
import { SwitchRow } from "./PlaySection";

/** Bytes as "4.2 GB". */
export const gb = (bytes: number) => `${formatNumber(bytes / 1024 ** 3)} GB`;

/** Share of memory in use, 0-100. */
export function inUsePercent(m: MemoryUse): number {
  if (m.totalBytes <= 0) return 0;
  return Math.min(100, Math.max(0, Math.round(((m.totalBytes - m.availableBytes) / m.totalBytes) * 100)));
}

/** Empty the standby list, then read memory again so the numbers move at once. */
export function useCleanMemory() {
  const op = useStore((s) => s.standbyOp);
  const capturing = useStore((s) => s.proof.capturingSession !== null);
  const { purgeStandby, readLive } = useActions();
  return {
    op,
    capturing,
    clean: async () => {
      await purgeStandby();
      await readLive();
    },
  };
}

/** The ring: memory in use, from the live reading. */
function Gauge({ percent }: { percent: number | null }) {
  const r = 42;
  const length = 2 * Math.PI * r;
  const shown = percent ?? 0;
  return (
    <div className="relative size-28 shrink-0">
      <svg viewBox="0 0 100 100" className="size-full -rotate-90" aria-hidden>
        <circle cx="50" cy="50" r={r} fill="none" strokeWidth="9" className="stroke-surface-3" />
        <circle
          cx="50"
          cy="50"
          r={r}
          fill="none"
          strokeWidth="9"
          strokeLinecap="round"
          strokeDasharray={length}
          strokeDashoffset={length * (1 - shown / 100)}
          className={cx("transition-[stroke-dashoffset] duration-700", shown >= 90 ? "stroke-bad" : "stroke-violet-soft")}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className="font-display text-2xl font-extrabold tabular-nums">{percent === null ? "–" : `${percent}%`}</span>
        <span className="text-[10px] font-bold tracking-wider text-ink-faint uppercase">in use</span>
      </div>
    </div>
  );
}

/**
 * Catalogue E6 as a memory panel (Kegan, 2026-10-10: "a build in live memory
 * reduction tool like an app called Mem Reduct"): live memory readings and one
 * Clean memory button. Cleaning empties Windows' standby list only
 * (`memory.rs`; trimming every program's working set reaches into games, plan
 * section 12, so it is left out). Changes no setting: no restore point, nothing
 * to undo, and the change record keeps a line. Refused while Proof records.
 */
export function MemoryCleaner() {
  const live = useStore((s) => s.live);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { op, capturing, clean } = useCleanMemory();
  const headingId = useId();
  useLiveReadings();

  const memory = live?.memory.state === "yes" ? live.memory.value : null;
  const percent = memory ? inUsePercent(memory) : null;
  const result = op.status === "done" ? op.value : null;
  const freed = result ? Math.max(0, result.before.cachedBytes - result.after.cachedBytes) : 0;

  return (
    <section
      aria-labelledby={headingId}
      className="relative isolate overflow-hidden rounded-2xl border border-line bg-surface-1 p-5"
    >
      <div
        aria-hidden
        className="pointer-events-none absolute -top-16 -right-16 -z-10 size-56 rounded-full bg-violet/20 blur-3xl"
      />
      {result && (
        <span
          key={result.unixMs}
          aria-hidden
          className="pointer-events-none absolute inset-y-0 left-0 -z-10 w-1/3 bg-linear-to-r from-transparent via-lime/15 to-transparent motion-safe:animate-sweep-once"
        />
      )}
      <div className="flex flex-wrap items-center gap-2">
        <MemoryStick aria-hidden className="size-4 text-lime" strokeWidth={2} />
        <h3 id={headingId} className="font-display text-lg font-extrabold tracking-tight">
          Clean memory
        </h3>
        {sample && <SampleBadge />}
      </div>
      <div className="mt-4 flex flex-wrap items-center gap-6">
        <Gauge percent={percent} />
        {memory ? (
          <dl className="grid min-w-48 flex-1 grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
            <dt className="text-ink-muted">In use</dt>
            <dd className="text-right font-bold tabular-nums">{gb(memory.totalBytes - memory.availableBytes)}</dd>
            <dt className="text-ink-muted">Available</dt>
            <dd className="text-right font-bold tabular-nums">{gb(memory.availableBytes)}</dd>
            <dt className="text-ink-muted">Files kept in memory</dt>
            <dd className="text-right font-bold text-lime tabular-nums">{gb(memory.cachedBytes)}</dd>
            <dt className="text-ink-muted">Total</dt>
            <dd className="text-right font-bold tabular-nums">{gb(memory.totalBytes)}</dd>
          </dl>
        ) : (
          <p className="min-w-48 flex-1 text-sm text-ink-muted">
            {live && live.memory.state !== "yes" ? `Not read: ${live.memory.reason}` : "Reading memory…"}
          </p>
        )}
        <div className="flex flex-col items-start gap-2">
          <Button
            variant="go"
            busy={op.status === "running"}
            disabled={capturing}
            onClick={() => void clean()}
            icon={<Sparkles aria-hidden className="size-4" />}
          >
            Clean memory
          </Button>
          <span className="text-xs text-ink-faint">Read from Windows every few seconds</span>
        </div>
      </div>
      <p className="mt-4 text-sm text-ink-muted">
        Windows keeps files it read recently in memory that nothing else is using, and hands that memory to a program as soon as
        it asks. Cleaning empties that list now; Windows fills it again as files are read. Windows already counts those files as
        available, so "In use" hardly moves.
      </p>
      {capturing && <p className="mt-2 text-sm text-ink-muted">Available again when the Proof recording finishes.</p>}
      {result && (
        <p className="mt-2 text-sm" role="status">
          Cleaned {formatDateTime(result.unixMs)}: {gb(freed)} of files let go. Files kept in memory:{" "}
          {gb(result.before.cachedBytes)} before, {gb(result.after.cachedBytes)} after.
        </p>
      )}
      {op.status === "failed" && (
        <div className="mt-3">
          <ErrorCallout text={explain(op.error)} technical={technical} />
        </div>
      )}
      <AutoClean />
    </section>
  );
}

/** "Clean memory during games" (Kegan, 2026-10-10: Mem Reduct's automatic
 * cleaning). The game watcher cleans by itself while a known game runs, on
 * the engine's limits (`memory::auto_clean_due`); each game's report counts
 * the cleans. A preference, saved like the other while-you-play switches. */
function AutoClean() {
  const settings = useStore((s) => s.settings);
  const settingsOp = useStore((s) => s.settingsOp);
  const play = useStore((s) => s.play);
  const technical = useTechnical();
  const { saveSettings } = useActions();
  const [saving, setSaving] = useState(false);
  const [failedHere, setFailedHere] = useState(false);
  if (!settings) return null;
  const on = settings.memoryAutoClean;
  const flip = async (next: boolean) => {
    setSaving(true);
    setFailedHere(false);
    setFailedHere(!(await saveSettings({ ...settings, memoryAutoClean: next }, settings)));
    setSaving(false);
  };
  const cleans = play?.memoryCleans ?? 0;
  return (
    <div className="mt-4 border-t border-line pt-4">
      <SwitchRow
        label="Clean memory during games"
        checked={on}
        disabled={settingsOp.status === "running"}
        busy={saving}
        onChange={(next) => void flip(next)}
        badge={on && play?.game ? <StatusBadge tone="ok">On now</StatusBadge> : null}
      >
        While a game runs, empties the standby list as the button does, whenever less than 1 GB is free (Available less
        Files kept in memory) and Windows keeps more than 1 GB of files in memory: at most once a minute, and not while
        Proof records. Files the game read recently
        are let go too, so it may read some of them from the drive again. Each game's report in While you play says how
        often it cleaned.
        {on && play?.game && cleans > 0 && ` Cleaned ${cleans === 1 ? "once" : `${cleans.toLocaleString()} times`} so far in this game.`}
      </SwitchRow>
      {failedHere && settingsOp.status === "failed" && (
        <div className="mt-3">
          <ErrorCallout text={explain(settingsOp.error)} technical={technical} />
        </div>
      )}
    </div>
  );
}
