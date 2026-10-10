import { PanelBottomOpen } from "lucide-react";

import { useActions, useStore } from "../../store/hooks";
import { SampleBadge, StatusBadge } from "../ui/primitives";
import { GamingModeToggle } from "./GamingModeButton";
import { useNavigate } from "./nav";

/** While Proof records, by hand or by itself during a game: a pulsing pill
 * that opens Proof on that comparison. */
function ProofRecording() {
  const auto = useStore((s) => (s.play?.autoRecord?.recordingNow ? s.play.autoRecord : null));
  const manual = useStore((s) => s.proof.capturingSession);
  const { showComparison } = useActions();
  const navigate = useNavigate();
  const sessionId = auto?.sessionId ?? manual;
  if (!sessionId) return null;
  const label = auto ? `Proof recording sample ${auto.recorded + 1} of ${auto.wanted}` : "Proof recording";
  return (
    <button
      type="button"
      onClick={() => {
        showComparison(sessionId);
        navigate("proof");
      }}
      title="Open Proof on this comparison"
      className="flex items-center gap-2 rounded-full border border-bad/60 bg-bad/10 py-1.5 pr-3 pl-2.5 text-xs font-bold text-ink hover:border-bad"
    >
      <span aria-hidden className="relative flex size-2.5">
        <span className="absolute inline-flex size-full rounded-full bg-bad opacity-75 motion-safe:animate-ping" />
        <span className="relative inline-flex size-2.5 rounded-full bg-bad" />
      </span>
      <span>{label}</span>
    </button>
  );
}

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
        <ProofRecording />
        <GamingModeToggle />
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
