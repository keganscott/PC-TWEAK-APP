import { useMemo, useState } from "react";
import { Undo2 } from "lucide-react";

import type { ActionDone } from "../../generated/ActionDone";
import type { AppliedChange } from "../../generated/AppliedChange";
import type { ChangeKind } from "../../generated/ChangeKind";
import type { OneTimeAction } from "../../generated/OneTimeAction";
import type { Record as JournalRecord } from "../../generated/Record";
import { explain } from "../../lib/errors";
import { formatBytes, formatDateTime, formatDuration } from "../../lib/format";
import { describeEffect, describeItem, describeState } from "../../lib/systemItems";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Callout, Card, Dialog, ErrorCallout, PageHeader, SampleBadge, StatusBadge } from "../ui/primitives";

export function BackupsView() {
  const tweaks = useStore((s) => s.tweaks);
  const journal = useStore((s) => s.journal);
  const restore = useStore((s) => s.audit?.env.restore ?? null);
  const revertAllOp = useStore((s) => s.revertAllOp);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { revertAll } = useActions();
  const [confirming, setConfirming] = useState(false);

  // From the journal, not the tweak list: every change with an apply still on
  // record, including PeakTweaks' own and any this version no longer ships.
  const applied = useMemo(() => journal?.applied ?? [], [journal]);
  const records = useMemo(() => [...(journal?.records ?? [])].sort((a, b) => b.seq - a.seq), [journal]);
  const name = (id: string) =>
    tweaks.find((t) => t.id === id)?.name ?? applied.find((c) => c.tweakId === id)?.name ?? id;

  return (
    <>
      <PageHeader
        title="Backups"
        description="Everything PeakTweaks changed, the restore points it made, and the way back."
        actions={
          <Button
            variant="danger"
            icon={<Undo2 aria-hidden className="size-4" />}
            disabled={applied.length === 0}
            busy={revertAllOp.status === "running"}
            onClick={() => setConfirming(true)}
          >
            Undo all
          </Button>
        }
      />
      <div className="flex max-w-4xl flex-col gap-5">
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

        <Card aria-labelledby="applied-title">
          <h2 id="applied-title" className="font-extrabold tracking-tight">
            Applied now ({applied.length})
          </h2>
          {applied.length === 0 ? (
            <p className="mt-2 text-sm text-ink-muted">Nothing PeakTweaks changed is in effect.</p>
          ) : (
            <ul className="mt-3 divide-y divide-line">
              {applied.map((c) => (
                <AppliedRow key={c.tweakId} change={c} technical={technical} />
              ))}
            </ul>
          )}
        </Card>

        <Card aria-labelledby="points-title">
          <div className="flex items-center gap-2">
            <h2 id="points-title" className="font-extrabold tracking-tight">
              Windows restore points
            </h2>
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
            <ul className="mt-3 divide-y divide-line text-sm">
              {[...restore.points.value].reverse().map((p) => (
                <li key={p.sequenceNumber} className="flex justify-between gap-3 py-2">
                  <span>
                    #{p.sequenceNumber} {p.description}
                  </span>
                  <span className="text-ink-faint">{p.createdUnixMs ? formatDateTime(p.createdUnixMs) : "time unknown"}</span>
                </li>
              ))}
            </ul>
          )}
          <p className="mt-3 text-xs text-ink-faint">
            To roll the whole PC back, open Windows' System Restore (search for â€œCreate a restore pointâ€, then System
            Restore) and pick one of these.
          </p>
        </Card>

        <Card aria-labelledby="no-start-title">
          <h2 id="no-start-title" className="font-extrabold tracking-tight">
            If Windows will not start
          </h2>
          <p className="mt-2 text-sm text-ink-muted">
            PeakTweaks keeps its undo files in its folder in ProgramData, usually C:\ProgramData\PeakTweaks. If Windows
            starts only in Safe Mode, open the backups folder there and import the newest session file of each change.
            If it does not start at all, the offline folder there has README.txt with the steps and recover.cmd, which
            puts back the values PeakTweaks changed from the Windows Recovery Environment.
          </p>
          <p className="mt-2 text-xs text-ink-faint">
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

        <Card aria-labelledby="journal-title">
          <div className="flex items-center gap-2">
            <h2 id="journal-title" className="font-extrabold tracking-tight">
              Change record
            </h2>
            {sample && <SampleBadge />}
          </div>
          {journal?.warnings.length ? (
            <div className="mt-3">
              <Callout tone="warn" title={`${journal.warnings.length} damaged line(s) in the record were skipped.`}>
                {technical && journal.warnings.map((w) => <p key={w.line}>Line {w.line}: {w.detail}</p>)}
              </Callout>
            </div>
          ) : null}
          {records.length === 0 ? (
            <p className="mt-2 text-sm text-ink-muted">Nothing recorded yet.</p>
          ) : (
            <ol className="mt-3 divide-y divide-line text-sm">
              {records.map((r) => (
                <JournalRow key={r.seq} record={r} name={name} technical={technical} />
              ))}
            </ol>
          )}
        </Card>
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

/** One applied change with its own Undo: busy while it runs, and its error if it fails. */
const KIND_NOTE: Record<ChangeKind, string | null> = {
  catalogue: null,
  internal: "Made by PeakTweaks so it can create a restore point when you ask, even if Windows made one in the last day.",
  retired: "This version of PeakTweaks no longer includes this change, so Undo may not be able to put it back.",
};

function AppliedRow({ change, technical }: { change: AppliedChange; technical: boolean }) {
  const id = change.tweakId;
  const op = useStore((s) => s.tweakOps[id]);
  const { revertTweak, clearTweakOp } = useActions();
  const note = KIND_NOTE[change.kind];
  return (
    <li className="py-2">
      <div className="flex items-center justify-between gap-3">
        <div className="min-w-0">
          <span className="text-sm">{change.name}</span>
          {note && <p className="text-xs text-ink-faint">{note}</p>}
          {technical && <p className="font-mono text-xs text-ink-faint">{id}</p>}
        </div>
        <Button variant="ghost" busy={op?.status === "running"} onClick={() => void revertTweak(id)}>
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
};

const ACTION_FAILED: Record<OneTimeAction, string> = {
  purge_standby: "Could not empty the standby list",
  cleanup: "Could not clear junk files",
  optimize_drive: "The drive optimization did not finish",
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
  }
}

function JournalRow({ record, name, technical }: { record: JournalRecord; name: (id: string) => string; technical: boolean }) {
  const when = formatDateTime(record.unixMs);
  if (record.record === "action") {
    return (
      <li className="py-2">
        <div className="flex justify-between gap-3">
          <span>{record.done ? `${ACTION_DONE[record.action]}: ${describeDone(record.done)}` : ACTION_FAILED[record.action]}</span>
          <span className="shrink-0 text-ink-faint">{when}</span>
        </div>
        {technical && record.error && <div className="break-all font-mono text-xs text-ink-faint">{record.error}</div>}
      </li>
    );
  }
  if (record.record === "restore_point") {
    return (
      <li className="flex justify-between gap-3 py-2">
        <span>
          <StatusBadge tone="ok">Restore point</StatusBadge> #{record.sequenceNumber} {record.description}
        </span>
        <span className="text-ink-faint">{when}</span>
      </li>
    );
  }
  if (record.record === "commit") {
    const label = record.action === "apply" ? "Applied" : record.action === "revert" ? "Undone" : "Rolled back after an error";
    return (
      <li className="flex justify-between gap-3 py-2">
        <span>
          {label}: {name(record.tweakId)}
        </span>
        <span className="text-ink-faint">{when}</span>
      </li>
    );
  }
  if (!technical) return null;
  if (record.record === "change") {
    return (
      <li className="py-2 font-mono text-xs text-ink-muted">
        <div className="flex justify-between gap-3">
          <span className="break-all">
            {record.action} {describeItem(record.item)}
          </span>
          <span className="shrink-0 text-ink-faint">{when}</span>
        </div>
        <div className="text-ink-faint">before: {describeState(record.previous)}</div>
      </li>
    );
  }
  if (record.record === "effect") {
    return (
      <li className="py-2 font-mono text-xs text-ink-muted">
        <div className="flex justify-between gap-3">
          <span className="break-all">
            {describeEffect(record.effect)}: {record.error ? `failed (${record.error})` : "done"}
          </span>
          <span className="shrink-0 text-ink-faint">{when}</span>
        </div>
      </li>
    );
  }
  return (
    <li className="py-2 font-mono text-xs text-ink-muted">
      <div className="flex justify-between gap-3">
        <span className="break-all">
          {record.action} {record.displayPath}\{record.valueName}
        </span>
        <span className="shrink-0 text-ink-faint">{when}</span>
      </div>
      <div className="text-ink-faint">backup: {record.backupFile}</div>
    </li>
  );
}
