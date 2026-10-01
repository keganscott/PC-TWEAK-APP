import { useCallback, useState, type ReactNode } from "react";
import { Activity, Gamepad2, Home, LifeBuoy, Wrench } from "lucide-react";

import { explain } from "../../lib/errors";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, cx, ErrorCallout } from "../ui/primitives";
import { ExecutionBus } from "./ExecutionBus";
import { NavContext, VIEWS, type ViewId } from "./nav";
import { TopBar } from "./TopBar";

const ICON: Record<ViewId, typeof Home> = { home: Home, games: Gamepad2, tools: Wrench, proof: Activity, backups: LifeBuoy };

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
      <div className="flex h-full">
        <nav aria-label="Main" className="flex w-52 shrink-0 flex-col border-r border-line bg-surface-1">
          <div className="flex h-14 items-center gap-2 border-b border-line px-5">
            <span aria-hidden className="size-2.5 rounded-full bg-accent" />
            <span className="text-base font-semibold tracking-tight">PeakTweaks</span>
          </div>
          <ul className="flex flex-1 flex-col gap-1 p-3">
            {VIEWS.map(({ id, label }) => {
              const Icon = ICON[id];
              const active = id === view;
              return (
                <li key={id}>
                  <button
                    type="button"
                    onClick={() => onNavigate(id)}
                    aria-current={active ? "page" : undefined}
                    className={cx(
                      "flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm",
                      active ? "bg-surface-3 font-medium text-ink" : "text-ink-muted hover:bg-surface-2 hover:text-ink",
                    )}
                  >
                    <Icon aria-hidden className="size-4" />
                    {label}
                  </button>
                </li>
              );
            })}
          </ul>
        </nav>
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar onOpenSettings={onOpenSettings} busOpen={busOpen} onToggleBus={toggleBus} />
          <main className="min-h-0 flex-1 overflow-y-auto px-8 py-7">
            <RefreshBanner />
            {children}
          </main>
          <ExecutionBus open={busOpen} />
        </div>
      </div>
    </NavContext.Provider>
  );
}

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
