import { Search } from "lucide-react";
import { useEffect, useId, useMemo, useState } from "react";

import type { BlockedCode } from "../../generated/BlockedCode";
import type { CleanupArea } from "../../generated/CleanupArea";
import type { DiskMedia } from "../../generated/DiskMedia";
import type { TweakView } from "../../generated/TweakView";
import { blockedHint, whyBlocked } from "../../lib/blocked";
import { explain } from "../../lib/errors";
import { formatBytes, formatDateTime, formatDuration, probeValue } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { otherLongWork, recommendedIds, type LongWork } from "../../store/store";
import { RestorePointButton, useCanMakeRestorePoint } from "../shell/RestorePointButton";
import { categoryIcon, categoryName } from "./categories";
import { ConnectionSection } from "./ConnectionSection";
import { LastChange } from "./LastChange";
import { MemoryCleaner } from "./MemoryCleaner";
import { PlaySection } from "./PlaySection";
import { PresetsSection } from "./Presets";
import { StartupSection } from "./StartupSection";
import { Button, Callout, Card, cx, Dialog, ErrorCallout, PageHeader, SampleBadge, StatusBadge, type Tone } from "../ui/primitives";

const STATE: Record<TweakView["state"]["status"], { tone: Tone; label: string }> = {
  default: { tone: "neutral", label: "Not applied" },
  applied: { tone: "ok", label: "Optimized" },
  foreign: { tone: "ok", label: "Already optimized" },
  drifted: { tone: "warn", label: "Changed outside PeakTweaks since it was applied" },
  blocked: { tone: "bad", label: "Not available" },
  unknown: { tone: "warn", label: "Could not read its current state" },
};

const TIER_LABEL = { free: null, pro: "Pro", ultimate: "Ultimate" } as const;

/** Reasons a change can never be made on this PC as it is (its hardware, its
 * Windows), as opposed to ones the user can act on. */
const NOT_HERE: readonly BlockedCode[] = ["hardware_unsupported", "os_version_unsupported"];

/** A change this PC cannot take at all: listed apart, folded, with why. */
export function notForThisPc(t: TweakView): boolean {
  return t.state.status === "blocked" && NOT_HERE.includes(t.state.reason.code);
}

/** Does a change match what was typed in Tools' search box? Every word must
 * appear in its name, summary, cost line, category or what it changes. */
export function matchesSearch(t: TweakView, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const text = [t.name, t.summary, t.tradeoff ?? "", t.category, t.target].join(" ").toLowerCase();
  return words.every((w) => text.includes(w));
}

export function ToolsView() {
  const tweaks = useStore((s) => s.tweaks);
  // null until the audit has answered (or if it failed): the engine still
  // checks at apply time, so only a definite "no restore point" locks the UI.
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const [advanced, setAdvanced] = useState(false);
  const [query, setQuery] = useState("");
  const advancedId = useId();
  const searchId = useId();
  const canMake = useCanMakeRestorePoint();

  const groups = useMemo(() => {
    const visible = tweaks.filter((t) => (advanced || t.safety === "safe") && matchesSearch(t, query));
    const byCategory = new Map<string, TweakView[]>();
    for (const t of visible) byCategory.set(t.category, [...(byCategory.get(t.category) ?? []), t]);
    return [...byCategory.entries()].sort(([a], [b]) => a.localeCompare(b));
  }, [tweaks, advanced, query]);
  const hiddenCount = tweaks.filter((t) => t.safety !== "safe").length;
  // Matches the search would show with Advanced on.
  const hiddenMatches = advanced || !query.trim() ? 0 : tweaks.filter((t) => t.safety !== "safe" && matchesSearch(t, query)).length;
  // Settings this PC already has count as done, whoever set them: they are
  // listed, not hidden, so the user sees the whole set. Changes this PC
  // cannot take are not counted.
  const doneCount = tweaks.filter((t) => t.state.status === "applied" || t.state.status === "foreign").length;
  const forThisPc = tweaks.filter((t) => !notForThisPc(t)).length;

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
      <div className="flex max-w-4xl flex-col gap-6 2xl:max-w-none">
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
        <LastChange />
        <Overview done={doneCount} total={forThisPc} tweaks={tweaks} />
        <PresetsSection gateOpen={gateOpen} />
        <section aria-labelledby="quick-tools-title">
          <h2 id="quick-tools-title" className="mb-3 font-display text-xl font-extrabold tracking-tight">
            Quick tools
          </h2>
          <MemoryCleaner />
        </section>
        <div className="sticky top-0 z-10 -mx-1 flex flex-wrap items-center justify-between gap-3 border-b border-line bg-surface-0 px-1 py-2.5">
          <CategoryNav groups={groups} />
          <div className="relative w-full sm:w-72">
            <label htmlFor={searchId} className="sr-only">
              Search the tools
            </label>
            <Search aria-hidden className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-ink-faint" />
            <input
              id={searchId}
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search the tools"
              className="w-full rounded-lg border border-line bg-surface-1 py-2 pr-3 pl-9 text-sm placeholder:text-ink-faint focus:border-violet-soft focus:outline-none"
            />
          </div>
        </div>
        {hiddenMatches > 0 && (
          <p className="text-sm text-ink-muted" role="status">
            {hiddenMatches} more {hiddenMatches === 1 ? "tool matches" : "tools match"} under Advanced.{" "}
            <button type="button" className="font-bold text-ink underline" onClick={() => setAdvanced(true)}>
              Show Advanced
            </button>
          </p>
        )}
        {groups.length === 0 && (
          <p className="text-sm text-ink-muted">
            {query.trim() ? `No tools match "${query.trim()}".` : "No changes are available in this view."}
          </p>
        )}
        {groups.map(([category, list]) => (
          <section key={category} aria-labelledby={`cat-${category}`} className="scroll-mt-16">
            <CategoryHeader category={category} list={list} gateOpen={gateOpen} />
            <ul className="grid items-start gap-3 2xl:grid-cols-2">
              {list
                .filter((t) => !notForThisPc(t))
                .map((t) => (
                  <li key={t.id}>
                    <TweakCard tweak={t} gateOpen={gateOpen} />
                  </li>
                ))}
            </ul>
            <NotForThisPc list={list.filter(notForThisPc)} />
          </section>
        ))}
        {advanced && <DevicesSection gateOpen={gateOpen} />}
        <PlaySection />
        <StartupSection />
        <ConnectionSection />
        <OneTimeActions />
      </div>
    </>
  );
}

/** In effect on this PC, by PeakTweaks or set before. */
const inEffect = (t: TweakView) => t.state.status === "applied" || t.state.status === "foreign";

/** The ring at the top of Tools: how much of the list this PC already has. */
function Overview({ done, total, tweaks }: { done: number; total: number; tweaks: readonly TweakView[] }) {
  if (total === 0) return null;
  const r = 40;
  const length = 2 * Math.PI * r;
  const share = done / total;
  const setBack = tweaks.filter((t) => t.state.status === "drifted").length;
  const toApply = tweaks.filter((t) => t.state.status === "default" && !t.blocked).length;
  return (
    <section
      aria-label="Overview"
      className="relative isolate flex flex-wrap items-center gap-5 overflow-hidden rounded-2xl border border-line bg-surface-1 p-5"
    >
      <div aria-hidden className="pointer-events-none absolute inset-0 -z-10 texture-lines" />
      <div
        aria-hidden
        className="pointer-events-none absolute -bottom-24 -left-10 -z-10 size-64 rounded-full bg-violet/25 blur-3xl"
      />
      <div className="relative size-24 shrink-0">
        <svg viewBox="0 0 100 100" className="size-full -rotate-90" aria-hidden>
          <circle cx="50" cy="50" r={r} fill="none" strokeWidth="10" className="stroke-surface-3" />
          <circle
            cx="50"
            cy="50"
            r={r}
            fill="none"
            strokeWidth="10"
            strokeLinecap="round"
            strokeDasharray={length}
            strokeDashoffset={length * (1 - share)}
            className="stroke-lime transition-[stroke-dashoffset] duration-700"
          />
        </svg>
        <span className="absolute inset-0 flex items-center justify-center font-display text-xl font-extrabold tabular-nums">
          {done}/{total}
        </span>
      </div>
      <div className="min-w-0 flex-1">
        <p className="font-display text-2xl font-extrabold tracking-tight">
          {done} of {total} already optimized on this PC.
        </p>
        <p className="mt-1 text-sm text-ink-muted">
          {toApply === 0 ? "Nothing left to apply in this list." : `${toApply} still to apply.`}
          {setBack > 0 && ` ${setBack} set back outside PeakTweaks.`} A preset below applies a set in one go, or pick
          them one by one further down.
        </p>
      </div>
    </section>
  );
}

/** Jump to a category further down the page. */
function CategoryNav({ groups }: { groups: [string, TweakView[]][] }) {
  if (groups.length < 2) return <span />;
  return (
    <nav aria-label="Tool categories" className="flex min-w-0 flex-1 flex-wrap gap-1.5">
      {groups.map(([category, list]) => {
        const Icon = categoryIcon(category);
        const here = list.filter((t) => !notForThisPc(t));
        const done = here.filter(inEffect).length;
        return (
          <button
            key={category}
            type="button"
            onClick={() =>
              document.getElementById(`cat-${category}`)?.closest("section")?.scrollIntoView?.({ behavior: "smooth" })
            }
            className="flex items-center gap-1.5 rounded-full border border-line bg-surface-1 px-3 py-1.5 text-xs font-bold text-ink-muted transition-colors hover:border-violet hover:text-ink"
          >
            <Icon aria-hidden className="size-3.5" />
            {categoryName(category)}
            <span
              className={cx(
                "rounded-full px-1.5 tabular-nums",
                here.length > 0 && done === here.length ? "bg-lime text-black" : "bg-surface-3 text-ink-muted",
              )}
            >
              {done}/{here.length}
            </span>
          </button>
        );
      })}
    </nav>
  );
}

/** A category's title, how much of it is in effect, and Apply recommended. */
function CategoryHeader({ category, list, gateOpen }: { category: string; list: TweakView[]; gateOpen: boolean | null }) {
  const Icon = categoryIcon(category);
  const here = list.filter((t) => !notForThisPc(t));
  const done = here.filter(inEffect).length;
  return (
    <div className="mb-3 flex flex-wrap items-center gap-3">
      <span
        aria-hidden
        className="flex size-10 shrink-0 items-center justify-center rounded-xl bg-linear-to-br from-violet to-violet-strong text-white shadow-[0_0_24px_-6px] shadow-violet"
      >
        <Icon className="size-5" strokeWidth={2.2} />
      </span>
      <div className="min-w-0 flex-1">
        <h2 id={`cat-${category}`} className="font-display text-xl font-extrabold tracking-tight">
          {categoryName(category)}
        </h2>
        {here.length > 0 && (
          <div className="mt-1 flex items-center gap-2">
            <div aria-hidden className="h-1 w-24 overflow-hidden rounded-full bg-surface-3">
              <div className="h-full rounded-full bg-lime" style={{ width: `${(done / here.length) * 100}%` }} />
            </div>
            <span className="text-xs font-semibold text-ink-muted">
              {done} of {here.length} in effect
            </span>
          </div>
        )}
      </div>
      <ApplyRecommended ids={recommendedIds(list)} gateOpen={gateOpen} />
    </div>
  );
}

/** Catalogue H6: MSI mode for each graphics card and network adapter. One
 * change per device, listed by the engine from what this PC has; Advanced
 * only, read when the section first shows. */
function DevicesSection({ gateOpen }: { gateOpen: boolean | null }) {
  const list = useStore((s) => s.msi);
  const op = useStore((s) => s.msiOp);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { loadMsi } = useActions();

  useEffect(() => {
    if (op.status === "idle") void loadMsi();
  }, [op.status, loadMsi]);

  return (
    <section aria-labelledby="cat-devices">
      <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id="cat-devices" className="text-base font-extrabold tracking-tight">
              Devices
            </h2>
            {sample && <SampleBadge />}
          </div>
          <p className="mt-1 text-sm text-ink-muted">
            This PC's graphics cards and network adapters. Most current drivers use MSI mode already; those are listed as
            already optimized.
          </p>
        </div>
        <Button variant="ghost" busy={op.status === "running"} onClick={() => void loadMsi()}>
          Check again
        </Button>
      </div>
      {!list && op.status === "running" && <p className="text-sm text-ink-muted">Looking for devices…</p>}
      {list && list.devices.length === 0 && !list.problem && (
        <p className="text-sm text-ink-muted">No graphics card or network adapter here can take this change.</p>
      )}
      {list && list.devices.length > 0 && (
        <ul className="grid items-start gap-3 2xl:grid-cols-2">
          {list.devices.map((d) => (
            <li key={d.tweak.id}>
              <TweakCard tweak={d.tweak} gateOpen={gateOpen} />
            </li>
          ))}
        </ul>
      )}
      {list?.problem && (
        <Callout tone="warn" title="The devices could not be listed.">
          {list.problem}
        </Callout>
      )}
      {op.status === "failed" && <ErrorCallout text={explain(op.error)} technical={technical} />}
    </section>
  );
}

/** The group's changes this PC cannot take, folded into one line, each with
 * the engine's reason. */
function NotForThisPc({ list }: { list: TweakView[] }) {
  if (list.length === 0) return null;
  return (
    <details className="mt-3 text-sm text-ink-muted">
      <summary className="cursor-pointer">
        {list.length === 1 ? "1 change does not" : `${list.length} changes do not`} apply to this PC
      </summary>
      <ul className="mt-2 flex flex-col gap-2 pl-4">
        {list.map((t) => (
          <li key={t.id}>
            <span className="font-bold text-ink">{t.name}</span>
            {t.state.status === "blocked" && `: ${t.state.reason.message}`}
          </li>
        ))}
      </ul>
    </details>
  );
}

/** Actions that change no setting: no restore point needed, nothing to undo. */
function OneTimeActions() {
  return (
    <section aria-labelledby="cat-one-time">
      <div className="mb-3">
        <h2 id="cat-one-time" className="text-base font-extrabold tracking-tight">
          One-time actions
        </h2>
        <p className="mt-1 text-sm text-ink-muted">These change no setting, so there is nothing to undo.</p>
      </div>
      <ul className="grid items-start gap-3">
        {/* Clean memory is under Quick tools, at the top. */}
        <li>
          <DriveCard />
        </li>
        <li>
          <CleanupCard />
        </li>
      </ul>
    </section>
  );
}

/** Why a long one-time action waits: the engine runs these one at a time. */
const WAIT_FOR: Record<LongWork, string> = {
  proof: "Available again when the Proof recording finishes.",
  cleanup: "Available again when the junk cleanup finishes.",
  drive: "Available again when the drive optimization finishes.",
};

/** Catalogue H28, in the engine's order. */
const AREAS: { area: CleanupArea; name: string; note: string }[] = [
  {
    area: "user_temp",
    name: "Your temporary files",
    note: "Anything created or changed in the last 7 days is kept, in case a program or an installer still needs it.",
  },
  { area: "windows_temp", name: "Windows temporary files", note: "The same 7-day rule applies." },
  { area: "thumbnails", name: "Thumbnail cache", note: "File Explorer makes these previews again when you open a folder." },
  {
    area: "shader_caches",
    name: "Shader caches",
    note: "Games and the graphics driver build these again, so a game can stutter for a while the first time it runs afterwards.",
  },
  {
    area: "crash_dumps",
    name: "Crash reports and memory dumps",
    note: "What Windows saves when a program or the PC crashes. Keep them if someone is looking into a crash.",
  },
];

/** Shader caches start unselected: clearing them has a cost the next time a game runs. */
const START_SELECTED: ReadonlySet<CleanupArea> = new Set(["user_temp", "windows_temp", "thumbnails", "crash_dumps"]);

/** Catalogue H28: sizes first, one confirmation, cannot be undone. Refused by
 * the engine while a Proof recording runs. */
function CleanupCard() {
  const sizesOp = useStore((s) => s.cleanupSizesOp);
  const op = useStore((s) => s.cleanupOp);
  const capturing = useStore((s) => s.proof.capturingSession !== null);
  const waiting = useStore((s) => otherLongWork(s, "cleanup"));
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { measureCleanup, runCleanup } = useActions();
  const headingId = useId();
  const [selected, setSelected] = useState<ReadonlySet<CleanupArea>>(START_SELECTED);
  const [confirming, setConfirming] = useState(false);

  // Look once when the card first shows; "Check again" looks again.
  useEffect(() => {
    if (sizesOp.status === "idle" && !capturing) void measureCleanup();
  }, [sizesOp.status, capturing, measureCleanup]);

  const sizes = sizesOp.status === "done" ? sizesOp.value : null;
  const checking = sizesOp.status === "running";
  const running = op.status === "running";
  const chosen = AREAS.filter((a) => selected.has(a.area));
  const total = (sizes ?? []).filter((s) => selected.has(s.area)).reduce((sum, s) => sum + s.bytes, 0);
  const report = op.status === "done" ? op.value : null;
  const removed = report?.areas.reduce((sum, a) => sum + a.removedBytes, 0) ?? 0;
  const removedFiles = report?.areas.reduce((sum, a) => sum + a.removedFiles, 0) ?? 0;
  const leftFiles = report?.areas.reduce((sum, a) => sum + a.leftFiles, 0) ?? 0;
  const examples = report?.areas.flatMap((a) => a.leftExamples) ?? [];

  const toggle = (area: CleanupArea, on: boolean) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (on) next.add(area);
      else next.delete(area);
      return next;
    });

  return (
    <Card className="p-4" aria-labelledby={headingId}>
      <div className="flex flex-wrap items-center gap-2">
        <h3 id={headingId} className="font-bold">
          Clear out junk files
        </h3>
        {sample && <SampleBadge />}
      </div>
      <p className="mt-1 text-sm text-ink-muted">
        Temporary files, caches and crash reports that Windows and programs leave behind. Deleting them cannot be undone.
        Files a program has open are left where they are.
      </p>
      {waiting && <p className="mt-2 text-sm text-ink-muted">{WAIT_FOR[waiting]}</p>}

      <fieldset className="mt-3" disabled={running}>
        <legend className="sr-only">What to clear</legend>
        <ul className="grid items-start gap-3 2xl:grid-cols-2">
          {AREAS.map(({ area, name, note }) => {
            const size = sizes?.find((s) => s.area === area);
            return (
              <li key={area}>
                <label className="flex cursor-pointer items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={selected.has(area)}
                    onChange={(e) => toggle(area, e.target.checked)}
                    className="mt-0.5 size-4 shrink-0 accent-accent"
                  />
                  <span className="min-w-0 flex-1">
                    <span className="flex flex-wrap justify-between gap-x-3">
                      <span className="font-bold">{name}</span>
                      <span className="text-ink-muted">
                        {size ? (size.files === 0 ? "Nothing to clear" : formatBytes(size.bytes)) : checking ? "Checking…" : ""}
                      </span>
                    </span>
                    <span className="mt-0.5 block text-xs text-ink-faint">{note}</span>
                    {size && size.skipped.length > 0 && (
                      <span className="mt-0.5 block break-all text-xs text-warn">
                        {technical ? size.skipped.join(" ") : "PeakTweaks could not look in every folder here, so those were left alone."}
                      </span>
                    )}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
      </fieldset>

      {report && (
        <div className="mt-3 text-sm" role="status">
          <p>
            Cleared {formatDateTime(report.unixMs)}. Deleted {formatBytes(removed)} in {removedFiles.toLocaleString()}{" "}
            {removedFiles === 1 ? "file" : "files"}.
          </p>
          {leftFiles > 0 && (
            <p className="mt-1 text-ink-muted">
              {leftFiles.toLocaleString()} {leftFiles === 1 ? "file was" : "files were"} left: a program has them open, or
              Windows would not allow it.
            </p>
          )}
          {technical && examples.length > 0 && (
            <ul className="mt-1 break-all font-mono text-xs text-ink-faint">
              {examples.map((x) => (
                <li key={x}>{x}</li>
              ))}
            </ul>
          )}
        </div>
      )}

      <div className="mt-3 flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-ink-muted">{sizes ? `Selected: ${formatBytes(total)}` : ""}</p>
        <div className="flex gap-2">
          <Button variant="ghost" busy={checking} disabled={capturing || running} onClick={() => void measureCleanup()}>
            Check again
          </Button>
          <Button
            variant="danger"
            busy={running}
            disabled={waiting !== null || checking || !sizes || total === 0}
            onClick={() => setConfirming(true)}
          >
            Delete selected files
          </Button>
        </div>
      </div>
      {sizesOp.status === "failed" && (
        <div className="mt-3">
          <ErrorCallout text={explain(sizesOp.error)} technical={technical} />
        </div>
      )}
      {op.status === "failed" && (
        <div className="mt-3">
          <ErrorCallout text={explain(op.error)} technical={technical} />
        </div>
      )}

      <Dialog
        open={confirming}
        title="Delete these files?"
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
                void runCleanup(chosen.map((a) => a.area));
              }}
            >
              Delete {formatBytes(total)}
            </Button>
          </>
        }
      >
        <p>PeakTweaks deletes {formatBytes(total)} from:</p>
        <ul className="mt-1 list-disc pl-5">
          {chosen.map((a) => (
            <li key={a.area}>{a.name}</li>
          ))}
        </ul>
        <p className="mt-2">
          This cannot be undone, and a restore point does not bring these files back. Files a program has open are left where
          they are.
        </p>
      </Dialog>
    </Card>
  );
}

/** What Windows' optimization does on the Windows drive (VERIFY, NOTES N71). */
function driveJob(media: DiskMedia | null): string {
  switch (media) {
    case "ssd":
      return "The Windows drive is an SSD, so Windows tells it which space is no longer in use (TRIM). This is usually quick.";
    case "hdd":
      return "The Windows drive is a hard drive, so Windows puts the pieces of each file back together (defragmenting). This can take an hour or more, and the PC can be used meanwhile.";
    default:
      return "On an SSD, Windows tells the drive which space is no longer in use (TRIM). On a hard drive it puts the pieces of each file back together (defragmenting), which can take an hour or more.";
  }
}

/** Catalogue H29: Windows' own drive optimization, now. The engine runs it
 * one long action at a time, never during a Proof recording. */
function DriveCard() {
  const op = useStore((s) => s.driveOp);
  const media = useStore((s) => probeValue(s.audit?.env.hardware?.bootDisk)?.media ?? null);
  const waiting = useStore((s) => otherLongWork(s, "drive"));
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { optimizeDrive } = useActions();
  const headingId = useId();
  const running = op.status === "running";
  const result = op.status === "done" ? op.value : null;

  return (
    <Card className="p-4" aria-labelledby={headingId}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 id={headingId} className="font-bold">
              Optimize the Windows drive
            </h3>
            {sample && <SampleBadge />}
          </div>
          <p className="mt-1 text-sm text-ink-muted">
            Windows normally does this on a schedule; this runs it now. {driveJob(media)}
          </p>
          {waiting && <p className="mt-2 text-sm text-ink-muted">{WAIT_FOR[waiting]}</p>}
          {running && (
            <p className="mt-2 text-sm text-ink-muted">Windows is working on it. The result shows here when it finishes.</p>
          )}
          {result && (
            <p className="mt-2 text-sm" role="status">
              Optimized {formatDateTime(result.unixMs)} (drive {result.drive}). Windows took {formatDuration(result.seconds)}.
            </p>
          )}
        </div>
        <Button busy={running} disabled={waiting !== null} onClick={() => void optimizeDrive()}>
          Optimize now
        </Button>
      </div>
      {technical && result && result.report.length > 0 && (
        <pre className="mt-3 overflow-x-auto whitespace-pre-wrap break-all font-mono text-xs text-ink-faint">
          {result.report.join("\n")}
        </pre>
      )}
      {op.status === "failed" && (
        <div className="mt-3">
          <ErrorCallout text={explain(op.error)} technical={technical} />
        </div>
      )}
    </Card>
  );
}

/** One click for a category's recommended changes (DECISIONS 15.22). */
function ApplyRecommended({ ids, gateOpen }: { ids: string[]; gateOpen: boolean | null }) {
  const busy = useStore((s) => s.applyManyOp.status === "running");
  const { applyMany } = useActions();
  if (ids.length === 0) return null;
  return (
    <Button busy={busy} disabled={gateOpen === false} onClick={() => void applyMany(ids)}>
      Apply recommended ({ids.length})
    </Button>
  );
}

/** One tool with its state, Apply and Undo. Also shown under a scan finding
 * that names it as the fix (Home), so both places share the same rules. */
export function TweakCard({ tweak, gateOpen }: { tweak: TweakView; gateOpen: boolean | null }) {
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
  // DECISIONS 15.22: a safe change's cost is one line; only Advanced changes
  // ask for a confirmation.
  const needsConfirm = !!tweak.tradeoff && tweak.safety !== "safe";
  // Our apply is still on record but Windows has another value now: both
  // directions stay open (set ours again, or put back what was there before).
  const drifted = tweak.state.status === "drifted";
  // Why the engine does not offer it: what this PC has (no such adapter, say),
  // which only reading its state shows, else its plan or its rules.
  const blocked = whyBlocked(tweak);
  const canApply =
    !applied && !blocked && gateOpen !== false && tweak.state.status !== "unknown" && (!needsConfirm || acknowledged);
  const tier = TIER_LABEL[tweak.tier];
  // DECISIONS 15.22, fewer warnings: the restart is said once. Not for a
  // setting this PC already has, nor when the cost line shown says it.
  const tradeoffShown = !!tweak.tradeoff && !foreign && (!needsConfirm || !applied);
  const restartLine =
    tweak.requiresReboot && !foreign && !(tradeoffShown && /needs a restart/i.test(tweak.tradeoff ?? ""));

  const Icon = categoryIcon(tweak.category);
  const inPlace = applied || foreign;
  return (
    <Card
      className={cx(
        "relative overflow-hidden p-4 pl-5 transition-colors",
        inPlace ? "border-lime/30" : blocked ? "border-line" : "hover:border-violet/60",
      )}
    >
      {/* The state at a glance, beside its word: lime in effect, violet to apply, red not offered. */}
      <span
        aria-hidden
        className={cx(
          "absolute inset-y-0 left-0 w-1",
          inPlace ? "bg-lime" : blocked || tweak.state.status === "blocked" ? "bg-bad/70" : drifted ? "bg-violet-soft" : "bg-violet",
        )}
      />
      <div className="flex flex-wrap items-start justify-between gap-3">
        <span
          aria-hidden
          className={cx(
            "flex size-9 shrink-0 items-center justify-center rounded-lg",
            inPlace ? "bg-lime/15 text-lime" : "bg-violet/15 text-violet-soft",
          )}
        >
          <Icon className="size-[18px]" strokeWidth={2} />
        </span>
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
          {blocked && (
            <p className="mt-2 text-sm">
              {blocked.message} <span className="text-ink-muted">{blockedHint(blocked)}</span>
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
          {restartLine && <p className="mt-2 text-xs text-ink-faint">Takes effect after a restart.</p>}
          {technical ? (
            <p className="mt-2 break-all font-mono text-xs text-ink-faint">{tweak.target}</p>
          ) : (
            <details className="mt-2 text-xs text-ink-muted">
              <summary className="cursor-pointer font-semibold">What this changes</summary>
              <p className="mt-1 break-all font-mono text-ink-faint">{tweak.target}</p>
            </details>
          )}
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

      {tweak.tradeoff && !needsConfirm && !foreign && <p className="mt-2 text-sm text-ink-muted">{tweak.tradeoff}</p>}
      {needsConfirm && !applied && !foreign && (
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
