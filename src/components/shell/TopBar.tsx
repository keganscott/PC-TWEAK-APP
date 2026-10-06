import { PanelBottomOpen } from "lucide-react";

import { useStore } from "../../store/hooks";
import { SampleBadge, StatusBadge } from "../ui/primitives";

export function TopBar({ busOpen, onToggleBus }: { busOpen: boolean; onToggleBus: () => void }) {
  const sample = useStore((s) => s.sample);
  const context = useStore((s) => s.context);

  return (
    <header className="flex h-14 shrink-0 items-center justify-between gap-4 px-9 print:hidden">
      <div className="flex items-center gap-3">
        {context?.testerBuild && (
          <span className="flex items-center gap-2 text-xs font-semibold text-ink-muted">
            <span className="rounded-md border border-violet bg-violet px-1.5 py-0.5 text-[10px] font-bold tracking-wider text-white uppercase">
              Tester build
            </span>
            <span>Every plan is unlocked for testing. Not a release.</span>
          </span>
        )}
        {sample && (
          <span className="flex items-center gap-2 text-xs font-semibold text-sample">
            <SampleBadge />
            <span>Demo data, not this PC</span>
          </span>
        )}
      </div>
      <div className="flex items-center gap-2">
        {context && !context.elevated && <StatusBadge tone="bad">Not running as administrator</StatusBadge>}
        <button
          type="button"
          onClick={onToggleBus}
          aria-pressed={busOpen}
          aria-controls="execution-bus"
          className="flex items-center gap-2 rounded-lg px-2.5 py-1.5 text-xs font-semibold text-ink-muted hover:bg-surface-2 hover:text-ink aria-pressed:bg-surface-2 aria-pressed:text-ink"
          title="Activity log"
        >
          <PanelBottomOpen aria-hidden className="size-4" />
          <span>Activity log</span>
        </button>
      </div>
    </header>
  );
}
