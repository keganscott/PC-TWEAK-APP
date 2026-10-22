import { Crosshair, EyeOff, ShieldCheck, type LucideIcon } from "lucide-react";
import { useId, useMemo, useState } from "react";

import type { TweakView } from "../../generated/TweakView";
import { needsTick, planPreset, PRESETS, type Preset, type PresetId } from "../../lib/presets";
import { useActions, useStore } from "../../store/hooks";
import { Button, cx, Dialog, SampleBadge, StatusBadge } from "../ui/primitives";

const ICON: Record<PresetId, LucideIcon> = { basics: ShieldCheck, competitive: Crosshair, privacy: EyeOff };

/** Tools' presets: a card each, and a review before anything is applied. */
export function PresetsSection({ gateOpen }: { gateOpen: boolean | null }) {
  const tweaks = useStore((s) => s.tweaks);
  const sample = useStore((s) => s.sample);
  const [open, setOpen] = useState<PresetId | null>(null);
  const reviewing = PRESETS.find((p) => p.id === open) ?? null;

  return (
    <section aria-labelledby="presets-title">
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <h2 id="presets-title" className="font-display text-xl font-extrabold tracking-tight">
          Presets
        </h2>
        {sample && <SampleBadge />}
        <p className="w-full text-sm text-ink-muted">
          A set of changes in one go. Each one is listed with what it costs before anything is applied, and each can be undone on
          its own from Backups.
        </p>
      </div>
      <ul className="grid gap-3 md:grid-cols-3">
        {PRESETS.map((p) => (
          <li key={p.id}>
            <PresetCard preset={p} tweaks={tweaks} onReview={() => setOpen(p.id)} />
          </li>
        ))}
      </ul>
      {reviewing && <PresetReview preset={reviewing} tweaks={tweaks} gateOpen={gateOpen} onClose={() => setOpen(null)} />}
    </section>
  );
}

function PresetCard({ preset, tweaks, onReview }: { preset: Preset; tweaks: readonly TweakView[]; onReview: () => void }) {
  const plan = useMemo(() => planPreset(preset, tweaks), [preset, tweaks]);
  const Icon = ICON[preset.id];
  const total = plan.toApply.length + plan.inPlace.length;
  const done = total > 0 && plan.toApply.length === 0;
  const headingId = useId();
  return (
    <article
      aria-labelledby={headingId}
      className={cx(
        "group relative isolate flex h-full flex-col overflow-hidden rounded-2xl border bg-surface-1 p-4 transition-colors",
        done ? "border-lime/50" : "border-line hover:border-violet/70",
      )}
    >
      <div
        aria-hidden
        className="pointer-events-none absolute -top-10 -right-10 -z-10 size-36 rounded-full bg-violet/25 blur-2xl transition-opacity group-hover:opacity-100 sm:opacity-60"
      />
      <div className="flex items-center gap-3">
        <span
          aria-hidden
          className={cx(
            "flex size-10 shrink-0 items-center justify-center rounded-xl",
            done ? "bg-lime text-black" : "bg-linear-to-br from-violet to-violet-strong text-white",
          )}
        >
          <Icon className="size-5" strokeWidth={2.2} />
        </span>
        <h3 id={headingId} className="font-display text-lg font-extrabold tracking-tight">
          {preset.name}
        </h3>
        {done && <StatusBadge tone="ok">In place</StatusBadge>}
      </div>
      <p className="mt-2 flex-1 text-sm text-ink-muted">{preset.blurb}</p>
      <div className="mt-3">
        <div className="flex items-baseline justify-between text-xs font-semibold text-ink-muted">
          <span>{total === 0 ? "None offered on this PC" : `${plan.inPlace.length} of ${total} in place`}</span>
          {plan.notHere.length > 0 && <span>{plan.notHere.length} not offered here</span>}
        </div>
        <div aria-hidden className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-surface-3">
          <div
            className="h-full rounded-full bg-lime transition-[width] duration-500"
            style={{ width: `${total ? (plan.inPlace.length / total) * 100 : 0}%` }}
          />
        </div>
      </div>
      <Button className="mt-4" variant={done ? "secondary" : "primary"} disabled={plan.toApply.length === 0} onClick={onReview}>
        {plan.toApply.length === 0
          ? "Nothing to apply"
          : `Review ${plan.toApply.length} ${plan.toApply.length === 1 ? "change" : "changes"}`}
      </Button>
    </article>
  );
}

function PresetReview({
  preset,
  tweaks,
  gateOpen,
  onClose,
}: {
  preset: Preset;
  tweaks: readonly TweakView[];
  gateOpen: boolean | null;
  onClose: () => void;
}) {
  const plan = useMemo(() => planPreset(preset, tweaks), [preset, tweaks]);
  const busy = useStore((s) => s.applyManyOp.status === "running");
  const { applyMany } = useActions();
  const [read, setRead] = useState(false);
  const tickId = useId();
  const tick = needsTick(plan);
  const ids = plan.toApply.map((t) => t.id);

  return (
    <Dialog
      open
      title={`${preset.name}: ${ids.length} ${ids.length === 1 ? "change" : "changes"} to apply`}
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
