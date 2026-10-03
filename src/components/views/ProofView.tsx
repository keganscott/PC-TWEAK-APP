import { useEffect, useId, useState, type FormEvent } from "react";
import { Play, Plus, Scale } from "lucide-react";

import type { Comparison } from "../../generated/Comparison";
import type { ProofRun } from "../../generated/ProofRun";
import type { ProofSessionSummary } from "../../generated/ProofSessionSummary";
import type { Side } from "../../generated/Side";
import { explain } from "../../lib/errors";
import { formatDateTime, formatNumber } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Callout, Card, ErrorCallout, PageHeader, SampleBadge, Spinner, cx } from "../ui/primitives";

/** The engine's limits (proof/capture.rs `validate_timing`). */
const SECONDS = { min: 10, max: 600, default: 30 };
const DELAY = { min: 0, max: 120, default: 5 };

export function ProofView() {
  const sessions = useStore((s) => s.proof.sessions);
  const loadError = useStore((s) => s.proof.loadError);
  const technical = useTechnical();
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const { loadSessions } = useActions();

  useEffect(() => {
    void loadSessions();
  }, [loadSessions]);

  const current = sessions.find((s) => s.session.sessionId === selected) ?? null;

  return (
    <>
      <PageHeader
        title="Proof"
        description="Record the same scene before and after a change, a few times each. PeakTweaks only calls a difference real when it is bigger than the variation between your own runs."
        actions={
          <Button variant="primary" icon={<Plus aria-hidden className="size-4" />} onClick={() => setCreating(true)}>
            New comparison
          </Button>
        }
      />
      {loadError && (
        <div className="mb-4 max-w-4xl">
          <ErrorCallout text={explain(loadError)} technical={technical} />
        </div>
      )}
      <div className="grid max-w-6xl gap-5 lg:grid-cols-[18rem_1fr]">
        <SessionList sessions={sessions} selected={selected} onSelect={(id) => (setSelected(id), setCreating(false))} />
        <div>
          {creating ? (
            <NewSession
              onCreated={(id) => {
                setCreating(false);
                setSelected(id);
              }}
              onCancel={() => setCreating(false)}
            />
          ) : current ? (
            <SessionDetail summary={current} />
          ) : (
            <Card>
              <p className="text-sm text-ink-muted">Pick a comparison on the left, or start a new one.</p>
            </Card>
          )}
        </div>
      </div>
    </>
  );
}

function SessionList({ sessions, selected, onSelect }: { sessions: ProofSessionSummary[]; selected: string | null; onSelect: (id: string) => void }) {
  if (sessions.length === 0) {
    return (
      <Card>
        <p className="text-sm text-ink-muted">No comparisons yet.</p>
      </Card>
    );
  }
  return (
    <nav aria-label="Comparisons">
      <ul className="flex flex-col gap-2">
        {sessions.map(({ session, beforeRuns, afterRuns }) => (
          <li key={session.sessionId}>
            <button
              type="button"
              onClick={() => onSelect(session.sessionId)}
              aria-current={selected === session.sessionId ? "true" : undefined}
              className={cx(
                "w-full rounded-lg border p-3 text-left text-sm",
                selected === session.sessionId ? "border-accent bg-accent/10" : "border-line bg-surface-1 hover:bg-surface-2",
              )}
            >
              <span className="block font-medium">{session.exe}</span>
              <span className="block text-xs text-ink-faint">{formatDateTime(session.createdUnixMs)}</span>
              <span className="mt-1 block text-xs text-ink-muted">
                {beforeRuns} before · {afterRuns} after
              </span>
            </button>
          </li>
        ))}
      </ul>
    </nav>
  );
}

function NewSession({ onCreated, onCancel }: { onCreated: (id: string) => void; onCancel: () => void }) {
  const games = useStore((s) => s.games);
  const beginOp = useStore((s) => s.proof.beginOp);
  const technical = useTechnical();
  const { beginSession } = useActions();
  const installs = useStore((s) => s.audit?.env.gameInstalls ?? null);
  const [exe, setExe] = useState("");
  const [gameId, setGameId] = useState<string>("");
  // The program name filled in from a game found on this PC, so picking
  // another game replaces it but never something the user typed.
  const [filled, setFilled] = useState<string | null>(null);
  const found = installs?.find((i) => i.gameId === gameId && i.exe) ?? null;

  const pickGame = (id: string) => {
    setGameId(id);
    const path = installs?.find((i) => i.gameId === id)?.exe ?? null;
    const name = path ? path.slice(path.lastIndexOf("\\") + 1) : null;
    if (exe.trim() === "" || exe === filled) {
      setExe(name ?? "");
      setFilled(name);
    }
  };
  const [build, setBuild] = useState("");
  const ids = { exe: useId(), game: useId(), build: useId() };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const session = await beginSession(exe.trim(), gameId || null, build.trim() || null);
    if (session) onCreated(session.sessionId);
  };

  return (
    <Card>
      <h2 className="text-lg font-semibold">New comparison</h2>
      <form className="mt-4 flex flex-col gap-4" onSubmit={(e) => void submit(e)}>
        <div>
          <label htmlFor={ids.game} className="block text-sm font-medium">
            Game (optional)
          </label>
          <select
            id={ids.game}
            value={gameId}
            onChange={(e) => pickGame(e.target.value)}
            className="mt-1 w-full rounded-md border border-line bg-surface-0 px-3 py-2 text-sm"
          >
            <option value="">Not listed</option>
            {games.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name}
              </option>
            ))}
          </select>
          {found?.exe && (
            <p className="mt-1 text-xs text-ink-faint wrap-anywhere">Found on this PC: {found.exe}</p>
          )}
        </div>
        <div>
          <label htmlFor={ids.exe} className="block text-sm font-medium">
            Game program name
          </label>
          <input
            id={ids.exe}
            required
            value={exe}
            onChange={(e) => setExe(e.target.value)}
            placeholder="FortniteClient-Win64-Shipping.exe"
            className="mt-1 w-full rounded-md border border-line bg-surface-0 px-3 py-2 text-sm"
            aria-describedby={`${ids.exe}-hint`}
          />
          <p id={`${ids.exe}-hint`} className="mt-1 text-xs text-ink-faint">
            The .exe name as Task Manager shows it under Details.
          </p>
        </div>
        <div>
          <label htmlFor={ids.build} className="block text-sm font-medium">
            Game version or patch (optional)
          </label>
          <input
            id={ids.build}
            value={build}
            onChange={(e) => setBuild(e.target.value)}
            className="mt-1 w-full rounded-md border border-line bg-surface-0 px-3 py-2 text-sm"
          />
        </div>
        {beginOp.status === "failed" && <ErrorCallout text={explain(beginOp.error)} technical={technical} />}
        <div className="flex gap-2">
          <Button type="submit" variant="primary" busy={beginOp.status === "running"}>
            Start
          </Button>
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </form>
    </Card>
  );
}

function SessionDetail({ summary }: { summary: ProofSessionSummary }) {
  const { session } = summary;
  const runs = useStore((s) => s.proof.runs[session.sessionId]);
  const captureOp = useStore((s) => s.proof.captureOps[session.sessionId]);
  const capturingSession = useStore((s) => s.proof.capturingSession);
  const comparison = useStore((s) => s.proof.comparisons[session.sessionId]);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { loadRuns, capture, compare } = useActions();
  const [side, setSide] = useState<Side>("before");
  const [seconds, setSeconds] = useState(SECONDS.default);
  const [delay, setDelay] = useState(DELAY.default);
  const ids = { seconds: useId(), delay: useId() };

  useEffect(() => {
    void loadRuns(session.sessionId);
  }, [loadRuns, session.sessionId]);

  const before = (runs ?? []).filter((r) => r.side === "before");
  const after = (runs ?? []).filter((r) => r.side === "after");
  const capturing = captureOp?.status === "running";
  const otherRecording = capturingSession !== null && capturingSession !== session.sessionId;

  return (
    <div className="flex flex-col gap-5">
      <Card>
        <div className="flex flex-wrap items-center gap-2">
          <h2 className="text-lg font-semibold">{session.exe}</h2>
          {sample && <SampleBadge />}
        </div>
        <p className="mt-1 text-xs text-ink-faint">
          Started {formatDateTime(session.createdUnixMs)} · PresentMon {session.toolVersion}
          {session.gameBuild && ` · version ${session.gameBuild}`}
        </p>

        <form
          className="mt-5 grid gap-4 sm:grid-cols-[auto_auto_auto_1fr] sm:items-end"
          onSubmit={(e) => {
            e.preventDefault();
            void capture(session.sessionId, side, seconds, delay);
          }}
        >
          <fieldset>
            <legend className="text-sm font-medium">Run</legend>
            <div className="mt-1 flex gap-1">
              {(["before", "after"] as const).map((s) => (
                <label
                  key={s}
                  className={cx(
                    "cursor-pointer rounded-md border px-3 py-1.5 text-sm",
                    side === s ? "border-accent bg-accent/10" : "border-line text-ink-muted",
                  )}
                >
                  <input type="radio" name="side" className="sr-only" checked={side === s} onChange={() => setSide(s)} />
                  {s === "before" ? "Before the change" : "After the change"}
                </label>
              ))}
            </div>
          </fieldset>
          <div>
            <label htmlFor={ids.seconds} className="block text-sm font-medium">
              Seconds
            </label>
            <input
              id={ids.seconds}
              type="number"
              min={SECONDS.min}
              max={SECONDS.max}
              value={seconds}
              onChange={(e) => setSeconds(Number(e.target.value))}
              className="mt-1 w-24 rounded-md border border-line bg-surface-0 px-3 py-1.5 text-sm"
            />
          </div>
          <div>
            <label htmlFor={ids.delay} className="block text-sm font-medium">
              Start after
            </label>
            <input
              id={ids.delay}
              type="number"
              min={DELAY.min}
              max={DELAY.max}
              value={delay}
              onChange={(e) => setDelay(Number(e.target.value))}
              className="mt-1 w-24 rounded-md border border-line bg-surface-0 px-3 py-1.5 text-sm"
            />
          </div>
          <div className="flex items-center gap-3">
            <Button
              type="submit"
              variant="primary"
              busy={capturing}
              disabled={otherRecording}
              icon={<Play aria-hidden className="size-4" />}
            >
              Record
            </Button>
            {capturing && <Spinner label={`Recording for ${delay + seconds} s. Keep the game in front.`} />}
            {otherRecording && <span className="text-sm text-ink-muted">Another comparison is recording.</span>}
          </div>
        </form>
        <p className="mt-3 text-xs text-ink-faint">
          Get the game to the same place each time, press Record, then switch back to it before the countdown ends.
          Three runs on each side give a fair picture.
        </p>
        {captureOp?.status === "failed" && (
          <div className="mt-3">
            <ErrorCallout text={explain(captureOp.error)} technical={technical} />
          </div>
        )}
      </Card>

      <div className="grid gap-5 xl:grid-cols-2">
        <RunTable title="Before" runs={before} />
        <RunTable title="After" runs={after} />
      </div>

      <Card>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <h2 className="text-lg font-semibold">Result</h2>
          <Button
            onClick={() => void compare(session.sessionId)}
            busy={comparison?.status === "running"}
            disabled={before.length === 0 || after.length === 0}
            icon={<Scale aria-hidden className="size-4" />}
          >
            Compare
          </Button>
        </div>
        {comparison?.status === "failed" && (
          <div className="mt-3">
            <ErrorCallout text={explain(comparison.error)} technical={technical} />
          </div>
        )}
        {comparison?.status === "done" ? (
          <ComparisonResult comparison={comparison.value} />
        ) : (
          <p className="mt-2 text-sm text-ink-muted">Record at least two runs on each side, then compare.</p>
        )}
      </Card>
    </div>
  );
}

function RunTable({ title, runs }: { title: string; runs: ProofRun[] }) {
  return (
    <Card className="p-4">
      <h3 className="font-medium">{title}</h3>
      {runs.length === 0 ? (
        <p className="mt-2 text-sm text-ink-muted">No runs yet.</p>
      ) : (
        <table className="mt-3 w-full text-left text-sm">
          <thead className="text-xs text-ink-faint">
            <tr>
              <th scope="col" className="pb-2 pr-4 font-medium">
                #
              </th>
              <th scope="col" className="pb-2 pr-4 font-medium">
                {/* copy-lint-allow: column of numbers from a stored proof run */}
                {"Average FPS"}
              </th>
              <th scope="col" className="pb-2 pr-4 font-medium">
                {/* copy-lint-allow: column of numbers from a stored proof run */}
                {"1% low FPS"}
              </th>
              <th scope="col" className="pb-2 pr-4 font-medium">
                Stutters
              </th>
              <th scope="col" className="pb-2 pr-4 font-medium">
                GPU held back
              </th>
            </tr>
          </thead>
          <tbody>
            {runs.map((r) => (
              <tr key={r.runId} className="border-t border-line">
                <td className="py-1.5 pr-4">{r.index}</td>
                <td className="py-1.5 pr-4 tabular-nums">{formatNumber(r.stats.avgFps)}</td>
                <td className="py-1.5 pr-4 tabular-nums">{formatNumber(r.stats.onePercentLowFps)}</td>
                <td className="py-1.5 pr-4 tabular-nums">{r.stats.stutters}</td>
                <td className="py-1.5 pr-4">{throttleText(r)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Card>
  );
}

function throttleText(run: ProofRun): string {
  const t = run.gpuThrottle;
  if (t.state !== "yes") return "Not known";
  const limiting = t.value.seen.filter((s) =>
    ["software_power_cap", "hardware_slowdown", "software_thermal_slowdown", "hardware_thermal_slowdown", "hardware_power_brake"].includes(s.reason),
  );
  return limiting.length ? `Yes (${limiting.length} reason${limiting.length > 1 ? "s" : ""})` : "No";
}

/** The engine's headline, verbatim, then the numbers behind it. */
function ComparisonResult({ comparison }: { comparison: Comparison }) {
  const rows = [comparison.average, comparison.lows];
  return (
    <div className="mt-3 flex flex-col gap-4">
      <p className="text-base font-medium" data-testid="verdict-headline">
        {comparison.headline}
      </p>
      {comparison.warnings.length > 0 && (
        <Callout tone="warn" title="Trust these numbers less:">
          <ul className="list-disc pl-5">
            {comparison.warnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        </Callout>
      )}
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-ink-faint">
          <tr>
            <th scope="col" className="pb-2 pr-4 font-medium">
              Figure
            </th>
            <th scope="col" className="pb-2 pr-4 font-medium">
              Before (median)
            </th>
            <th scope="col" className="pb-2 pr-4 font-medium">
              After (median)
            </th>
            <th scope="col" className="pb-2 pr-4 font-medium">
              Needed to count
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((m) => (
            <tr key={m.metric} className="border-t border-line tabular-nums">
              {/* copy-lint-allow: labels for numbers from stored proof runs */}
              <td className="py-1.5 pr-4">{m.metric === "avg_fps" ? "Average FPS" : "1% low FPS"}</td>
              <td className="py-1.5 pr-4">{formatNumber(m.beforeMedian)}</td>
              <td className="py-1.5 pr-4">{formatNumber(m.afterMedian)}</td>
              <td className="py-1.5 pr-4">±{formatNumber(m.threshold)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-xs text-ink-faint">
        Runs compared: {comparison.beforeRunIds.length} before, {comparison.afterRunIds.length} after.
      </p>
    </div>
  );
}
