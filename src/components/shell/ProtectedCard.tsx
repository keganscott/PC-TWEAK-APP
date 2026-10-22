import { Check, Loader2, ShieldAlert } from "lucide-react";

import { useStore } from "../../store/hooks";
import { cx } from "../ui/primitives";
import { RestorePointButton, useCanMakeRestorePoint } from "./RestorePointButton";

/** The safety net at a glance, from the engine's restore gate only. */
export function ProtectedCard() {
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const ready = gateOpen === true;
  const canMake = useCanMakeRestorePoint();
  return (
    <section
      aria-label="Safety net"
      className={cx("rounded-xl border p-3.5", ready ? "texture-lines border-line" : "border-violet/70 bg-surface-1")}
    >
      <p className="flex items-center gap-2 text-[13px] font-bold">
        {gateOpen === null ? (
          <Loader2 aria-hidden className="size-4 animate-spin text-ink-muted" />
        ) : ready ? (
          <span aria-hidden className="grid size-[18px] place-items-center rounded-[5px] bg-lime">
            <Check className="size-3 text-black" strokeWidth={3.5} />
          </span>
        ) : (
          <ShieldAlert aria-hidden className="size-[18px] text-violet-soft" />
        )}
        {gateOpen === null ? "Checking the safety net" : ready ? "Restore point ready" : "No recent restore point"}
      </p>
      <p className="mt-1.5 text-xs leading-relaxed text-ink-faint">
        {ready
          ? "Every change is recorded first and can be undone."
          : "Changes stay locked until there is one."}
      </p>
      {gateOpen === false && canMake && (
        <RestorePointButton label="Make one now" variant="secondary" compact className="mt-2.5" />
      )}
    </section>
  );
}
