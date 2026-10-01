import { useMemo, useState } from "react";
import { Undo2 } from "lucide-react";

import type { Record as JournalRecord } from "../../generated/Record";
import { explain } from "../../lib/errors";
import { formatDateTime } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Callout, Card, Dialog, ErrorCallout, PageHeader, SampleBadge, StatusBadge } from "../ui/primitives";

export function BackupsView() {
  const tweaks = useStore((s) => s.tweaks);
  const journal = useStore((s) => s.journal);
  const restore = useStore((s) => s.audit?.env.restore ?? null);
  const revertAllOp = useStore((s) => s.revertAllOp);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { revertAll, revertTweak } = useActions();
  const [confirming, setConfirming] = useState(false);

  const applied = useMemo(() => tweaks.filter((t) => t.state.status === "applied"), [tweaks]);
  const records = useMemo(() => [...(journal?.records ?? [])].sort((a, b) => b.seq - a.seq), [journal]);
  const name = (id: string) => tweaks.find((t) => t.id === id)?.name ?? id;

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
          <h2 id="applied-title" className="font-semibold">
            Applied now ({applied.length})
          </h2>
          {applied.length === 0 ? (
            <p className="mt-2 text-sm text-ink-muted">Nothing PeakTweaks changed is in effect.</p>
          ) : (
            <ul className="mt-3 divide-y divide-line">
              {applied.map((t) => (
                <li key={t.id} className="flex items-center justify-between gap-3 py-2">
                  <span className="text-sm">{t.name}</span>
                  <Button variant="ghost" onClick={() => void revertTweak(t.id)}>
                    Undo
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </Card>

        <Card aria-labelledby="points-title">
          <div className="flex items-center gap-2">
            <h2 id="points-title" className="font-semibold">
              Windows restore points
            </h2>
            {sample && <SampleBadge />}
          </div>
          {!restore ? (
            <p className="mt-2 text-sm text-ink-muted">Not read yet.</p>
          ) : restore.points.state !== "yes" ? (
            <p className="mt-2 text-sm text-ink-muted">Could not be read: {restore.points.reason}</p>
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
            To roll the whole PC back, open Windows' System Restore (search for “Create a restore point”, then System
            Restore) and pick one of these.
          </p>
        </Card>

        <Card aria-labelledby="journal-title">
          <div className="flex items-center gap-2">
            <h2 id="journal-title" className="font-semibold">
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

function JournalRow({ record, name, technical }: { record: JournalRecord; name: (id: string) => string; technical: boolean }) {
  const when = formatDateTime(record.unixMs);
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
