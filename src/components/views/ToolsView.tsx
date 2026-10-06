import { useId, useMemo, useState } from "react";

import type { TweakView } from "../../generated/TweakView";
import { blockedHint } from "../../lib/blocked";
import { explain } from "../../lib/errors";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { RestorePointButton, useCanMakeRestorePoint } from "../shell/RestorePointButton";
import { Button, Callout, Card, ErrorCallout, PageHeader, SampleBadge, StatusBadge, type Tone } from "../ui/primitives";

const STATE: Record<TweakView["state"]["status"], { tone: Tone; label: string }> = {
  default: { tone: "neutral", label: "Not applied" },
  applied: { tone: "ok", label: "Optimized" },
  foreign: { tone: "ok", label: "Already optimized" },
  drifted: { tone: "warn", label: "Changed outside PeakTweaks since it was applied" },
  blocked: { tone: "bad", label: "Not available" },
  unknown: { tone: "warn", label: "Could not read its current state" },
};

const TIER_LABEL = { free: null, pro: "Pro", ultimate: "Ultimate" } as const;

export function ToolsView() {
  const tweaks = useStore((s) => s.tweaks);
  // null until the audit has answered (or if it failed): the engine still
  // checks at apply time, so only a definite "no restore point" locks the UI.
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const [advanced, setAdvanced] = useState(false);
  const advancedId = useId();
  const canMake = useCanMakeRestorePoint();

  const groups = useMemo(() => {
    const visible = tweaks.filter((t) => advanced || t.safety === "safe");
    const byCategory = new Map<string, TweakView[]>();
    for (const t of visible) byCategory.set(t.category, [...(byCategory.get(t.category) ?? []), t]);
    return [...byCategory.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [tweaks, advanced]);
  const hiddenCount = tweaks.filter((t) => t.safety !== "safe").length;
  // Settings this PC already has count as done, whoever set them: they are
  // listed, not hidden, so the user sees the whole set.
  const doneCount = tweaks.filter((t) => t.state.status === "applied" || t.state.status === "foreign").length;

  return (
    <>
      <PageHeader
        title="Tools"
        description="Each change is recorded before it is made and can be undone on its own or all together from Backups."
        actions={
          <label htmlFor={advancedId} className="flex cursor-pointer items-center gap-2 text-sm text-ink-muted">
            <input
              id={advancedId}
              type="checkbox"
              role="switch"
              aria-checked={advanced}
              checked={advanced}
              onChange={(e) => setAdvanced(e.target.checked)}
              className="size-4 accent-accent"
            />
            Advanced{hiddenCount > 0 && !advanced ? ` (${hiddenCount} more)` : ""}
          </label>
        }
      />
      <div className="flex max-w-4xl flex-col gap-6">
        {gateOpen === false && (
          <Callout
            tone="warn"
            title="Changes are locked until there is a restore point."
            action={canMake === false ? undefined : <RestorePointButton />}
          >
            {canMake === false
              ? "System Restore is not available on this PC, so PeakTweaks cannot make changes here."
              : "A restore point lets Windows put the whole PC back the way it is now. One click makes it; it can take a minute."}
          </Callout>
        )}
        {tweaks.length > 0 && (
          <p className="text-sm text-ink-muted">
            {doneCount} of {tweaks.length} already optimized on this PC.
          </p>
        )}
        {groups.length === 0 && <p className="text-sm text-ink-muted">No changes are available in this view.</p>}
        {groups.map(([category, list]) => (
          <section key={category} aria-labelledby={`cat-${category}`}>
            <h2 id={`cat-${category}`} className="mb-3 text-base font-extrabold tracking-tight">
              {category.charAt(0).toUpperCase() + category.slice(1)}
            </h2>
            <ul className="flex flex-col gap-3">
              {list.map((t) => (
                <li key={t.id}>
                  <TweakCard tweak={t} gateOpen={gateOpen} />
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </>
  );
}

function TweakCard({ tweak, gateOpen }: { tweak: TweakView; gateOpen: boolean | null }) {
  const op = useStore((s) => s.tweakOps[tweak.id]);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { applyTweak, revertTweak, clearTweakOp } = useActions();
  const [acknowledged, setAcknowledged] = useState(false);
  const ackId = useId();

  const { tone, label } = STATE[tweak.state.status];
  const running = op?.status === "running";
  const applied = tweak.state.status === "applied";
  // Already set on this PC by Windows, the user or another program: nothing to
  // apply and nothing of ours to undo.
  const foreign = tweak.state.status === "foreign";
  // Our apply is still on record but Windows has another value now: both
  // directions stay open (set ours again, or put back what was there before).
  const drifted = tweak.state.status === "drifted";
  const canApply =
    !applied && !tweak.blocked && gateOpen !== false && tweak.state.status !== "unknown" && (!tweak.tradeoff || acknowledged);
  const tier = TIER_LABEL[tweak.tier];

  return (
    <Card className="p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-bold">{tweak.name}</h3>
            <StatusBadge tone={tone}>{label}</StatusBadge>
            {tier && <StatusBadge tone="info">{tier}</StatusBadge>}
            {tweak.safety !== "safe" && <StatusBadge tone="warn">Advanced</StatusBadge>}
            {sample && <SampleBadge />}
          </div>
          <p className="mt-1 text-sm text-ink-muted">{tweak.summary}</p>
          {/* The engine's reason, also when the change was applied before the block began (it keeps its Undo). */}
          {tweak.blocked && (
            <p className="mt-2 text-sm">
              {tweak.blocked.message} <span className="text-ink-muted">{blockedHint(tweak.blocked)}</span>
            </p>
          )}
          {tweak.state.status === "unknown" && technical && (
            <p className="mt-2 font-mono text-xs text-ink-faint">{tweak.state.detail}</p>
          )}
          {foreign && (
            <p className="mt-2 text-sm text-ink-muted">This PC already has this setting, so there is nothing to apply.</p>
          )}
          {drifted && (
            <p className="mt-2 text-sm text-ink-muted">
              Windows no longer has the value PeakTweaks set. Undo puts back what was there before PeakTweaks changed it.
            </p>
          )}
          {tweak.requiresReboot && <p className="mt-2 text-xs text-ink-faint">Takes effect after a restart.</p>}
          {technical && <p className="mt-2 break-all font-mono text-xs text-ink-faint">{tweak.target}</p>}
        </div>
        <div className="flex shrink-0 gap-2">
          {foreign ? null : applied ? (
            <Button busy={running} onClick={() => void revertTweak(tweak.id)}>
              Undo
            </Button>
          ) : (
            <>
              <Button variant="primary" busy={running} disabled={!canApply} onClick={() => void applyTweak(tweak.id)}>
                {drifted ? "Apply again" : "Apply"}
              </Button>
              {drifted && (
                <Button busy={running} onClick={() => void revertTweak(tweak.id)}>
                  Undo
                </Button>
              )}
            </>
          )}
        </div>
      </div>

      {tweak.tradeoff && !applied && !foreign && (
        <div className="mt-3 rounded-md border border-warn/40 bg-warn/10 p-3 text-sm">
          <p className="font-bold text-warn">Before you apply</p>
          <p className="mt-1 text-ink">{tweak.tradeoff}</p>
          <label htmlFor={ackId} className="mt-2 flex cursor-pointer items-center gap-2 text-ink-muted">
            <input
              id={ackId}
              type="checkbox"
              checked={acknowledged}
              onChange={(e) => setAcknowledged(e.target.checked)}
              className="size-4 accent-accent"
            />
            I have read this
          </label>
        </div>
      )}

      {op?.status === "failed" && (
        <div className="mt-3">
          <ErrorCallout
            text={explain(op.error)}
            technical={technical}
            action={
              <Button variant="ghost" onClick={() => clearTweakOp(tweak.id)}>
                Dismiss
              </Button>
            }
          />
        </div>
      )}
    </Card>
  );
}
