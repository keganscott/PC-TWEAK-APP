import { useCallback, useState, type ReactNode } from "react";
import { ChartColumn, Gamepad2, History, LayoutGrid, Settings as SettingsIcon, SlidersHorizontal } from "lucide-react";

import { explain } from "../../lib/errors";
import type { State } from "../../store/store";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Logo } from "../brand/Logo";
import { Button, cx, ErrorCallout } from "../ui/primitives";
import { ExecutionBus } from "./ExecutionBus";
import { NavContext, VIEWS, type ViewId } from "./nav";
import { ProtectedCard } from "./ProtectedCard";
import { TopBar } from "./TopBar";

const ICON: Record<ViewId, typeof LayoutGrid> = {
  home: LayoutGrid,
  games: Gamepad2,
  tools: SlidersHorizontal,
  proof: ChartColumn,
  backups: History,
};

/** The small count next to a view, from what the engine reported; 0 shows nothing. */
const COUNT: Partial<Record<ViewId, { select: (s: State) => number; describe: (n: number) => string }>> = {
  home: {
    select: (s) => s.audit?.scan.findings.filter((f) => f.status === "attention").length ?? 0,
    describe: (n) => `${n} worth a look`,
  },
  games: { select: (s) => s.audit?.env.gameInstalls?.length ?? 0, describe: (n) => `${n} found on this PC` },
  backups: { select: (s) => s.journal?.applied.length ?? 0, describe: (n) => `${n} applied` },
};

export function AppShell({
  view,
  onNavigate,
  onOpenSettings,
  children,
}: {
  view: ViewId;
  onNavigate: (v: ViewId) => void;
  onOpenSettings: () => void;
  children: ReactNode;
}) {
  const [busOpen, setBusOpen] = useState(false);
  const toggleBus = useCallback(() => setBusOpen((o) => !o), []);
  return (
    <NavContext.Provider value={onNavigate}>
      <div className="flex h-full print:block print:h-auto">
        <nav aria-label="Main" className="scrollbar-quiet flex w-60 shrink-0 flex-col overflow-y-auto border-r border-line bg-black px-4 pt-6 pb-4">
          <div className="px-2">
            <Logo />
          </div>
          {(["Overview", "Optimise"] as const).map((group) => (
            <div key={group} className="mt-8">
              <h2 className="px-2.5 pb-2 text-[10.5px] font-bold tracking-[0.14em] text-ink-faint uppercase">{group}</h2>
              <ul className="flex flex-col gap-1">
                {VIEWS.filter((v) => v.group === group).map(({ id, label }) => (
                  <NavItem key={id} id={id} label={label} active={id === view} onNavigate={onNavigate} />
                ))}
              </ul>
            </div>
          ))}
          <div className="mt-1">
            <button
              type="button"
              onClick={onOpenSettings}
              className="flex h-10 w-full items-center gap-3 rounded-lg px-2.5 text-sm font-semibold text-ink-muted hover:bg-surface-2 hover:text-ink"
            >
              <SettingsIcon aria-hidden className="size-[18px]" strokeWidth={1.8} />
              Settings
            </button>
          </div>
          <div className="mt-auto pt-6">
            <ProtectedCard />
          </div>
        </nav>
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar busOpen={busOpen} onToggleBus={toggleBus} />
          {/* `relative` makes this the box hidden (`sr-only`) labels are
              placed in. Without it they are placed against the page, so a
              label far down a long view made the whole page taller than the
              window and the page itself scrolled, cutting off the top bar
              and leaving black space at the bottom of a maximised window. */}
          <main className="scrollbar-quiet relative min-h-0 flex-1 overflow-y-auto px-9 pt-2 pb-10 print:overflow-visible print:p-0">
            {/* Centred, and wide enough to use a maximised window on a large
                screen; each view widens at 2xl rather than staying a narrow
                column with black beside it. */}
            <div className="mx-auto w-full max-w-[1600px]">
              <RefreshBanner />
              {children}
            </div>
          </main>
          <ExecutionBus open={busOpen} />
        </div>
      </div>
    </NavContext.Provider>
  );
}

function NavItem({
  id,
  label,
  active,
  onNavigate,
}: {
  id: ViewId;
  label: string;
  active: boolean;
  onNavigate: (v: ViewId) => void;
}) {
  const Icon = ICON[id];
  const count = COUNT[id];
  const n = useStore(count?.select ?? zero);
  const descId = `nav-count-${id}`;
  return (
    <li className="relative">
      {active && <span aria-hidden className="absolute top-2.5 bottom-2.5 -left-4 w-[3px] rounded-r-sm bg-violet" />}
      {/* The count is drawn from data-count by CSS, so the button's name stays
          just the view's name; screen readers get it as the description. */}
      <button
        type="button"
        onClick={() => onNavigate(id)}
        aria-current={active ? "page" : undefined}
        aria-describedby={n > 0 ? descId : undefined}
        data-count={n > 0 ? String(n) : undefined}
        className={cx(
          "flex h-10 w-full items-center gap-3 rounded-lg px-2.5 text-sm font-semibold",
          "data-count:after:ml-auto data-count:after:grid data-count:after:h-5 data-count:after:min-w-5 data-count:after:place-items-center data-count:after:rounded-md data-count:after:px-1.5 data-count:after:text-[11px] data-count:after:font-bold data-count:after:content-[attr(data-count)]",
          active
            ? "bg-surface-2 text-ink data-count:after:bg-violet data-count:after:text-white"
            : "text-ink-muted hover:bg-surface-2 hover:text-ink data-count:after:bg-surface-3 data-count:after:text-ink-muted",
        )}
      >
        <Icon aria-hidden className={cx("size-[18px]", active && "text-lime")} strokeWidth={1.8} />
        {label}
      </button>
      {n > 0 && count && (
        <span id={descId} className="sr-only">
          {count.describe(n)}
        </span>
      )}
    </li>
  );
}

const zero = () => 0;

/** A re-read after a change failed: say that what is shown may be out of date. */
function RefreshBanner() {
  const error = useStore((s) => s.refreshError);
  const technical = useTechnical();
  const { rescan } = useActions();
  if (!error) return null;
  const text = explain(error);
  return (
    <div className="mb-5 max-w-4xl">
      <ErrorCallout
        text={{ ...text, title: `Some of what is shown may be out of date. ${text.title}` }}
        technical={technical}
        action={<Button onClick={() => void rescan()}>Check again</Button>}
      />
    </div>
  );
}
