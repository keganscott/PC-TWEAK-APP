import type { ReactNode } from "react";

import { explain } from "../../lib/errors";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import type { BusEntry, State } from "../../store/store";
import { Button, ErrorCallout } from "../ui/primitives";

/** The newest restore progress message of the current attempt, or null. Returns
 * an entry already in state, so the selector is stable. */
export function currentRestoreStage(s: State): BusEntry | null {
  for (let i = s.bus.length - 1; i >= 0; i -= 1) {
    const e = s.bus[i]!;
    if (e.id <= s.restoreSinceBusId) break;
    if (e.stage.startsWith("restore_")) return e;
  }
  return null;
}

/** Whether this PC can make a restore point at all (false: no System Restore,
 * or policy turns it off). Null until the audit has answered. */
export function useCanMakeRestorePoint(): boolean | null {
  return useStore((s) => {
    const restore = s.audit?.env.restore;
    if (!restore) return null;
    return restore.supported.state !== "no" && !restore.disabledByPolicy;
  });
}

/**
 * One click: the engine turns System Protection on if needed, asks Windows for
 * a restore point and checks it exists. PeakTweaks already runs as
 * administrator (Windows asked when it started), so there is no second prompt.
 * Used wherever the restore lock is shown, so the fix sits next to the message.
 */
export function RestorePointButton({
  label = "Make a restore point",
  variant = "primary",
  compact = false,
  className,
}: {
  label?: ReactNode;
  variant?: "primary" | "go" | "secondary";
  /** Button only: the error and progress text are shown by the page. */
  compact?: boolean;
  className?: string;
}) {
  const restoreOp = useStore((s) => s.restoreOp);
  const lastStage = useStore(currentRestoreStage);
  const technical = useTechnical();
  const { createRestorePoint } = useActions();
  const running = restoreOp.status === "running";
  return (
    <div className={className}>
      {!compact && restoreOp.status === "failed" && (
        <div className="mb-3">
          <ErrorCallout text={explain(restoreOp.error)} technical={technical} />
        </div>
      )}
      <div className="flex flex-wrap items-center gap-3">
        <Button variant={variant} busy={running} onClick={() => void createRestorePoint()}>
          {restoreOp.status === "failed" ? "Try again" : label}
        </Button>
        {!compact && running && lastStage && (
          <span role="status" className="text-sm font-semibold">
            {lastStage.message}
          </span>
        )}
      </div>
    </div>
  );
}
