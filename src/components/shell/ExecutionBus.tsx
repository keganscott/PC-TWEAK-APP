import { useState } from "react";

import type { BusEntry } from "../../store/store";
import { useStore, useActions } from "../../store/hooks";
import { Button } from "../ui/primitives";

const time = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" });

/** The log as text, for pasting into a bug report; build first. */
export function busText(bus: readonly BusEntry[], build: string = __APP_BUILD__): string {
  return [
    `PeakTweaks ${build}`,
    ...bus.map((e) => `${new Date(e.at).toISOString()} ${e.stage}${e.tweakId ? ` [${e.tweakId}]` : ""} ${e.message}`),
  ].join("\n");
}

/** The engine's own progress messages, newest last. Closed by default (plan section 7). */
export function ExecutionBus({ open }: { open: boolean }) {
  const bus = useStore((s) => s.bus);
  const { clearBus } = useActions();
  const [copied, setCopied] = useState<"yes" | "no" | null>(null);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(busText(bus));
      setCopied("yes");
    } catch {
      setCopied("no");
    }
  };
  // Always in the DOM (hidden when closed) so the toggle's aria-controls points at it.
  return (
    <section id="execution-bus" aria-label="Activity log" hidden={!open} className="h-48 shrink-0 border-t border-line bg-surface-1 print:hidden">
      <div className="flex items-center justify-between px-5 py-2">
        <h2 className="text-[10.5px] font-bold tracking-[0.14em] text-ink-faint uppercase">Activity log</h2>
        <div className="flex items-center gap-1">
          {copied && (
            <span role="status" className="text-xs text-ink-faint">
              {copied === "yes" ? "Copied" : "Could not copy here"}
            </span>
          )}
          <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => void copy()} disabled={bus.length === 0}>
            Copy
          </Button>
          <Button
            variant="ghost"
            className="px-2 py-1 text-xs"
            onClick={() => {
              setCopied(null);
              clearBus();
            }}
            disabled={bus.length === 0}
          >
            Clear
          </Button>
        </div>
      </div>
      <ol aria-live="polite" className="scrollbar-quiet h-36 overflow-y-auto px-5 pb-3 font-mono text-xs text-ink-muted">
        {bus.length === 0 && <li className="text-ink-faint">Nothing yet. Changes and scans report here as they run.</li>}
        {bus.map((e) => (
          <li key={e.id} className="py-0.5">
            <span className="text-ink-faint">{time.format(e.at)}</span> <span className="text-violet-soft">{e.stage}</span>
            {e.tweakId && <span className="text-ink-faint"> [{e.tweakId}]</span>} {e.message}
          </li>
        ))}
      </ol>
    </section>
  );
}
