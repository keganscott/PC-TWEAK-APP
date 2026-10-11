import { useMemo, useState, type ReactNode } from "react";
import { ArrowRightLeft, Check, ClipboardCopy, History, Layers, LifeBuoy, ShieldCheck, Undo2, type LucideIcon } from "lucide-react";

import type { ActionDone } from "../../generated/ActionDone";
import type { AppliedChange } from "../../generated/AppliedChange";
import type { ChangeKind } from "../../generated/ChangeKind";
import type { OneTimeAction } from "../../generated/OneTimeAction";
import type { JournalWarning } from "../../generated/JournalWarning";
import type { Record as JournalRecord } from "../../generated/Record";
import type { RestorePoint } from "../../generated/RestorePoint";
import type { RestoreStatus } from "../../generated/RestoreStatus";
import { explain } from "../../lib/errors";
import { formatBytes, formatDate, formatDateTime, formatDuration, formatTime } from "../../lib/format";
import { summaryText } from "../../lib/report";
import { appliedAt } from "../../lib/restartCheck";
import { planSetup, readSetupCode, setupCode } from "../../lib/setupCode";
import { describeEffect, describeItem, describeState } from "../../lib/systemItems";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Callout, Card, Dialog, ErrorCallout, PageHeader, SampleBadge, StatusBadge, cx } from "../ui/primitives";
import { categoryIcon } from "./categories";

export function BackupsView() {
  const tweaks = useStore((s) => s.tweaks);
  const journal = useStore((s) => s.journal);
  const restore = useStore((s) => s.audit?.env.restore ?? null);
  const revertAllOp = useStore((s) => s.revertAllOp);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { revertAll } = useActions();
  const [confirming, setConfirming] = useState(false);
  const hardware = useStore((s) => s.audit?.env.hardware ?? null);
  // "copied", or the text itself when the clipboard refused it, so it can
  // still be selected by hand.
  const [copy, setCopy] = useState<{ copied: true } | { copied: false; text: string } | null>(null);

  // From the journal, not the tweak list: every change with an apply still on
  // record, including PeakTweaks' own and any this version no longer ships.
  const applied = useMemo(() => journal?.applied ?? [], [journal]);
  const appliedTimes = useMemo(() => (journal ? appliedAt(journal) : new Map<string, number>()), [journal]);
  const records = useMemo(() => [...(journal?.records ?? [])].sort((a, b) => b.seq - a.seq), [journal]);
  // Windows' numbers of the restore points PeakTweaks made and checked.
  const madeHere = useMemo(
    () => new Set(records.flatMap((r) => (r.record === "restore_point" ? [r.sequenceNumber] : []))),
    [records],
  );
  const name = (id: string) =>
    tweaks.find((t) => t.id === id)?.name ?? applied.find((c) => c.tweakId === id)?.name ?? id;
  const category = (id: string) => tweaks.find((t) => t.id === id)?.category ?? null;

  const copySummary = async () => {
    const text = summaryText(hardware, applied, tweaks, new Date(), sample);
    try {
      await navigator.clipboard.writeText(text);
      setCopy({ copied: true });
    } catch {
      setCopy({ copied: false, text });
    }
  };

  return (
    <>
      <PageHeader
        title="Backups"
        description="Everything PeakTweaks changed, the restore points it made, and the way back."
        actions={
          <>
            <Button
              icon={copy?.copied ? <Check aria-hidden className="size-4" /> : <ClipboardCopy aria-hidden className="size-4" />}
              title="A plain-text list of this PC and the changes in place, to paste into a message. Nothing is sent anywhere."
              onClick={() => void copySummary()}
            >
              {copy?.copied ? "Copied" : "Copy summary"}
            </Button>
            <Button
              variant="danger"
              icon={<Undo2 aria-hidden className="size-4" />}
              disabled={applied.length === 0}
              busy={revertAllOp.status === "running"}
              onClick={() => setConfirming(true)}
            >
              Undo all
            </Button>
          </>
        }
      />
      <div className="flex flex-col gap-5">
        {copy && !copy.copied && (
          <Callout tone="warn" title="Windows did not allow copying. Select the text below and copy it yourself.">
            <textarea
              readOnly
              aria-label="Summary of this PC and its changes"
              className="mt-2 h-48 w-full rounded-lg border border-line bg-transparent p-2 font-mono text-xs"
              value={copy.text}
              onFocus={(e) => e.currentTarget.select()}
            />
          </Callout>
        )}
        {revertAllOp.status === "failed" && <ErrorCallout text={explain(revertAllOp.error)} technical={technical} />}
        {revertAllOp.status === "done" && revertAllOp.value.some((r) => !r.ok) && (
          <Callout tone="warn" title="Some changes could not be undone.">
            <ul className="list-disc pl-5">
              {revertAllOp.value
                .filter((r) => !r.ok)
                .map((r) => (
                  <li key={r.tweakId}>
                    {name(r.tweakId)}: {r.error ?? "no reason given"}
                  </li>
                ))}
            </ul>
            <p className="mt-2">They are still recorded, so Undo can be tried again.</p>
          </Callout>
        )}

        <Overview applied={journal ? applied.length : null} restore={restore} records={records} madeHere={madeHere} name={name} />

        <div className="grid items-start gap-5 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
          <div className="flex min-w-0 flex-col gap-5">
            <Card aria-labelledby="applied-title">
              <SectionTitle id="applied-title" icon={Layers}>
                Applied now ({applied.length})
              </SectionTitle>
              {applied.length === 0 ? (
                <p className="mt-2 text-sm text-ink-muted">Nothing PeakTweaks changed is in effect.</p>
              ) : (
                <ul className="mt-3 divide-y divide-line">
                  {applied.map((c) => (
                    <AppliedRow
                      key={c.tweakId}
                      change={c}
                      category={category(c.tweakId)}
                      when={appliedTimes.get(c.tweakId)}
                      technical={technical}
                    />
                  ))}
                </ul>
              )}
            </Card>

            <ChangeRecord records={records} name={name} technical={technical} sample={sample} warnings={journal?.warnings ?? []} />
          </div>

          <div className="flex min-w-0 flex-col gap-5">
            <Card aria-labelledby="points-title">
              <div className="flex items-center gap-2">
                <SectionTitle id="points-title" icon={ShieldCheck}>
                  Windows restore points
                </SectionTitle>
                {sample && <SampleBadge />}
              </div>
              {!restore ? (
                <p className="mt-2 text-sm text-ink-muted">Not read yet.</p>
              ) : restore.points.state !== "yes" ? (
                <p className="mt-2 text-sm text-ink-muted">
                  Windows did not list them.
                  {technical && <span className="mt-1 block break-all font-mono text-xs">{restore.points.reason}</span>}
                </p>
              ) : restore.points.value.length === 0 ? (
                <p className="mt-2 text-sm text-ink-muted">There are none.</p>
              ) : (
                <ul className="mt-3 flex flex-col gap-2 text-sm">
                  {[...restore.points.value].reverse().map((p) => (
                    <li key={p.sequenceNumber} className="flex items-center gap-3 rounded-xl border border-line bg-surface-2/60 px-3 py-2">
                      <span className="shrink-0 rounded-md bg-violet/15 px-2 py-0.5 font-mono text-xs font-bold text-violet-soft">
                        #{p.sequenceNumber}
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block break-words">{p.description}</span>
                        <span className="block text-xs text-ink-faint">
                          {p.createdUnixMs ? formatDateTime(p.createdUnixMs) : "time unknown"}
                        </span>
                      </span>
                      {madeHere.has(p.sequenceNumber) && <StatusBadge tone="ok">Made by PeakTweaks</StatusBadge>}
                    </li>
                  ))}
                </ul>
              )}
              <p className="mt-3 text-xs text-ink-faint">
                To roll the whole PC back, open Windows' System Restore (search for “Create a restore point”, then System
                Restore) and pick one of these.
              </p>
            </Card>

            <Card aria-labelledby="no-start-title" className="relative overflow-hidden border-violet/40">
              <span aria-hidden className="absolute inset-y-0 left-0 w-1 bg-violet" />
              <SectionTitle id="no-start-title" icon={LifeBuoy}>
                If Windows will not start
              </SectionTitle>
              <p className="mt-2 text-sm text-ink-muted">
                PeakTweaks keeps its undo files in its folder in ProgramData, usually{" "}
                <code className="rounded bg-surface-3 px-1.5 py-0.5 font-mono text-xs text-ink">C:\ProgramData\PeakTweaks</code>. If
                Windows starts only in Safe Mode, open the backups folder there and import the newest session file of each
                change. If it does not start at all, the offline folder there has README.txt with the steps and recover.cmd,
                which puts back the values PeakTweaks changed from the Windows Recovery Environment.
              </p>
              <p className="mt-2 text-xs font-bold text-violet-soft">
                Note this down now: if Windows does not start, this page cannot be opened.
              </p>
              {journal?.offlineError && (
                <div className="mt-3">
                  <Callout tone="warn" title="The offline undo files could not be brought up to date when PeakTweaks started.">
                    <p>They will be rewritten at the next change or Undo; if that fails too, it says why.</p>
                    {technical && <p className="mt-1 break-all font-mono text-xs">{journal.offlineError}</p>}
                  </Callout>
                </div>
              )}
            </Card>

            <CopySetup />
          </div>
        </div>
      </div>

      <Dialog
        open={confirming}
        title="Undo every change?"
        onClose={() => setConfirming(false)}
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              Keep them
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                setConfirming(false);
                void revertAll();
              }}
            >
              Undo {applied.length} change{applied.length === 1 ? "" : "s"}
            </Button>
          </>
        }
      >
        <p>
          PeakTweaks puts back the exact values it found before each change, newest first. Windows restore points are
          kept.
        </p>
      </Dialog>
    </>
  );
}

/** A section heading with its icon inside it, so the heading's own box is the card's. */
function SectionTitle({ id, icon: Icon, children }: { id: string; icon: LucideIcon; children: ReactNode }) {
  return (
    <h2 id={id} className="flex items-center gap-2 font-extrabold tracking-tight">
      <Icon aria-hidden className="size-4 shrink-0 text-lime" strokeWidth={2.25} />
      {children}
    </h2>
  );
}

/** The small uppercase label used on the overview tiles, as on Home. */
function Eyebrow({ children }: { children: ReactNode }) {
  return <p className="text-[10.5px] font-bold tracking-[0.14em] text-ink-faint uppercase">{children}</p>;
}

/** Three tiles over the page: what is in effect, the restore points, and the last change. */
function Overview({
  applied,
  restore,
  records,
  madeHere,
  name,
}: {
  applied: number | null;
  restore: RestoreStatus | null;
  records: readonly JournalRecord[];
  madeHere: ReadonlySet<number>;
  name: (id: string) => string;
}) {
  const points = restore?.points.state === "yes" ? restore.points.value : null;
  const newest = points
    ?.filter((p) => p.createdUnixMs !== null)
    .reduce<RestorePoint | null>((a, p) => (!a || p.createdUnixMs! > a.createdUnixMs! ? p : a), null);
  // Records are newest first.
  const ours = points?.filter((p) => madeHere.has(p.sequenceNumber)).length ?? 0;
  const last = records.find((r) => r.record === "commit");
  const lastLine =
    last?.record === "commit"
      ? `${last.action === "apply" ? "Applied" : last.action === "revert" ? "Undid" : "Rolled back"} ${name(last.tweakId)}.`
      : null;
  const tile = "flex flex-col rounded-2xl border border-line bg-surface-1 p-5";
  return (
    <div className="grid gap-4 md:grid-cols-3">
      <section aria-label="Changes in effect" className={cx(tile, "texture-lines")}>
        <Eyebrow>Changes in effect</Eyebrow>
        <div className="mt-2 flex items-baseline gap-2">
          <p className={cx("text-4xl font-bold tracking-tight tabular-nums", applied ? "text-lime" : "")}>{applied ?? "–"}</p>
          <p className="text-sm text-ink-muted">
            {applied === null ? "not read yet" : applied === 1 ? "change made by PeakTweaks" : "changes made by PeakTweaks"}
          </p>
        </div>
        <p className="mt-auto pt-3 text-xs text-ink-faint">
          {applied ? "Each was recorded first and has its own Undo below." : "None right now."}
        </p>
      </section>
      <section aria-label="Restore points" className={tile}>
        <Eyebrow>Restore points</Eyebrow>
        <div className="mt-2 flex items-baseline gap-2">
          <p className="text-4xl font-bold tracking-tight tabular-nums">{points ? points.length : "–"}</p>
          <p className="text-sm text-ink-muted">
            {!restore ? "not read yet" : !points ? "not listed by Windows" : "listed by Windows"}
          </p>
        </div>
        <p className="mt-auto pt-3 text-xs text-ink-faint">
          {newest ? `Newest #${newest.sequenceNumber}, made ${formatDateTime(newest.createdUnixMs!)}.` : "None with a date yet."}
          {ours > 0 && ` PeakTweaks made ${ours === 1 ? "1 of them" : `${ours} of them`}.`}
        </p>
      </section>
      <section aria-label="Last change" className={tile}>
        <Eyebrow>Last change</Eyebrow>
        {last ? (
          <>
            <p className="mt-2 text-lg font-bold leading-snug [overflow-wrap:anywhere]">{lastLine}</p>
            <p className="mt-auto pt-3 text-xs text-ink-faint">{formatDateTime(last.unixMs)}</p>
          </>
        ) : (
          <p className="mt-2 text-sm text-ink-muted">None yet.</p>
        )}
      </section>
    </div>
  );
}

/** Rows shown before "Show older". */
const RECORD_ROWS = 15;
/** Record kinds shown in plain wording; the rest are the technical detail under them. */
const PLAIN_RECORDS: ReadonlySet<JournalRecord["record"]> = new Set(["action", "restore_point", "commit", "note"]);

/** The change record as a timeline, newest first, a heading per day. */
function ChangeRecord({
  records,
  name,
  technical,
  sample,
  warnings,
}: {
  records: readonly JournalRecord[];
  name: (id: string) => string;
  technical: boolean;
  sample: boolean;
  warnings: readonly JournalWarning[];
}) {
  const [all, setAll] = useState(false);
  const visible = records.filter((r) => technical || PLAIN_RECORDS.has(r.record));
  const shown = all ? visible : visible.slice(0, RECORD_ROWS);
  const days: { label: string; rows: JournalRecord[] }[] = [];
  for (const r of shown) {
    const label = dayLabel(r.unixMs);
    const day = days.at(-1);
    if (day?.label === label) day.rows.push(r);
    else days.push({ label, rows: [r] });
  }
  return (
    <Card aria-labelledby="journal-title">
      <div className="flex items-center gap-2">
        <SectionTitle id="journal-title" icon={History}>
          Change record
        </SectionTitle>
        {sample && <SampleBadge />}
      </div>
      {warnings.length ? (
        <div className="mt-3">
          <Callout
            tone="warn"
            title={
              warnings.length === 1 ? "1 damaged line in the record was skipped." : `${warnings.length} damaged lines in the record were skipped.`
            }
          >
            {technical && warnings.map((w) => <p key={w.line}>Line {w.line}: {w.detail}</p>)}
          </Callout>
        </div>
      ) : null}
      {visible.length === 0 ? (
        <p className="mt-2 text-sm text-ink-muted">Nothing recorded yet.</p>
      ) : (
        <div className="mt-3 flex flex-col gap-4">
          {days.map((d) => (
            <div key={d.label}>
              <p className="text-[10.5px] font-bold tracking-[0.14em] text-ink-faint uppercase">{d.label}</p>
              <ol className="relative mt-1 text-sm before:absolute before:inset-y-3 before:left-[5px] before:w-px before:bg-line-strong">
                {d.rows.map((r) => (
                  <JournalRow key={r.seq} record={r} name={name} technical={technical} />
                ))}
              </ol>
            </div>
          ))}
          {visible.length > shown.length && (
            <div>
              <Button variant="ghost" onClick={() => setAll(true)}>
                Show older ({visible.length - shown.length} more)
              </Button>
            </div>
          )}
        </div>
      )}
    </Card>
  );
}

/** "Today", "Yesterday" or the date, in this PC's own calendar. */
function dayLabel(unixMs: number): string {
  const day = (ms: number) => new Date(ms).toDateString();
  const now = Date.now();
  if (day(unixMs) === day(now)) return "Today";
  if (day(unixMs) === day(now - 86_400_000)) return "Yesterday";
  return formatDate(unixMs);
}

/** One applied change with its own Undo: busy while it runs, and its error if it fails. */
const KIND_NOTE: Record<ChangeKind, string | null> = {
  catalogue: null,
  internal: "Made by PeakTweaks so it can create a restore point when you ask, even if Windows made one in the last day.",
  retired: "This version of PeakTweaks no longer includes this change, so Undo may not be able to put it back.",
};

/** Game plan new idea 7: this PC's applied changes as a text to paste into
 * PeakTweaks on another PC, and the other way round (`lib/setupCode.ts`). */
function CopySetup() {
  const tweaks = useStore((s) => s.tweaks);
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const applying = useStore((s) => s.applyManyOp.status === "running");
  const { applyMany } = useActions();
  const [copied, setCopied] = useState<boolean | string | null>(null);
  const [pasted, setPasted] = useState("");
  const [read, setRead] = useState<ReturnType<typeof readSetupCode> | null>(null);
  const mine = tweaks.filter((t) => t.state.status === "applied").length;
  const plan = read && "ids" in read ? planSetup(read.ids, tweaks) : null;

  const copy = async () => {
    const text = setupCode(tweaks);
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(text);
    }
  };
  const names = (list: { name: string }[]) => list.map((t) => t.name).join(", ");

  return (
    <Card aria-labelledby="setup-title">
      <SectionTitle id="setup-title" icon={ArrowRightLeft}>
        Copy this setup to another PC
      </SectionTitle>
      <p className="mt-2 text-sm text-ink-muted">
        Copy a short text that lists the changes PeakTweaks applied here, then paste it into PeakTweaks on the other PC.
        Nothing is sent anywhere, and the other PC checks each change against its own list before offering it.
      </p>
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <Button
          icon={copied === true ? <Check aria-hidden className="size-4" /> : <ClipboardCopy aria-hidden className="size-4" />}
          disabled={mine === 0}
          onClick={() => void copy()}
        >
          {copied === true ? "Copied" : `Copy setup (${mine})`}
        </Button>
        {mine === 0 && <p className="text-sm text-ink-faint">No change PeakTweaks applied is in effect here yet.</p>}
      </div>
      {typeof copied === "string" && (
        <textarea
          readOnly
          aria-label="This PC's setup"
          className="mt-3 h-20 w-full rounded-lg border border-line bg-transparent p-2 font-mono text-xs"
          value={copied}
          onFocus={(e) => e.currentTarget.select()}
        />
      )}

      <label htmlFor="setup-paste" className="mt-5 block text-sm font-medium">
        Paste a setup from another PC
      </label>
      <textarea
        id="setup-paste"
        value={pasted}
        onChange={(e) => {
          setPasted(e.target.value);
          setRead(null);
        }}
        className="mt-2 h-20 w-full rounded-lg border border-line bg-surface-0 p-2 font-mono text-xs"
      />
      <div className="mt-2">
        <Button variant="ghost" disabled={!pasted.trim()} onClick={() => setRead(readSetupCode(pasted))}>
          Check it
        </Button>
      </div>
      {read && "problem" in read && (
        <div className="mt-3">
          <Callout tone="warn" title={read.problem} />
        </div>
      )}
      {plan && (
        <div className="mt-3 flex flex-col gap-2 text-sm" role="status">
          {plan.apply.length > 0 ? (
            <div>
              <p>
                <span className="font-bold">Can be applied here ({plan.apply.length}):</span> {names(plan.apply)}.
              </p>
              <div className="mt-2">
                <Button
                  variant="primary"
                  busy={applying}
                  disabled={gateOpen === false}
                  onClick={() => void applyMany(plan.apply.map((t) => t.id))}
                >
                  {plan.apply.length === 1 ? "Apply 1 change" : `Apply ${plan.apply.length} changes`}
                </Button>
                {gateOpen === false && <p className="mt-1 text-ink-muted">Make a restore point first, on Home.</p>}
              </div>
            </div>
          ) : (
            <p>Nothing in it is left to apply here.</p>
          )}
          {plan.yourself.length > 0 && (
            <p>
              <span className="font-bold">Apply these yourself in Tools ({plan.yourself.length}):</span>{" "}
              {names(plan.yourself)}. Each has something to read first, or a reason Tools explains.
            </p>
          )}
          {plan.already.length > 0 && (
            <p className="text-ink-muted">
              Already in place here ({plan.already.length}): {names(plan.already)}.
            </p>
          )}
          {plan.notHere.length > 0 && (
            <p className="text-ink-muted">
              Not available on this PC ({plan.notHere.length}): {plan.notHere.join(", ")}.
            </p>
          )}
        </div>
      )}
    </Card>
  );
}

function AppliedRow({
  change,
  category,
  when,
  technical,
}: {
  change: AppliedChange;
  category: string | null;
  when: number | undefined;
  technical: boolean;
}) {
  const id = change.tweakId;
  const op = useStore((s) => s.tweakOps[id]);
  const { revertTweak, clearTweakOp } = useActions();
  const note = KIND_NOTE[change.kind];
  const Icon = change.kind === "internal" ? ShieldCheck : categoryIcon(category ?? "");
  return (
    <li className="py-3">
      <div className="flex items-center gap-3">
        <span
          aria-hidden
          className={cx(
            "grid size-9 shrink-0 place-items-center rounded-lg border",
            change.kind === "retired" ? "border-line text-ink-faint" : "border-lime/30 bg-lime/10 text-lime",
          )}
        >
          <Icon className="size-4" strokeWidth={2.25} />
        </span>
        <div className="min-w-0 flex-1">
          <span className="text-sm font-bold">{change.name}</span>
          {when !== undefined && <p className="text-xs text-ink-faint">Applied {formatDateTime(when)}</p>}
          {note && <p className="text-xs text-ink-faint">{note}</p>}
          {technical && <p className="font-mono text-xs text-ink-faint">{id}</p>}
        </div>
        <Button busy={op?.status === "running"} onClick={() => void revertTweak(id)} icon={<Undo2 aria-hidden className="size-4" />}>
          Undo
        </Button>
      </div>
      {op?.status === "failed" && (
        <div className="mt-2">
          <ErrorCallout
            text={explain(op.error)}
            technical={technical}
            action={
              <Button variant="ghost" onClick={() => clearTweakOp(id)}>
                Dismiss
              </Button>
            }
          />
        </div>
      )}
    </li>
  );
}

/** One-time actions from Tools: nothing to undo, listed so the record says what ran. */
const ACTION_DONE: Record<OneTimeAction, string> = {
  purge_standby: "Emptied the standby list",
  cleanup: "Cleared junk files",
  optimize_drive: "Optimized the Windows drive",
  install_gpu_driver: "NVIDIA driver clean install",
};

const ACTION_FAILED: Record<OneTimeAction, string> = {
  purge_standby: "Could not empty the standby list",
  cleanup: "Could not clear junk files",
  optimize_drive: "The drive optimization did not finish",
  install_gpu_driver: "The NVIDIA driver install did not finish",
};

function describeDone(done: ActionDone): string {
  switch (done.action) {
    case "purge_standby":
      return `files kept in memory ${formatBytes(done.cachedBefore)} before, ${formatBytes(done.cachedAfter)} after`;
    case "cleanup": {
      const files = `${done.removedFiles.toLocaleString()} ${done.removedFiles === 1 ? "file" : "files"}`;
      const left = done.leftFiles > 0 ? `, ${done.leftFiles.toLocaleString()} left in place` : "";
      return `deleted ${formatBytes(done.removedBytes)} in ${files}${left}`;
    }
    case "optimize_drive":
      return `drive ${done.drive}, Windows took ${formatDuration(done.seconds)}`;
    case "gpu_driver_started":
      return `started ${done.version ? `driver ${done.version}` : done.file} (a restore point puts the old driver back)`;
    case "gpu_driver_installed":
      return `driver ${done.version ?? done.file} installed${done.restart ? ", restart Windows to finish" : ""}`;
  }
}

/** The dot on the timeline: lime a change made or a job done, violet an undo or
 * a restore point, red a failure, grey the detail under a change. */
function Dot({ tone }: { tone: "made" | "undone" | "failed" | "detail" }) {
  return (
    <span
      aria-hidden
      className={cx(
        "absolute top-[13px] left-0 size-[11px] rounded-full ring-4 ring-surface-1",
        tone === "made" ? "bg-lime" : tone === "undone" ? "bg-violet-soft" : tone === "failed" ? "bg-bad" : "bg-line-strong",
      )}
    />
  );
}

function JournalRow({ record, name, technical }: { record: JournalRecord; name: (id: string) => string; technical: boolean }) {
  const when = formatTime(record.unixMs);
  const time = <span className="shrink-0 text-xs text-ink-faint tabular-nums">{when}</span>;
  if (record.record === "action") {
    return (
      <li className="relative py-2 pl-6">
        <Dot tone={record.done ? "made" : "failed"} />
        <div className="flex justify-between gap-3">
          <span>{record.done ? `${ACTION_DONE[record.action]}: ${describeDone(record.done)}` : ACTION_FAILED[record.action]}</span>
          {time}
        </div>
        {technical && record.error && <div className="break-all font-mono text-xs text-ink-faint">{record.error}</div>}
      </li>
    );
  }
  if (record.record === "restore_point") {
    return (
      <li className="relative flex justify-between gap-3 py-2 pl-6">
        <Dot tone="undone" />
        <span>
          <StatusBadge tone="ok">Restore point</StatusBadge> #{record.sequenceNumber} {record.description}
        </span>
        {time}
      </li>
    );
  }
  if (record.record === "commit") {
    const label = record.action === "apply" ? "Applied" : record.action === "revert" ? "Undone" : "Rolled back after an error";
    return (
      <li className="relative flex justify-between gap-3 py-2 pl-6">
        <Dot tone={record.action === "apply" ? "made" : record.action === "revert" ? "undone" : "failed"} />
        <span>
          {label}: {name(record.tweakId)}
        </span>
        {time}
      </li>
    );
  }
  if (record.record === "note") {
    return (
      <li className="relative flex justify-between gap-3 py-2 pl-6">
        <Dot tone="detail" />
        <span>
          {name(record.tweakId)}: {record.text}
        </span>
        {time}
      </li>
    );
  }
  if (!technical) return null;
  if (record.record === "change") {
    return (
      <li className="relative py-2 pl-6 font-mono text-xs text-ink-muted">
        <Dot tone="detail" />
        <div className="flex justify-between gap-3">
          <span className="break-all">
            {record.action} {describeItem(record.item)}
          </span>
          {time}
        </div>
        <div className="text-ink-faint">before: {describeState(record.previous, record.item)}</div>
      </li>
    );
  }
  if (record.record === "effect") {
    return (
      <li className="relative py-2 pl-6 font-mono text-xs text-ink-muted">
        <Dot tone={record.error ? "failed" : "detail"} />
        <div className="flex justify-between gap-3">
          <span className="break-all">
            {describeEffect(record.effect)}: {record.error ? `failed (${record.error})` : "done"}
          </span>
          {time}
        </div>
      </li>
    );
  }
  return (
    <li className="relative py-2 pl-6 font-mono text-xs text-ink-muted">
      <Dot tone="detail" />
      <div className="flex justify-between gap-3">
        <span className="break-all">
          {record.action} {record.displayPath}\{record.valueName}
        </span>
        {time}
      </div>
      <div className="text-ink-faint">backup: {record.backupFile}</div>
    </li>
  );
}
