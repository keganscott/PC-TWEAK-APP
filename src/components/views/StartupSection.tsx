import { useEffect, useId } from "react";

import type { StartupApp } from "../../generated/StartupApp";
import type { StartupSource } from "../../generated/StartupSource";
import { blockedHint } from "../../lib/blocked";
import { explain } from "../../lib/errors";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Callout, Card, ErrorCallout, SampleBadge, StatusBadge } from "../ui/primitives";

/** Where Windows keeps the entry, in words that hold whether it is on or off. */
const WHERE: Record<StartupSource, string> = {
  user_run: "Set up for your account",
  machine_run: "Set up for everyone on this PC",
  machine_run32: "Set up for everyone on this PC",
  user_folder: "In your Startup folder",
  machine_folder: "In the Startup folder for everyone",
};

/** Does it start at sign-in? Only a switch that is off says no. */
export function startsAtSignIn(app: StartupApp): boolean {
  const status = app.tweak.state.status;
  return status !== "applied" && status !== "foreign";
}

/** Catalogue H12: the programs Windows starts when the user signs in, each with
 * Task Manager's own on/off switch. Turning one off is a change like any other:
 * recorded first, undone here or from Backups. The engine refuses security
 * software and anti-cheat. */
export function StartupSection() {
  const list = useStore((s) => s.startup);
  const op = useStore((s) => s.startupOp);
  const gateOpen = useStore((s) => s.audit?.env.restoreGateOpen ?? null);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { loadStartup } = useActions();

  // Read once when the section first shows; "Check again" reads again.
  useEffect(() => {
    if (op.status === "idle") void loadStartup();
  }, [op.status, loadStartup]);

  const starting = list?.apps.filter(startsAtSignIn).length ?? 0;

  return (
    <section aria-labelledby="cat-startup">
      <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 id="cat-startup" className="text-base font-extrabold tracking-tight">
              Startup apps
            </h2>
            {sample && <SampleBadge />}
          </div>
          <p className="mt-1 text-sm text-ink-muted">
            Programs Windows starts when you sign in. A program turned off here stays installed and starts when you open
            it. This is the same switch as Task Manager's, and it takes effect the next time you sign in.
          </p>
        </div>
        <Button variant="ghost" busy={op.status === "running"} onClick={() => void loadStartup()}>
          Check again
        </Button>
      </div>
      <Card className="flex flex-col gap-4 p-4">
        {!list && op.status === "running" && <p className="text-sm text-ink-muted">Looking for startup apps…</p>}
        {list && list.apps.length === 0 && list.problems.length === 0 && (
          <p className="text-sm text-ink-muted">Nothing starts when you sign in.</p>
        )}
        {list && list.apps.length > 0 && (
          <>
            <p className="text-sm text-ink-muted" role="status">
              {starting} of {list.apps.length} start when you sign in.
            </p>
            <ul className="flex flex-col gap-4">
              {list.apps.map((app) => (
                <li key={app.tweak.id}>
                  <StartupRow app={app} gateOpen={gateOpen} />
                </li>
              ))}
            </ul>
          </>
        )}
        {list && list.problems.length > 0 && (
          <Callout tone="warn" title="Some places could not be read, so this list may be missing programs.">
            <ul className="list-disc pl-5">
              {list.problems.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          </Callout>
        )}
        {op.status === "failed" && <ErrorCallout text={explain(op.error)} technical={technical} />}
      </Card>
    </section>
  );
}

function StartupRow({ app, gateOpen }: { app: StartupApp; gateOpen: boolean | null }) {
  const { tweak } = app;
  const op = useStore((s) => s.tweakOps[tweak.id]);
  const technical = useTechnical();
  const { applyTweak, revertTweak, clearTweakOp } = useActions();
  const id = useId();
  const hintId = useId();

  const status = tweak.state.status;
  const running = op?.status === "running";
  const on = startsAtSignIn(app);
  // The engine's reason it is not offered: its plan, or a protected program.
  const blocked = tweak.blocked ?? (tweak.state.status === "blocked" ? tweak.state.reason : null);
  // Off: ours to turn back on. On: ours to turn off, once there is a restore
  // point. Turned off elsewhere, or unreadable: nothing to do here.
  const canTurn =
    status === "applied" || (on && !blocked && status !== "unknown" && gateOpen !== false);

  const turn = (wantOn: boolean) => void (wantOn ? revertTweak(tweak.id) : applyTweak(tweak.id));

  return (
    <div>
      <div className="flex items-start gap-3">
        <input
          id={id}
          type="checkbox"
          role="switch"
          aria-checked={on}
          aria-describedby={hintId}
          aria-busy={running}
          checked={on}
          disabled={!canTurn || running}
          onChange={(e) => turn(e.target.checked)}
          className="mt-1 size-4 shrink-0 accent-accent"
        />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <label htmlFor={id} className="cursor-pointer font-bold">
              {app.name}
            </label>
            {status === "applied" && <StatusBadge tone="ok">Turned off</StatusBadge>}
            {status === "foreign" && <StatusBadge tone="ok">Already off</StatusBadge>}
            {status === "drifted" && <StatusBadge tone="warn">Turned back on outside PeakTweaks</StatusBadge>}
          </div>
          <div id={hintId} className="mt-1 text-sm text-ink-muted">
            <p>
              {on ? "Starts when you sign in." : "Does not start when you sign in."} {WHERE[app.source]}.
            </p>
            {blocked && (
              <p className="mt-1 text-ink">
                {blocked.message} <span className="text-ink-muted">{blockedHint(blocked)}</span>
              </p>
            )}
            {status === "foreign" && <p className="mt-1">Turned off outside PeakTweaks, for example in Task Manager.</p>}
            {status === "unknown" && <p className="mt-1">PeakTweaks could not read its switch, so it leaves it alone.</p>}
          </div>
          {technical && (
            <p className="mt-1 break-all font-mono text-xs text-ink-faint">
              {app.command ?? app.name}
              {tweak.state.status === "unknown" && ` (${tweak.state.detail})`}
            </p>
          )}
        </div>
      </div>
      {op?.status === "failed" && (
        <div className="mt-2">
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
    </div>
  );
}
