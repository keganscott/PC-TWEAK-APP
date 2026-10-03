import { PanelBottomOpen, Settings as SettingsIcon } from "lucide-react";

import { useStore } from "../../store/hooks";
import { SampleBadge, StatusBadge } from "../ui/primitives";

export function TopBar({ onOpenSettings, busOpen, onToggleBus }: { onOpenSettings: () => void; busOpen: boolean; onToggleBus: () => void }) {
  const sample = useStore((s) => s.sample);
  const context = useStore((s) => s.context);
  const audit = useStore((s) => s.audit);
  const gateOpen = audit?.env.restoreGateOpen ?? null;

  return (
    <header className="flex h-14 items-center justify-between gap-4 border-b border-line bg-surface-1 px-5 print:hidden">
      <div className="flex items-center gap-3">
        {sample && (
          <span className="flex items-center gap-2 text-xs text-sample">
            <SampleBadge />
            <span>Demo data, not this PC</span>
          </span>
        )}
      </div>
      <div className="flex items-center gap-2">
        {context && !context.elevated && <StatusBadge tone="bad">Not running as administrator</StatusBadge>}
        {gateOpen === true && (
<StatusBadge tone="ok">Restore point ready</StatusBadge>
        )}
        {gateOpen === false && (
<StatusBadge tone="warn">No restore point yet</StatusBadge>
        )}
        <button
          type="button"
          onClick={onToggleBus}
          aria-pressed={busOpen}
          aria-controls="execution-bus"
          className="rounded-md p-2 text-ink-muted hover:bg-surface-2 hover:text-ink"
          title="Activity log"
        >
          <PanelBottomOpen aria-hidden className="size-5" />
          <span className="sr-only">Activity log</span>
        </button>
        <button
          type="button"
          onClick={onOpenSettings}
          className="rounded-md p-2 text-ink-muted hover:bg-surface-2 hover:text-ink"
          title="Settings"
        >
          <SettingsIcon aria-hidden className="size-5" />
          <span className="sr-only">Settings</span>
        </button>
      </div>
    </header>
  );
}
