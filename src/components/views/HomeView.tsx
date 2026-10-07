import {
  Check,
  CircleDot,
  Cpu,
  Gauge,
  Gpu,
  HardDrive,
  LayoutGrid,
  MemoryStick,
  Monitor,
  Printer,
  RefreshCw,
  ShieldCheck,
  type LucideIcon,
} from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { flushSync } from "react-dom";

import type { Finding } from "../../generated/Finding";
import type { FixBy } from "../../generated/FixBy";
import type { Probe } from "../../generated/Probe";
import type { SystemAudit } from "../../generated/SystemAudit";
import { explain } from "../../lib/errors";
import { formatDateTime, formatGiB, probeValue, RIG_LABEL } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { recommendedIds } from "../../store/store";
import { Facets } from "../brand/Facets";
import { useNavigate } from "../shell/nav";
import { RestorePointButton } from "../shell/RestorePointButton";
import { Button, Callout, cx, ErrorCallout, Skeleton, StatusBadge, type Tone } from "../ui/primitives";

/** The small uppercase label used across the dashboard. */
function Eyebrow({ children, className }: { children: ReactNode; className?: string }) {
  return <p className={cx("text-[10.5px] font-bold tracking-[0.14em] uppercase", className ?? "text-ink-faint")}>{children}</p>;
}

export function HomeView() {
  const audit = useStore((s) => s.audit);
  const auditOp = useStore((s) => s.auditOp);
  const technical = useTechnical();
  const { rescan } = useActions();

  return (
    <div className="flex max-w-[1240px] flex-col gap-6">
      <Greeting
        audit={audit}
        failed={!audit && auditOp.status === "failed"}
        actions={
          <Button onClick={() => void rescan()} busy={auditOp.status === "running"} icon={<RefreshCw aria-hidden className="size-4" />}>
            Check again
          </Button>
        }
      />
      {auditOp.status === "failed" && <ErrorCallout text={explain(auditOp.error)} technical={technical} />}
      <LastChange />
      <section aria-label="Safety and next step" className="grid gap-3.5 lg:grid-cols-[1.5fr_1fr] print:hidden">
        <NextStep />
        <div className="flex flex-col gap-3.5">
          <RestorePointCard />
          <ChangesCard />
        </div>
      </section>
      <YourPc audit={audit} />
      <div className="grid items-start gap-3.5 lg:grid-cols-[1.65fr_1fr]">
        <div className="flex min-w-0 flex-col gap-4">
          {audit ? <Findings findings={audit.scan.findings} /> : <FindingsSkeleton />}
        </div>
        <YourGames />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Greeting: the page heading and one sentence on the PC's state (plan section 7)
// ---------------------------------------------------------------------------

const today = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long" });

function partOfDay(hour: number): string {
  if (hour < 5) return "Good evening.";
  if (hour < 12) return "Good morning.";
  if (hour < 18) return "Good afternoon.";
  return "Good evening.";
}

function Greeting({ audit, failed, actions }: { audit: SystemAudit | null; failed: boolean; actions: ReactNode }) {
  const now = new Date();
  const attention = audit?.scan.findings.filter((f) => f.status === "attention").length ?? 0;
  return (
    <header className="flex flex-wrap items-end justify-between gap-4">
      <div className="min-w-0">
        <div className="flex items-center gap-2.5 text-[12.5px] font-semibold text-ink-faint">
          <h1 className="font-bold tracking-[0.14em] text-ink-muted uppercase">Home</h1>
          <span aria-hidden className="size-[3px] rounded-full bg-ink-faint" />
          <span>{today.format(now)}</span>
        </div>
        <p className="mt-2 text-3xl font-extrabold tracking-tight">
          {partOfDay(now.getHours())}{" "}
          {failed ? (
            <span>The check did not finish.</span>
          ) : !audit ? (
            "Checking this PC."
          ) : attention === 0 ? (
            "Nothing needs a look."
          ) : (
            <>
              <span className="text-lime">
                {attention} {attention === 1 ? "thing" : "things"}
              </span>{" "}
              worth a look.
            </>
          )}
        </p>
        {audit && (
          <p className="mt-1.5 text-sm text-ink-muted">
            <StateSentence audit={audit} />
          </p>
        )}
      </div>
      <div className="flex items-center gap-2 print:hidden">{actions}</div>
    </header>
  );
}

/** Windows edition, PC class and how many checks were fine. */
function StateSentence({ audit }: { audit: SystemAudit }) {
  const os = probeValue(audit.env.hardware?.os);
  const findings = audit.scan.findings;
  const fine = findings.filter((f) => f.status === "fine").length;
  const rig = audit.effectiveRigClass ? `${RIG_LABEL[audit.effectiveRigClass]} rig` : "PC class not known";
  return (
    <>
      {os ? os.caption.replace(/^Microsoft /, "") : "Windows"} · {rig} · {fine} of {findings.length} checks look good
    </>
  );
}

// ---------------------------------------------------------------------------
// Next step: the restore lock as an inline step, not a wall (plan section 7,
// locked decision); once it is open, where to go next.
// ---------------------------------------------------------------------------

function VioletCard({ label, children }: { label: string; children: ReactNode }) {
  // Every text colour on violet is plain white: lighter tints fall below 4.5:1.
  return (
    <section aria-label={label} className="relative flex min-h-56 flex-col overflow-hidden rounded-2xl bg-violet p-6 text-white">
      <Facets className="pointer-events-none absolute inset-y-0 right-0 h-full w-72" />
      <div className="relative flex flex-1 flex-col">{children}</div>
    </section>
  );
}

const STEPS = ["Restore point", "Choose changes", "Measure a game", "Undo any time"];

function NextStep() {
  const audit = useStore((s) => s.audit);
  const restoreOp = useStore((s) => s.restoreOp);
  const targetGame = useStore((s) => s.targetGame);
  const tweaks = useStore((s) => s.tweaks);
  const applyingMany = useStore((s) => s.applyManyOp.status === "running");
  const { applyMany } = useActions();
  const navigate = useNavigate();
  const safeSet = recommendedIds(tweaks);

  const auditFailed = useStore((s) => s.auditOp.status === "failed");
  const restore = audit?.env.restore;
  const gateOpen = audit?.env.restoreGateOpen;

  if (!audit && auditFailed) {
    return (
      <VioletCard label="Next step">
        <Eyebrow className="text-white">Next step</Eyebrow>
        <h2 className="mt-2.5 text-2xl font-extrabold tracking-tight">The check did not finish.</h2>
        <p className="mt-2 max-w-md text-sm">
          The reason is shown above. Check again to try once more; nothing on this PC was changed.
        </p>
      </VioletCard>
    );
  }
  if (!audit || !restore) {
    return (
      <VioletCard label="Next step">
        <Eyebrow className="text-white">Next step</Eyebrow>
        <h2 className="mt-2.5 text-2xl font-extrabold tracking-tight">Checking this PC.</h2>
        <p className="mt-2 max-w-md text-sm">The scan reads settings only. Nothing is changed.</p>
      </VioletCard>
    );
  }

  // The engine verified a new point before answering, so success counts at
  // once, not only after the (slower) audit re-read agrees.
  if (gateOpen || restoreOp.status === "done") {
    return (
      <VioletCard label="Next step">
        <Eyebrow className="text-white">Next step</Eyebrow>
        <h2 className="mt-2.5 max-w-lg text-2xl leading-tight font-extrabold tracking-tight">
          Choose the changes for this PC.
        </h2>
        <p className="mt-2 max-w-md text-sm">
          Each one is recorded before it is made and can be undone on its own or all together from Backups.
        </p>
        <ol className="mt-4 flex flex-wrap gap-1.5 text-xs font-semibold">
          {STEPS.map((step, i) => (
            <li key={step} className="flex items-center gap-1.5 rounded-md bg-black/25 px-2.5 py-1.5">
              {i === 0 ? (
                <>
                  <Check aria-hidden className="size-3.5 text-lime" strokeWidth={3} />
                  <span className="sr-only">Done:</span>
                </>
              ) : (
                <span className="font-extrabold text-lime">0{i + 1}</span>
              )}
              {step}
            </li>
          ))}
        </ol>
        <div className="mt-auto flex flex-wrap items-center gap-4 pt-5">
          {safeSet.length > 0 && (
            <Button variant="go" busy={applyingMany} onClick={() => void applyMany(safeSet)}>
              Apply the safe set ({safeSet.length})
            </Button>
          )}
          <Button variant={safeSet.length > 0 ? "secondary" : "go"} onClick={() => navigate("tools")}>
            Open Tools
          </Button>
          {!targetGame && (
            <button
              type="button"
              onClick={() => navigate("games")}
              className="border-b-[1.5px] border-white/60 pb-px text-sm font-bold hover:border-white"
            >
              Pick your main game first
            </button>
          )}
        </div>
      </VioletCard>
    );
  }

  if (restore.supported.state === "no") {
    return (
      <VioletCard label="Next step">
        <Eyebrow className="text-white">Changes are locked</Eyebrow>
        <h2 className="mt-2.5 text-2xl font-extrabold tracking-tight">This edition of Windows has no System Restore.</h2>
        <p className="mt-2 max-w-md text-sm">
          PeakTweaks makes no changes without a restore point to fall back on, so changes stay locked here. Scanning
          still works.
        </p>
      </VioletCard>
    );
  }
  if (restore.disabledByPolicy) {
    return (
      <VioletCard label="Next step">
        <Eyebrow className="text-white">Changes are locked</Eyebrow>
        <h2 className="mt-2.5 text-2xl font-extrabold tracking-tight">
          System Restore is turned off by your organisation's policy.
        </h2>
        <p className="mt-2 max-w-md text-sm">
          PeakTweaks makes no changes without a restore point, so changes stay locked. Ask whoever manages this PC.
        </p>
      </VioletCard>
    );
  }

  return (
    <VioletCard label="Next step">
      <Eyebrow className="text-white">Before the first change</Eyebrow>
      <h2 id="restore-lock-title" className="mt-2.5 text-2xl font-extrabold tracking-tight">
        Step 1: make a restore point
      </h2>
      <p className="mt-2 max-w-lg text-sm">
        Before PeakTweaks changes anything it asks Windows for a restore point, so the whole PC can be put back the way
        it is now. One click does it: this turns on System Protection for the Windows drive if it is off, and can take
        a minute.
      </p>
      <RestorePointButton variant="go" className="mt-auto pt-5" />
    </VioletCard>
  );
}


// ---------------------------------------------------------------------------
// Restore point and changes in effect
// ---------------------------------------------------------------------------

function StatCard({ label, children, className }: { label: string; children: ReactNode; className?: string }) {
  return (
    <section aria-label={label} className={cx("flex flex-1 flex-col rounded-2xl border border-line bg-surface-1 p-5", className)}>
      <Eyebrow>{label}</Eyebrow>
      {children}
    </section>
  );
}

/** Which restore point a roll-back would return to: the one just made, or the
 * newest Windows lists while the gate is open. */
function RestorePointCard() {
  const audit = useStore((s) => s.audit);
  const restoreOp = useStore((s) => s.restoreOp);
  const navigate = useNavigate();

  let point: { seq: number; title: string; detail: string } | null = null;
  if (restoreOp.status === "done") {
    point = {
      seq: restoreOp.value.sequenceNumber,
      title: `Restore point #${restoreOp.value.sequenceNumber} is ready.`,
      detail: `Windows recorded it as “${restoreOp.value.description}”. Every change PeakTweaks makes from now on can also be undone one by one from Backups.`,
    };
  } else if (audit?.env.restoreGateOpen && audit.env.restore?.points.state === "yes") {
    const newest = [...audit.env.restore.points.value]
      .filter((p) => p.createdUnixMs !== null)
      .sort((a, b) => (b.createdUnixMs ?? 0) - (a.createdUnixMs ?? 0))[0];
    if (newest) {
      point = {
        seq: newest.sequenceNumber,
        title: `Restore point #${newest.sequenceNumber} is ready.`,
        detail: `Windows made it on ${formatDateTime(newest.createdUnixMs!)} (“${newest.description}”). Rolling the PC back with System Restore returns it to how it was then.`,
      };
    }
  }

  return (
    <StatCard label="Restore point" className="texture-lines">
      {point ? (
        <>
          <div className="mt-2 flex items-start justify-between gap-3">
            <p aria-hidden className="text-4xl font-bold tracking-tight tabular-nums">
              #{point.seq}
            </p>
            <StatusBadge tone="ok">Ready</StatusBadge>
          </div>
          <p className="mt-2 text-sm font-bold">{point.title}</p>
          <p className="mt-1 text-xs leading-relaxed text-ink-muted">{point.detail}</p>
        </>
      ) : (
        <>
          <p className="mt-2 text-sm font-bold">{audit ? "None recent enough yet." : "Not read yet."}</p>
          <p className="mt-1 text-xs leading-relaxed text-ink-muted">
            PeakTweaks needs a recent restore point before its first change.
          </p>
        </>
      )}
      <div className="mt-auto flex justify-end pt-3">
        <LinkButton onClick={() => navigate("backups")}>All restore points</LinkButton>
      </div>
    </StatCard>
  );
}

function ChangesCard() {
  const applied = useStore((s) => s.journal?.applied.length ?? null);
  const navigate = useNavigate();
  return (
    <StatCard label="Changes in effect">
      <div className="mt-2 flex items-baseline gap-2">
        <p className="text-4xl font-bold tracking-tight tabular-nums">{applied ?? "–"}</p>
        <p className="text-sm text-ink-muted">
          {applied === null ? "not read yet" : applied === 1 ? "change made by PeakTweaks" : "changes made by PeakTweaks"}
        </p>
      </div>
      <div className="mt-auto flex items-center justify-between gap-3 pt-3">
        <span className="text-xs text-ink-faint">Recorded first, each can be undone.</span>
        <LinkButton onClick={() => navigate("backups")}>Review or undo</LinkButton>
      </div>
    </StatCard>
  );
}

function LinkButton({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <button type="button" onClick={onClick} className="group flex items-center gap-1.5 text-xs font-bold text-ink hover:text-lime">
      {children}
      <span aria-hidden className="text-lime">
        →
      </span>
    </button>
  );
}

// ---------------------------------------------------------------------------
// Your PC: real readings only (plan section 7: no invented live numbers)
// ---------------------------------------------------------------------------

function YourPc({ audit }: { audit: SystemAudit | null }) {
  const hw = audit?.env.hardware ?? null;
  const findings = audit?.scan.findings ?? [];
  const status = (ids: string[]): Tone | null => {
    const related = findings.filter((f) => ids.includes(f.id));
    if (related.some((f) => f.status === "attention")) return "warn";
    if (related.length > 0 && related.every((f) => f.status === "fine")) return "ok";
    return null;
  };

  return (
    <section aria-labelledby="your-pc-title">
      <div className="mb-2.5 flex items-baseline justify-between gap-3">
        <h2 id="your-pc-title" className="text-[15px] font-extrabold tracking-tight">
          Your PC
        </h2>
        {hw && <span className="text-xs font-semibold text-ink-faint">Read from Windows at the last check</span>}
      </div>
      {!hw ? (
        <div className="grid gap-3.5 sm:grid-cols-2 xl:grid-cols-4">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-32" label={i === 0 ? "Loading hardware" : "Loading"} />
          ))}
        </div>
      ) : (
        <ul className="grid gap-3.5 sm:grid-cols-2 xl:grid-cols-4">
          <CpuTile probe={hw.cpu} />
          <GpuTile audit={audit!} status={status(["gpu.driver_branch", "gpu.choice"])} />
          <MemoryTile probe={hw.memory} status={status(["memory.speed", "memory.channels"])} />
          <DisplayTile probe={hw.display} status={status(["display.refresh_rate"])} />
        </ul>
      )}
    </section>
  );
}

function Tile({
  label,
  icon: Icon,
  status,
  value,
  children,
}: {
  label: string;
  icon: LucideIcon;
  status?: Tone | null;
  value: ReactNode;
  children?: ReactNode;
}) {
  return (
    <li
      className={cx(
        "flex min-h-32 flex-col rounded-2xl border bg-surface-1 px-[18px] py-4 print:break-inside-avoid",
        status === "warn" ? "border-violet/70" : "border-line",
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="flex items-center gap-2">
          <Icon aria-hidden className="size-4 text-ink-faint" strokeWidth={1.8} />
          <Eyebrow>{label}</Eyebrow>
        </span>
        {status === "warn" && <StatusBadge tone="warn">Worth a look</StatusBadge>}
        {status === "ok" && <StatusBadge tone="ok">Good</StatusBadge>}
      </div>
      <p className="mt-3 truncate text-xl font-bold tracking-tight">{value}</p>
      <div className="mt-auto pt-2 text-xs text-ink-muted">{children}</div>
    </li>
  );
}

function CouldNotTell() {
  return <span className="text-ink-muted">Could not tell</span>;
}

/** A reading against its ceiling, e.g. 60 of 165 Hz. Drawn with SVG attributes,
 * not inline styles (the release CSP allows no inline styles). */
function Meter({ value, max, label }: { value: number; max: number; label: string }) {
  const pct = Math.max(0, Math.min(100, (value / max) * 100));
  return (
    <svg role="img" aria-label={label} className="mb-1.5 block h-1 w-full overflow-visible" preserveAspectRatio="none">
      <rect width="100%" height="4" rx="2" className="fill-surface-3" />
      <rect width={`${pct}%`} height="4" rx="2" className="fill-white" />
    </svg>
  );
}

function CpuTile({ probe }: { probe: Probe<{ name: string; cores: number; logicalProcessors: number }> }) {
  const cpu = probeValue(probe);
  return (
    <Tile label="Processor" icon={Cpu} value={cpu ? cpu.name : <CouldNotTell />}>
      {cpu && `${cpu.cores} cores · ${cpu.logicalProcessors} threads`}
    </Tile>
  );
}

function GpuTile({ audit, status }: { audit: SystemAudit; status: Tone | null }) {
  const gpu = probeValue(audit.env.hardware?.gpus)?.[0] ?? null;
  // DXGI and Win32_VideoController can name the same card differently; the
  // PCI vendor ties them together when the names do not match.
  const drivers = probeValue(audit.env.hardware?.gpuDrivers) ?? [];
  const driver = gpu
    ? (drivers.find((d) => d.name === gpu.name) ?? drivers.find((d) => d.vendorId !== null && d.vendorId === gpu.vendorId) ?? null)
    : null;
  const version = driver?.nvidiaVersion ?? driver?.driverVersion ?? null;
  return (
    <Tile label="Graphics" icon={Gpu} status={status} value={gpu ? gpu.name : <CouldNotTell />}>
      {gpu && [formatGiB(gpu.dedicatedVramBytes), version && `driver ${version}`].filter(Boolean).join(" · ")}
    </Tile>
  );
}

function MemoryTile({ probe, status }: { probe: SystemAuditMemory; status: Tone | null }) {
  const mem = probeValue(probe);
  // The slowest module sets the pace; compare it with its own rating.
  const slowest = mem?.sticks
    .filter((s) => s.configuredMhz !== null)
    .sort((a, b) => (a.configuredMhz ?? 0) - (b.configuredMhz ?? 0))[0];
  const running = slowest?.configuredMhz ?? null;
  const rated = slowest?.ratedMhz ?? null;
  const kind = mem?.sticks.find((s) => s.kind)?.kind ?? null;
  return (
    <Tile
      label="Memory"
      icon={MemoryStick}
      status={status}
      value={
        !mem ? (
          <CouldNotTell />
        ) : running && rated ? (
          <>
            <span className="tabular-nums">{running}</span>
            <span className="ml-1 text-sm font-semibold text-ink-faint">/ {rated} MT/s</span>
          </>
        ) : running ? (
          <>
            <span className="tabular-nums">{running}</span>
            <span className="ml-1 text-sm font-semibold text-ink-faint">MT/s</span>
          </>
        ) : (
          formatGiB(mem.installedBytes)
        )
      }
    >
      {mem && running && rated && <Meter value={running} max={rated} label={`Running at ${running} of a rated ${rated} MT/s`} />}
      {mem && [formatGiB(mem.installedBytes), kind].filter(Boolean).join(" · ")}
    </Tile>
  );
}

type SystemAuditMemory = NonNullable<SystemAudit["env"]["hardware"]>["memory"];
type SystemAuditDisplay = NonNullable<SystemAudit["env"]["hardware"]>["display"];

function DisplayTile({ probe, status }: { probe: SystemAuditDisplay; status: Tone | null }) {
  const d = probeValue(probe);
  return (
    <Tile
      label="Display"
      icon={Monitor}
      status={status}
      value={
        d ? (
          <>
            <span className="tabular-nums">{d.currentHz}</span>
            <span className="ml-1 text-sm font-semibold text-ink-faint">/ {d.maxHzAtCurrentResolution} Hz</span>
          </>
        ) : (
          <CouldNotTell />
        )
      }
    >
      {d && (
        <Meter
          value={d.currentHz}
          max={d.maxHzAtCurrentResolution}
          label={`Running at ${d.currentHz} of ${d.maxHzAtCurrentResolution} Hz`}
        />
      )}
      {d && `${d.width}×${d.height}`}
    </Tile>
  );
}

// ---------------------------------------------------------------------------
// Result card after a change (plan section 7)
// ---------------------------------------------------------------------------

function LastChange() {
  const change = useStore((s) => s.lastChange);
  const tweaks = useStore((s) => s.tweaks);
  const undoing = useStore((s) => s.applyManyOp.status === "running");
  const { dismissChange, revertMany } = useActions();
  const navigate = useNavigate();
  if (!change) return null;

  const name = (id: string) => tweaks.find((t) => t.id === id)?.name ?? id;
  const verb = change.kind === "apply" ? "Applied" : "Undid";
  const what = change.tweakIds.length ? change.tweakIds.map(name).join(", ") : "nothing";
  return (
    <Callout
      tone={change.failed.length ? "warn" : "ok"}
      title={`${verb}: ${what}`}
      action={
        <div className="flex flex-wrap gap-2">
          {change.kind === "apply" && change.tweakIds.length > 0 && (
            <Button busy={undoing} onClick={() => void revertMany(change.tweakIds)}>
              Undo these
            </Button>
          )}
          <Button onClick={() => navigate("backups")}>Review in Backups</Button>
          {change.kind === "apply" && <Button onClick={() => navigate("proof")}>Check the effect in Proof</Button>}
          <Button variant="ghost" onClick={dismissChange}>
            Dismiss
          </Button>
        </div>
      }
    >
      {change.failed.length > 0 && (
        <ul className="list-disc pl-5">
          {change.failed.map((f) => (
            <li key={f.tweakId}>
              {name(f.tweakId)} could not be {change.kind === "apply" ? "applied" : "undone"}: {f.error ?? "no reason given"}
            </li>
          ))}
        </ul>
      )}
    </Callout>
  );
}

// ---------------------------------------------------------------------------
// Scan findings
// ---------------------------------------------------------------------------

const STATUS: Record<Finding["status"], { tone: Tone; label: string }> = {
  attention: { tone: "warn", label: "Worth a look" },
  unknown: { tone: "neutral", label: "Could not tell" },
  fine: { tone: "ok", label: "Good" },
};

/** Who can act on a finding (plan 6.4: "fixed by us / fixable by you / needs
 * hardware"). The engine decides it; the UI only groups. */
const FIX_GROUPS: { by: FixBy; title: string; blurb: string }[] = [
  { by: "us", title: "PeakTweaks can fix", blurb: "One click each, recorded and undoable from Backups." },
  { by: "you", title: "You can fix", blurb: "Settings you change yourself. Each one says where." },
  {
    by: "hardware",
    title: "Needs different hardware",
    blurb: "Only a hardware change alters these. PeakTweaks does not push purchases.",
  },
];

/** An icon for each area the scanner checks, by the finding id's prefix. */
const AREA_ICON: Record<string, LucideIcon> = {
  memory: MemoryStick,
  display: Monitor,
  storage: HardDrive,
  games: HardDrive,
  gpu: Gpu,
  os: LayoutGrid,
  power: Gauge,
  security: ShieldCheck,
  background: Cpu,
};

function Findings({ findings }: { findings: Finding[] }) {
  const unknown = findings.filter((f) => f.status === "unknown");
  const fine = findings.filter((f) => f.status === "fine");
  const attention = findings.filter((f) => f.status === "attention");
  // The top bar, which labels demo data, does not print; the printout says it.
  const sample = useStore((s) => s.sample);
  // "Already good" is folded on screen, but a printed scan must show
  // all of it: open it for printing, whether from the button or Ctrl+P.
  const [fineOpen, setFineOpen] = useState(false);
  useEffect(() => {
    const open = () => flushSync(() => setFineOpen(true));
    window.addEventListener("beforeprint", open);
    return () => window.removeEventListener("beforeprint", open);
  }, []);
  const print = () => {
    flushSync(() => setFineOpen(true));
    window.print();
  };

  return (
    <>
      <section aria-labelledby="findings-title">
        <div className="mb-2.5 flex flex-wrap items-center justify-between gap-3">
          <h2 id="findings-title" className="text-[15px] font-extrabold tracking-tight">
            What the scan found
          </h2>
          {findings.length > 0 && (
            <Button
              variant="ghost"
              className="px-2.5 py-1 text-xs print:hidden"
              icon={<Printer aria-hidden className="size-4" />}
              onClick={print}
            >
              Print this scan
            </Button>
          )}
        </div>
        <p className="mb-3 hidden text-sm print:block">
          PeakTweaks scan, printed {formatDateTime(Date.now())}.
          {sample && <strong> SAMPLE: demo data, not this PC.</strong>}
        </p>
        {findings.length === 0 && <p className="text-sm text-ink-muted">This PC has not been checked yet.</p>}
        {findings.length > 0 && attention.length === 0 && unknown.length === 0 && (
          <p className="rounded-2xl border border-line bg-surface-1 p-5 text-sm text-ink-muted">Nothing needs a look.</p>
        )}
        <div className="flex flex-col gap-3.5">
          {FIX_GROUPS.map(({ by, title, blurb }) => {
            const list = attention.filter((f) => f.fixBy === by);
            if (list.length === 0) return null;
            return (
              <FindingGroup key={by} id={`fix-${by}`} title={`${title} (${list.length})`} blurb={blurb} findings={list} />
            );
          })}
          {unknown.length > 0 && (
            <FindingGroup
              id="fix-unknown"
              title={`Could not tell (${unknown.length})`}
              blurb="PeakTweaks could not read these. Each says why."
              findings={unknown}
              showStatus
            />
          )}
        </div>
      </section>
      {fine.length > 0 && (
        <details
          open={fineOpen}
          onToggle={(e) => setFineOpen(e.currentTarget.open)}
          className="group rounded-2xl border border-line bg-surface-1"
        >
          <summary className="flex cursor-pointer items-center gap-2.5 px-5 py-4 font-bold">
            <span aria-hidden className="grid size-[18px] place-items-center rounded-[5px] bg-lime">
              <Check className="size-3 text-black" strokeWidth={3.5} />
            </span>
            Already good ({fine.length})
          </summary>
          <ul className="divide-y divide-line border-t border-line">
            {fine.map((f) => (
              <li key={f.id}>
                <FindingRow finding={f} showStatus />
              </li>
            ))}
          </ul>
        </details>
      )}
    </>
  );
}

function FindingGroup({
  id,
  title,
  blurb,
  findings,
  showStatus = false,
}: {
  id: string;
  title: string;
  blurb: string;
  findings: Finding[];
  showStatus?: boolean;
}) {
  return (
    <section aria-labelledby={id} className="rounded-2xl border border-line bg-surface-1 print:break-inside-avoid">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 border-b border-line px-5 py-3.5">
        <h3 id={id} className="font-bold">
          {title}
        </h3>
        <p className="text-xs text-ink-faint">{blurb}</p>
      </div>
      <ul className="divide-y divide-line">
        {findings.map((f) => (
          <li key={f.id}>
            <FindingRow finding={f} nested showStatus={showStatus} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function FindingRow({ finding, nested = false, showStatus = false }: { finding: Finding; nested?: boolean; showStatus?: boolean }) {
  const { tone, label } = STATUS[finding.status];
  const Title = nested ? "h4" : "h3";
  const Icon = AREA_ICON[finding.id.split(".")[0] ?? ""] ?? CircleDot;
  return (
    <div className="flex gap-3.5 px-5 py-4 print:break-inside-avoid">
      <span aria-hidden className="grid size-9 shrink-0 place-items-center rounded-[9px] bg-surface-3">
        <Icon className="size-[17px] text-white" strokeWidth={1.8} />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <Title className="text-sm font-bold">{finding.title}</Title>
          {showStatus && <StatusBadge tone={tone}>{label}</StatusBadge>}
        </div>
        <p className="mt-1 text-xs leading-relaxed text-ink-muted">{finding.reading}</p>
        {finding.remedy && <p className="mt-1.5 text-xs leading-relaxed text-ink">{finding.remedy}</p>}
      </div>
    </div>
  );
}

function FindingsSkeleton() {
  return (
    <div className="flex flex-col gap-3" aria-busy="true">
      <Skeleton className="h-6 w-48" label="Loading scan results" />
      <Skeleton className="h-24 w-full" />
      <Skeleton className="h-24 w-full" />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Your games: what the scan found installed, and the main game
// ---------------------------------------------------------------------------

function YourGames() {
  const installs = useStore((s) => s.audit?.env.gameInstalls ?? null);
  const target = useStore((s) => s.targetGame);
  const sample = useStore((s) => s.sample);
  const navigate = useNavigate();
  return (
    <section aria-labelledby="your-games-title" className="print:hidden">
      <div className="mb-2.5 flex items-baseline justify-between gap-3">
        <h2 id="your-games-title" className="text-[15px] font-extrabold tracking-tight">
          Your games
        </h2>
        {installs && <span className="text-xs font-semibold text-ink-faint">{installs.length} found</span>}
      </div>
      <div className="rounded-2xl border border-line bg-surface-1 py-1.5">
        {installs === null ? (
          <p className="px-5 py-4 text-sm text-ink-muted">Not looked for yet.</p>
        ) : installs.length === 0 ? (
          <p className="px-5 py-4 text-sm text-ink-muted">No known games found in the places PeakTweaks looks.</p>
        ) : (
          <ul className="divide-y divide-line">
            {installs.map((g, i) => {
              const disk = probeValue(g.disk);
              return (
                <li key={g.gameId} className="flex items-center gap-3.5 px-5 py-3.5">
                  <span
                    aria-hidden
                    className={cx(
                      "grid size-11 shrink-0 place-items-center rounded-[10px] font-display text-lg font-extrabold",
                      i % 2 === 0 ? "bg-violet text-white" : "bg-white text-black",
                    )}
                  >
                    {g.name.charAt(0)}
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-bold">{g.name}</p>
                    <p className="mt-0.5 truncate text-xs text-ink-muted">
                      {g.drive}
                      {disk ? (disk.media === "hdd" ? " hard drive" : " solid-state drive") : ""}
                    </p>
                  </div>
                  {target === g.gameId && <StatusBadge tone="ok">Main game</StatusBadge>}
                </li>
              );
            })}
          </ul>
        )}
        <div className="px-5 pt-1.5 pb-3">
          <button
            type="button"
            onClick={() => navigate("games")}
            className="flex h-10 w-full items-center justify-center gap-2 rounded-[10px] border border-dashed border-line-strong text-xs font-bold text-ink-muted hover:border-ink-faint hover:text-ink"
          >
            {target ? "Games and anti-cheat checks" : "Pick your main game"}
          </button>
        </div>
        {sample && <p className="px-5 pb-3 text-[11px] text-sample">SAMPLE data</p>}
      </div>
    </section>
  );
}
