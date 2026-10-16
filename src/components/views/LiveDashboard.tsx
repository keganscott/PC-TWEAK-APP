import { Cpu, Gpu, MemoryStick, Sparkles, Thermometer, Timer, type LucideIcon } from "lucide-react";
import { useId, useMemo, type ReactNode } from "react";

import type { GpuLive } from "../../generated/GpuLive";
import type { LiveReadings } from "../../generated/LiveReadings";
import type { Probe } from "../../generated/Probe";
import { explain } from "../../lib/errors";
import { formatNumber } from "../../lib/format";
import { LIVE_EVERY_MS, useLiveReadings, useStore } from "../../store/hooks";
import { LIVE_HISTORY } from "../../store/store";
import { Button, cx, ErrorCallout, SampleBadge, Skeleton } from "../ui/primitives";
import { gb, inUsePercent, useCleanMemory } from "./MemoryCleaner";

/**
 * Home's live dashboard (Kegan, 2026-10-09: more and better UI sections):
 * processor, graphics card and memory, read every few seconds while Home is
 * open (`live.rs`), each with a graph of the last two minutes. Only what
 * Windows and NVIDIA's driver report; what is not read says so. Everything
 * is drawn with SVG attributes (the release CSP allows no inline styles).
 */
export function LiveDashboard() {
  const live = useStore((s) => s.live);
  const history = useStore((s) => s.liveHistory);
  const op = useStore((s) => s.liveOp);
  const sample = useStore((s) => s.sample);
  useLiveReadings();

  return (
    <section aria-labelledby="right-now-title" className="print:hidden">
      <div className="mb-2.5 flex flex-wrap items-baseline justify-between gap-3">
        <div className="flex items-center gap-2">
          <h2 id="right-now-title" className="text-[15px] font-extrabold tracking-tight">
            Right now
          </h2>
          {sample && <SampleBadge />}
        </div>
        <span className="text-xs font-semibold text-ink-faint">
          Read from Windows every {LIVE_EVERY_MS / 1000} seconds · graphs show the last {(LIVE_HISTORY * LIVE_EVERY_MS) / 60_000} minutes
        </span>
      </div>
      {!live ? (
        op.status === "failed" ? (
          <ErrorCallout text={explain(op.error)} technical={false} />
        ) : (
          <Skeleton className="h-56" label="Loading live readings" />
        )
      ) : (
        <ul className="grid gap-3.5 lg:grid-cols-3">
          <ProcessorCard live={live} history={history} />
          {live.gpus.state === "yes" ? (
            live.gpus.value.map((g, i) => <GpuCard key={i} gpu={g} index={i} history={history} />)
          ) : (
            <Card label="Graphics card" icon={Gpu}>
              <p className="mt-3 text-xs text-ink-muted">
                {live.gpus.state === "no"
                  ? "No NVIDIA graphics card to read. AMD and Intel cards are not read yet."
                  : `Not read: ${live.gpus.reason}. AMD and Intel cards are not read yet.`}
              </p>
            </Card>
          )}
          <MemoryCard live={live} history={history} />
          <TimerCard live={live} />
        </ul>
      )}
    </section>
  );
}

const yes = <T,>(p: Probe<T> | undefined): T | null => (p?.state === "yes" ? p.value : null);

function ProcessorCard({ live, history }: { live: LiveReadings; history: LiveReadings[] }) {
  const series = useMemo(() => history.map((r) => yes(r.cpuBusyPercent)), [history]);
  const mhz = yes(live.cpuMhz);
  const cores = yes(live.cpuCores);
  return (
    <Card label="Processor" icon={Cpu}>
      <div className="mt-3 flex items-center gap-4">
        <Ring label="Processor" reading={live.cpuBusyPercent} unit="busy" />
        <dl className="grid min-w-0 gap-1 text-xs">
          <Stat term="Speed" value={mhz === null ? null : `${formatNumber(mhz / 1000, 2)} GHz`} />
          <Stat term="Logical processors" value={cores === null ? null : String(cores.length)} />
        </dl>
      </div>
      <Sparkline label="Processor busy" series={series} />
      {cores ? (
        <CoreBars cores={cores} />
      ) : (
        live.cpuCores.state !== "yes" && <p className="mt-3 text-xs text-ink-muted">Each logical processor not read: {live.cpuCores.reason}.</p>
      )}
      <BoardTemperatures reading={live.thermalZones} />
    </Card>
  );
}

function GpuCard({ gpu, index, history }: { gpu: GpuLive; index: number; history: LiveReadings[] }) {
  const series = useMemo(() => history.map((r) => yes(yes(r.gpus)?.[index]?.busyPercent)), [history, index]);
  const name = yes(gpu.name) ?? `Graphics card ${index + 1}`;
  const used = yes(gpu.memoryUsedBytes);
  const total = yes(gpu.memoryTotalBytes);
  const temperature = yes(gpu.temperatureC);
  const clock = yes(gpu.graphicsClockMhz);
  const watts = yes(gpu.powerMilliwatts);
  const fan = yes(gpu.fanPercent);
  return (
    <Card label="Graphics card" icon={Gpu}>
      <p className="mt-1 truncate text-xs font-semibold text-ink-muted">{name}</p>
      <div className="mt-2 flex items-center gap-4">
        <Ring label={`Graphics card ${index + 1}`} reading={gpu.busyPercent} unit="busy" />
        <dl className="grid min-w-0 gap-1 text-xs">
          <Stat term="Temperature" value={temperature === null ? null : `${temperature} °C`} />
          <Stat term="Clock" value={clock === null ? null : `${clock.toLocaleString()} MHz`} />
          {watts !== null && <Stat term="Power" value={`${Math.round(watts / 1000)} W`} />}
          {fan !== null && <Stat term="Fan" value={`${fan}%`} />}
        </dl>
      </div>
      <Sparkline label={`${name} busy`} series={series} />
      {used !== null && total !== null && total > 0 && (
        <Bar label="Graphics memory" used={used} total={total} text={`${gb(used)} of ${gb(total)}`} />
      )}
    </Card>
  );
}

function MemoryCard({ live, history }: { live: LiveReadings; history: LiveReadings[] }) {
  const series = useMemo(() => history.map((r) => (r.memory.state === "yes" ? inUsePercent(r.memory.value) : null)), [history]);
  const memory = yes(live.memory);
  const reading: Probe<number> = memory
    ? { state: "yes", value: inUsePercent(memory) }
    : { state: "unknown", reason: live.memory.state === "yes" ? "" : live.memory.reason };
  return (
    <Card label="Memory" icon={MemoryStick}>
      <div className="mt-3 flex items-center gap-4">
        <Ring label="Memory" reading={reading} unit="in use" />
        {memory && (
          <dl className="grid min-w-0 gap-1 text-xs">
            <Stat term="In use" value={gb(memory.totalBytes - memory.availableBytes)} />
            <Stat term="Files kept" value={gb(memory.cachedBytes)} />
            <Stat term="Total" value={gb(memory.totalBytes)} />
          </dl>
        )}
      </div>
      <Sparkline label="Memory in use" series={series} />
      <div className="mt-auto pt-3">
        <CleanMemoryButton />
      </div>
    </Card>
  );
}

/**
 * Windows' timer resolution (NOTES N133): how often Windows wakes programs
 * that asked to be woken. While a known game runs, PeakTweaks holds the
 * finest (see Tools > System-wide timer requests for the Windows 11 half).
 */
function TimerCard({ live }: { live: LiveReadings }) {
  const timer = yes(live.timer);
  return (
    <Card label="Timer" icon={Timer}>
      {timer ? (
        <>
          <p className="mt-3 text-2xl font-extrabold tabular-nums">
            {timer.currentMs} <span className="text-sm font-bold text-ink-muted">ms now</span>
          </p>
          <dl className="mt-2 grid gap-1 text-xs">
            <Stat term="Finest this PC offers" value={`${timer.finestMs} ms`} />
          </dl>
          <p className="mt-3 text-xs text-ink-muted">
            How often Windows wakes programs that asked to be woken. While a game PeakTweaks knows runs, it holds the
            finest.
          </p>
        </>
      ) : (
        <p className="mt-3 text-xs text-ink-muted">
          {live.timer.state === "unknown" ? `Not read: ${live.timer.reason}.` : "Not read."}
        </p>
      )}
    </Card>
  );
}

function Card({ label, icon: Icon, children }: { label: string; icon: LucideIcon; children: ReactNode }) {
  return (
    <li className="flex min-w-0 flex-col rounded-2xl border border-line bg-surface-1 px-[18px] py-4">
      <span className="flex items-center gap-2">
        <Icon aria-hidden className="size-4 text-ink-faint" strokeWidth={1.8} />
        <span className="text-[10.5px] font-bold tracking-[0.14em] text-ink-faint uppercase">{label}</span>
      </span>
      {children}
    </li>
  );
}

function Stat({ term, value }: { term: string; value: string | null }) {
  return (
    <div className="flex items-baseline gap-2">
      <dt className="text-ink-faint">{term}</dt>
      <dd className="font-bold tabular-nums">{value ?? <span className="font-normal text-ink-muted">not read</span>}</dd>
    </div>
  );
}

/** A 0-100 reading as a ring, or why it was not read. */
function Ring({ label, reading, unit }: { label: string; reading: Probe<number>; unit: string }) {
  if (reading.state !== "yes") {
    return <p className="w-24 shrink-0 text-xs text-ink-muted">Not read: {reading.reason}</p>;
  }
  const value = Math.min(100, Math.max(0, reading.value));
  const r = 42;
  const length = 2 * Math.PI * r;
  return (
    <div
      role="meter"
      aria-label={`${label}: ${value} percent ${unit}`}
      aria-valuenow={value}
      aria-valuemin={0}
      aria-valuemax={100}
      className="relative size-24 shrink-0"
    >
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
          strokeDashoffset={length * (1 - value / 100)}
          className={cx("transition-[stroke-dashoffset] duration-700", value >= 90 ? "stroke-bad" : "stroke-violet-soft")}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <span className="font-display text-xl font-extrabold tabular-nums">{value}%</span>
        <span className="text-[9.5px] font-bold tracking-wider text-ink-faint uppercase">{unit}</span>
      </div>
    </div>
  );
}

/** The last `LIVE_HISTORY` readings of a 0-100 series, newest at the right.
 * A reading that was not taken leaves a gap. */
export function sparkPoints(series: (number | null)[], slots = LIVE_HISTORY): string[] {
  const start = slots - series.length;
  const runs: string[] = [];
  let run: string[] = [];
  series.forEach((v, i) => {
    if (v === null) {
      if (run.length) runs.push(run.join(" "));
      run = [];
      return;
    }
    const x = ((start + i) / (slots - 1)) * 100;
    const y = 30 - (Math.min(100, Math.max(0, v)) / 100) * 28 - 1;
    run.push(`${formatSvg(x)},${formatSvg(y)}`);
  });
  if (run.length) runs.push(run.join(" "));
  return runs;
}

const formatSvg = (n: number) => String(Math.round(n * 100) / 100);

function Sparkline({ label, series }: { label: string; series: (number | null)[] }) {
  const gradient = `spark-${useId().replace(/[^\w-]/g, "")}`;
  const runs = sparkPoints(series);
  const read = series.filter((v): v is number => v !== null);
  const summary = read.length
    ? `${label} over the last readings: lowest ${Math.min(...read)} percent, highest ${Math.max(...read)} percent`
    : `${label}: no readings yet`;
  return (
    <svg role="img" aria-label={summary} viewBox="0 0 100 30" preserveAspectRatio="none" className="mt-3 block h-12 w-full overflow-visible">
      <defs>
        <linearGradient id={gradient} x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" className="[stop-color:var(--color-violet-soft)] [stop-opacity:0.35]" />
          <stop offset="1" className="[stop-color:var(--color-violet-soft)] [stop-opacity:0]" />
        </linearGradient>
      </defs>
      {[25, 50, 75].map((p) => (
        <line key={p} x1="0" x2="100" y1={29 - (p / 100) * 28} y2={29 - (p / 100) * 28} vectorEffect="non-scaling-stroke" className="stroke-line" strokeDasharray="2 3" />
      ))}
      {runs.map((points, i) => {
        const xs = points.split(" ").map((p) => p.split(",")[0]);
        return (
          <g key={i}>
            <polygon points={`${xs[0]},30 ${points} ${xs[xs.length - 1]},30`} fill={`url(#${gradient})`} />
            <polyline
              points={points}
              fill="none"
              strokeWidth="2"
              strokeLinejoin="round"
              strokeLinecap="round"
              vectorEffect="non-scaling-stroke"
              className="stroke-violet-soft"
            />
          </g>
        );
      })}
    </svg>
  );
}

/** One bar per logical processor, in Windows' order. */
function CoreBars({ cores }: { cores: number[] }) {
  const busiest = Math.max(...cores);
  const gap = 1.5;
  const width = (100 - gap * (cores.length - 1)) / cores.length;
  return (
    <div className="mt-3">
      <p className="mb-1 text-[11px] text-ink-faint">Each logical processor</p>
      <svg
        role="img"
        aria-label={`${cores.length} logical processors, the busiest at ${busiest} percent`}
        viewBox="0 0 100 24"
        preserveAspectRatio="none"
        className="block h-8 w-full"
      >
        {cores.map((c, i) => {
          const h = Math.max(1, (Math.min(100, c) / 100) * 24);
          const x = formatSvg(i * (width + gap));
          return (
            <g key={i}>
              <rect x={x} y="0" width={formatSvg(width)} height="24" rx="0.8" className="fill-surface-3" />
              <rect
                x={x}
                y={formatSvg(24 - h)}
                width={formatSvg(width)}
                height={formatSvg(h)}
                rx="0.8"
                className={cx("transition-all duration-500", c >= 90 ? "fill-bad" : "fill-violet-soft")}
              />
            </g>
          );
        })}
      </svg>
    </div>
  );
}

function Bar({ label, used, total, text }: { label: string; used: number; total: number; text: string }) {
  const pct = Math.max(0, Math.min(100, (used / total) * 100));
  return (
    <div className="mt-3 text-xs">
      <div className="mb-1 flex justify-between gap-2">
        <span className="text-ink-faint">{label}</span>
        <span className="font-bold tabular-nums">{text}</span>
      </div>
      <svg role="img" aria-label={`${label}: ${text}`} className="block h-1.5 w-full" preserveAspectRatio="none">
        <rect width="100%" height="6" rx="3" className="fill-surface-3" />
        <rect width={`${pct}%`} height="6" rx="3" className="fill-violet-soft" />
      </svg>
    </div>
  );
}

/**
 * Windows' thermal zones. They come from the motherboard's firmware and are
 * often not the processor; its own temperature needs a kernel driver, which
 * PeakTweaks never installs (plan section 12).
 */
function BoardTemperatures({ reading }: { reading: Probe<{ name: string; celsius: number }[]> }) {
  const zones = yes(reading);
  return (
    <div className="mt-3 border-t border-line pt-3 text-xs">
      <p className="flex items-center gap-1.5 text-ink-faint">
        <Thermometer aria-hidden className="size-3.5" strokeWidth={1.8} />
        Motherboard temperature sensors
      </p>
      {zones ? (
        <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
          {zones.slice(0, 4).map((z) => (
            <li key={z.name} className="tabular-nums">
              <span className="font-bold">{formatNumber(z.celsius)} °C</span>
              <span className="ml-1.5 text-ink-faint">{z.name.replace(/^\\_TZ\./, "")}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-1 text-ink-muted">{reading.state === "yes" ? "" : reading.state === "no" ? reading.reason + "." : `Not read: ${reading.reason}.`}</p>
      )}
      <p className="mt-1 text-ink-faint">
        These are often not the processor itself. Reading the processor's own temperature needs a driver PeakTweaks does not install.
      </p>
    </div>
  );
}

/** Clean memory from Home: the same action as Tools' Quick tools. */
function CleanMemoryButton() {
  const { op, capturing, clean } = useCleanMemory();
  const result = op.status === "done" ? op.value : null;
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
      <Button
        variant="secondary"
        className="px-3 py-1.5 text-xs"
        busy={op.status === "running"}
        disabled={capturing}
        onClick={() => void clean()}
        icon={<Sparkles aria-hidden className="size-3.5" />}
      >
        Clean memory
      </Button>
      {result && (
        <span role="status" className="text-xs text-ink-muted">
          Cleaned: {gb(Math.max(0, result.before.cachedBytes - result.after.cachedBytes))} of files let go.
        </span>
      )}
      {op.status === "failed" && (
        <span role="alert" className="text-xs text-bad">
          {explain(op.error).title}
        </span>
      )}
    </div>
  );
}
