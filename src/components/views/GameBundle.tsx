import { Crosshair } from "lucide-react";
import { useId, useMemo, useState } from "react";

import { needsTick, planGameBundle } from "../../lib/presets";
import { useActions, useStore } from "../../store/hooks";
import { Button, Dialog, StatusBadge } from "../ui/primitives";

/**
 * One game's pro setup (NOTES N134): its start priority, fullscreen
 * optimizations, high-performance graphics chip and V-Sync setting, reviewed
 * and applied together like a preset. Every change is still the engine's own
 * tool, with its own record and Undo; while a game's anti-cheat keeps its
 * tools back (NOTES N75), they list as not offered and nothing here applies.
 */
export function GameBundleSection({ gameId, name }: { gameId: string; name: string }) {
  const tweaks = useStore((s) => s.tweaks);
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const [open, setOpen] = useState(false);
  const plan = useMemo(() => planGameBundle(gameId, tweaks), [gameId, tweaks]);
  const known = plan.toApply.length + plan.inPlace.length + plan.notHere.length;
  if (known === 0) return null;
  const qos = tweaks.find((t) => t.id === "network.qos.games");
  const done = plan.toApply.length === 0 && plan.inPlace.length > 0;
  return (
    <div className="mt-4 border-t border-line pt-3">
      <div className="flex flex-wrap items-center gap-2">
        <Crosshair aria-hidden className="size-4 text-ink-faint" strokeWidth={1.8} />
        <span className="text-sm font-bold">Pro setup for {name}</span>
        {done && <StatusBadge tone="ok">In place</StatusBadge>}
      </div>
      <p className="mt-1 text-sm text-ink-muted">
        {plan.inPlace.length} of {plan.toApply.length + plan.inPlace.length} in place
        {plan.notHere.length > 0 && ` · ${plan.notHere.length} not offered yet`}. Start priority, fullscreen, graphics
        chip and its own settings, in one review.
      </p>
      <Button
        className="mt-2"
        variant={done ? "secondary" : "primary"}
        disabled={plan.toApply.length === 0}
        onClick={() => setOpen(true)}
      >
        {plan.toApply.length === 0
          ? "Nothing to apply"
          : `Review ${plan.toApply.length} ${plan.toApply.length === 1 ? "change" : "changes"}`}
      </Button>
      {open && <GameBundleReview gameId={gameId} name={name} gateOpen={gateOpen} onClose={() => setOpen(false)} />}
      {qos && <QosRow qosId={qos.id} gateOpen={gateOpen} />}
    </div>
  );
}

/**
 * The shared game-traffic priority beside the setup: one Windows rule per
 * game found on this PC. It covers every game at once, so it keeps its own
 * switch here (and in Tools > Network) instead of joining the review.
 */
function QosRow({ qosId, gateOpen }: { qosId: string; gateOpen: boolean | null }) {
  const qos = useStore((s) => s.tweaks.find((t) => t.id === qosId));
  const op = useStore((s) => s.tweakOps[qosId]);
  const { applyTweak, revertTweak } = useActions();
  if (!qos) return null;
  const s = qos.state.status;
  const on = s === "applied" || s === "foreign";
  const blocked = Boolean(qos.blocked) || s === "blocked" || s === "unknown";
  const running = op?.status === "running";
  return (
    <div className="mt-3 flex flex-wrap items-center gap-2">
      <span className="text-sm">
        Game traffic priority:{" "}
        <span className="font-bold">{blocked ? "Not offered yet" : on ? "On" : "Off"}</span>
      </span>
      <Button
        variant="secondary"
        busy={running}
        disabled={blocked || gateOpen === false || running}
        onClick={() => void (on ? revertTweak(qosId) : applyTweak(qosId))}
      >
        {on ? "Undo" : "Apply"}
      </Button>
      <span className="text-xs text-ink-faint">One Windows rule per game found here; also in Tools.</span>
    </div>
  );
}

function GameBundleReview({
  gameId,
  name,
  gateOpen,
  onClose,
}: {
  gameId: string;
  name: string;
  gateOpen: boolean | null;
  onClose: () => void;
}) {
  const tweaks = useStore((s) => s.tweaks);
  const plan = useMemo(() => planGameBundle(gameId, tweaks), [gameId, tweaks]);
  const busy = useStore((s) => s.applyManyOp.status === "running");
  const { applyMany } = useActions();
  const [read, setRead] = useState(false);
  const tickId = useId();
  const tick = needsTick(plan);
  const ids = plan.toApply.map((t) => t.id);

  return (
    <Dialog
      open
      title={`${name}: ${ids.length} ${ids.length === 1 ? "change" : "changes"} to apply`}
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            busy={busy}
            disabled={ids.length === 0 || gateOpen === false || (tick && !read)}
            onClick={() => {
              void applyMany(ids);
              onClose();
            }}
          >
            Apply {ids.length}
          </Button>
        </>
      }
    >
      <p>Each change is recorded before it is made, and Backups can undo it on its own or all together.</p>
      <ul className="mt-3 flex max-h-[45vh] flex-col gap-2.5 overflow-auto pr-1">
        {plan.toApply.map((t) => (
          <li key={t.id} className="rounded-lg border border-line bg-surface-2 px-3 py-2">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-bold text-ink">{t.name}</span>
              {t.safety !== "safe" && <StatusBadge tone="warn">Advanced</StatusBadge>}
              {t.requiresReboot && <span className="text-xs text-ink-faint">Needs a restart</span>}
            </div>
            {t.tradeoff ? <p className="mt-1">{t.tradeoff}</p> : <p className="mt-1">{t.summary}</p>}
          </li>
        ))}
      </ul>
      {plan.inPlace.length > 0 && (
        <p className="mt-3">
          Already in place: {plan.inPlace.length} ({plan.inPlace.map((t) => t.name).join(", ")}).
        </p>
      )}
      {plan.notHere.length > 0 && (
        <p className="mt-2">Not offered on this PC now: {plan.notHere.map((t) => t.name).join(", ")}. Tools says why for each.</p>
      )}
      {tick && (
        <label htmlFor={tickId} className="mt-4 flex cursor-pointer items-start gap-2 text-ink">
          <input
            id={tickId}
            type="checkbox"
            checked={read}
            onChange={(e) => setRead(e.target.checked)}
            className="mt-0.5 size-4 accent-accent"
          />
          I have read what each Advanced change costs.
        </label>
      )}
      {gateOpen === false && <p className="mt-3 text-ink">Make a restore point first: changes are locked until there is one.</p>}
    </Dialog>
  );
}
