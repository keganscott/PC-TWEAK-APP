import { useActions, useStore } from "../../store/hooks";
import { useNavigate } from "../shell/nav";
import { Button, Callout } from "../ui/primitives";

// ---------------------------------------------------------------------------
// Result card after a change (plan section 7)
// ---------------------------------------------------------------------------

export function LastChange() {
  const change = useStore((s) => s.lastChange);
  const tweaks = useStore((s) => s.tweaks);
  const startup = useStore((s) => s.startup);
  const msi = useStore((s) => s.msi);
  const undoing = useStore((s) => s.applyManyOp.status === "running");
  const { dismissChange, revertMany } = useActions();
  const navigate = useNavigate();
  if (!change) return null;

  const name = (id: string) =>
    (
      tweaks.find((t) => t.id === id) ??
      startup?.apps.find((a) => a.tweak.id === id)?.tweak ??
      msi?.devices.find((d) => d.tweak.id === id)?.tweak
    )?.name ?? id;
  const verb = change.kind === "apply" ? "Applied" : "Undid";
  const what = change.tweakIds.length ? change.tweakIds.map(name).join(", ") : "nothing";
  return (
    <Callout
      tone={change.failed.length ? "warn" : "ok"}
      title={`${verb}: ${what}`}
      action={
        <div className="flex flex-wrap gap-2">
          {change.kind === "apply" && change.tweakIds.length > 0 && (
            <Button busy={undoing} onClick={() => void revertMany(change.tweakIds)}>
              Undo these
            </Button>
          )}
          <Button onClick={() => navigate("backups")}>Review in Backups</Button>
          {change.kind === "apply" && <Button onClick={() => navigate("proof")}>Check the effect in Proof</Button>}
          <Button variant="ghost" onClick={dismissChange}>
            Dismiss
          </Button>
        </div>
      }
    >
      {change.failed.length > 0 && (
        <ul className="list-disc pl-5">
          {change.failed.map((f) => (
            <li key={f.tweakId}>
              {name(f.tweakId)} could not be {change.kind === "apply" ? "applied" : "undone"}: {f.error ?? "no reason given"}
            </li>
          ))}
        </ul>
      )}
    </Callout>
  );
}
