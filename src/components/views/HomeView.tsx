import { RefreshCw, ShieldPlus } from "lucide-react";

import type { Finding } from "../../generated/Finding";
import type { SystemAudit } from "../../generated/SystemAudit";
import type { BusEntry, State } from "../../store/store";
import { explain } from "../../lib/errors";
import { formatDateTime, probeValue, RIG_LABEL } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { useNavigate } from "../shell/nav";
import { Button, Callout, Card, ErrorCallout, PageHeader, Skeleton, StatusBadge, type Tone } from "../ui/primitives";

export function HomeView() {
  const audit = useStore((s) => s.audit);
  const auditOp = useStore((s) => s.auditOp);
  const technical = useTechnical();
  const { rescan } = useActions();

  return (
    <>
      <PageHeader
        title="Home"
        description={audit ? <StateSentence audit={audit} /> : "Checking this PC."}
        actions={
          <Button onClick={() => void rescan()} busy={auditOp.status === "running"} icon={<RefreshCw aria-hidden className="size-4" />}>
            Check again
          </Button>
        }
      />
      <div className="flex max-w-4xl flex-col gap-5">
        {auditOp.status === "failed" && <ErrorCallout text={explain(auditOp.error)} technical={technical} />}
        <RestoreLock />
        <LastChange />
        {audit ? <Findings findings={audit.scan.findings} /> : <FindingsSkeleton />}
      </div>
    </>
  );
}

/** One sentence on the PC's state (plan section 7). */
function StateSentence({ audit }: { audit: SystemAudit }) {
  const os = probeValue(audit.env.hardware?.os);
  const attention = audit.scan.findings.filter((f) => f.status === "attention").length;
  const rig = audit.effectiveRigClass ? `a ${RIG_LABEL[audit.effectiveRigClass]} rig` : "a PC whose class could not be told";
  const things = attention === 0 ? "nothing needs a look" : attention === 1 ? "1 thing is worth a look" : `${attention} things are worth a look`;
  return (
    <>
      {os ? os.caption : "Windows"} on {rig}; {things}.
    </>
  );
}

// ---------------------------------------------------------------------------
// Restore lock: an inline step, not a wall (plan section 7, locked decision)
// ---------------------------------------------------------------------------

function RestoreLock() {
  const audit = useStore((s) => s.audit);
  const restoreOp = useStore((s) => s.restoreOp);
  const lastStage = useStore(currentRestoreStage);
  const technical = useTechnical();
  const { createRestorePoint } = useActions();

  const restore = audit?.env.restore;
  const gateOpen = audit?.env.restoreGateOpen;

  // The engine verified the point before answering, so success shows at once,
  // not only after the (slower) audit re-read agrees.
  if (restoreOp.status === "done") {
    return (
      <Callout tone="ok" title={`Restore point #${restoreOp.value.sequenceNumber} is ready.`}>
        Windows recorded it as “{restoreOp.value.description}”. Every change PeakTweaks makes from now on can also be
        undone one by one from Backups.
      </Callout>
    );
  }
  if (!audit || !restore) return null;
  if (gateOpen) return <RecentPoint points={restore.points} />;

  if (restore.supported.state === "no") {
    return (
      <Callout tone="warn" title="This edition of Windows has no System Restore.">
        PeakTweaks makes no changes without a restore point to fall back on, so changes stay locked here. Scanning still
        works.
      </Callout>
    );
  }
  if (restore.disabledByPolicy) {
    return (
      <Callout tone="warn" title="System Restore is turned off by your organisation's policy.">
        PeakTweaks makes no changes without a restore point, so changes stay locked. Ask whoever manages this PC.
      </Callout>
    );
  }

  const running = restoreOp.status === "running";
  return (
    <Card aria-labelledby="restore-lock-title" className="border-warn/40">
      <div className="flex items-start gap-4">
        <ShieldPlus aria-hidden className="mt-0.5 size-6 shrink-0 text-warn" />
        <div className="flex-1">
          <h2 id="restore-lock-title" className="font-semibold">
            Step 1: make a restore point
          </h2>
          <p className="mt-1 text-sm text-ink-muted">
            Before PeakTweaks changes anything it asks Windows for a restore point, so the whole PC can be put back the
            way it is now. This turns on System Protection for the Windows drive if it is off, and can take a minute.
          </p>
          {restoreOp.status === "failed" && (
            <div className="mt-3">
              <ErrorCallout text={explain(restoreOp.error)} technical={technical} />
            </div>
          )}
          <div className="mt-4 flex items-center gap-3">
            <Button variant="primary" busy={running} onClick={() => void createRestorePoint()}>
              {restoreOp.status === "failed" ? "Try again" : "Make a restore point"}
            </Button>
            {running && lastStage && (
              <span role="status" className="text-sm text-ink-muted">
                {lastStage.message}
              </span>
            )}
          </div>
        </div>
      </div>
    </Card>
  );
}

/** The gate is open because Windows lists a recent restore point. Say which one,
 * and when it was made, so the user knows what a roll-back would return to. */
function RecentPoint({ points }: { points: NonNullable<SystemAudit["env"]["restore"]>["points"] }) {
  if (points.state !== "yes") return null;
  const newest = [...points.value]
    .filter((p) => p.createdUnixMs !== null)
    .sort((a, b) => (b.createdUnixMs ?? 0) - (a.createdUnixMs ?? 0))[0];
  if (!newest) return null;
  return (
    <Callout tone="ok" title={`Restore point #${newest.sequenceNumber} is ready.`}>
      Windows made it on {formatDateTime(newest.createdUnixMs!)} (“{newest.description}”). Rolling the PC back with System
      Restore returns it to how it was then; each PeakTweaks change can also be undone on its own from Backups.
    </Callout>
  );
}

/** The newest restore progress message of the current attempt, or null. Returns
 * an entry already in state, so the selector is stable. */
function currentRestoreStage(s: State): BusEntry | null {
  for (let i = s.bus.length - 1; i >= 0; i -= 1) {
    const e = s.bus[i]!;
    if (e.id <= s.restoreSinceBusId) break;
    if (e.stage.startsWith("restore_")) return e;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Result card after a change (plan section 7)
// ---------------------------------------------------------------------------

function LastChange() {
  const change = useStore((s) => s.lastChange);
  const tweaks = useStore((s) => s.tweaks);
  const { dismissChange } = useActions();
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
          <Button onClick={() => navigate("backups")}>Undo or review in Backups</Button>
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
              {name(f.tweakId)} could not be undone: {f.error ?? "no reason given"}
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
  fine: { tone: "ok", label: "Fine" },
};

function Findings({ findings }: { findings: Finding[] }) {
  const open = findings.filter((f) => f.status !== "fine");
  const fine = findings.filter((f) => f.status === "fine");
  return (
    <>
      <section aria-labelledby="findings-title">
        <h2 id="findings-title" className="mb-3 text-lg font-semibold">
          What the scan found
        </h2>
        {findings.length === 0 && <p className="text-sm text-ink-muted">This PC has not been checked yet.</p>}
        {findings.length > 0 && open.length === 0 && <p className="text-sm text-ink-muted">Nothing needs a look.</p>}
        <ul className="flex flex-col gap-3">
          {open.map((f) => (
            <li key={f.id}>
              <FindingCard finding={f} />
            </li>
          ))}
        </ul>
      </section>
      {fine.length > 0 && (
        <details className="group rounded-lg border border-line bg-surface-1 p-5">
          <summary className="cursor-pointer font-semibold">What is already right ({fine.length})</summary>
          <ul className="mt-3 flex flex-col gap-3">
            {fine.map((f) => (
              <li key={f.id}>
                <FindingCard finding={f} />
              </li>
            ))}
          </ul>
        </details>
      )}
    </>
  );
}

function FindingCard({ finding }: { finding: Finding }) {
  const { tone, label } = STATUS[finding.status];
  return (
    <Card className="p-4">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="font-medium">{finding.title}</h3>
        <StatusBadge tone={tone}>{label}</StatusBadge>
        {finding.guidedOnly && finding.status === "attention" && <StatusBadge tone="info">You do this one</StatusBadge>}
      </div>
      <p className="mt-2 text-sm text-ink-muted">{finding.reading}</p>
      {finding.remedy && <p className="mt-2 text-sm">{finding.remedy}</p>}
    </Card>
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
