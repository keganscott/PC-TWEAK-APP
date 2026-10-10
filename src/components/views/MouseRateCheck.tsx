import { MousePointer2 } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";

import { formatNumber } from "../../lib/format";
import { CHECK_MS, mouseRate, type MouseRate } from "../../lib/mouseRate";
import { Button } from "../ui/primitives";

/** No first report within this long after Start: the check gives up. */
const WAIT_MS = 10_000;

type Check = { status: "idle" } | { status: "counting"; started: boolean } | { status: "done"; rate: MouseRate | null };

/**
 * Mouse report rate: counts the reports that reach this window while the
 * mouse moves (`lib/mouseRate.ts`). Reads nothing from Windows and changes
 * nothing; the engine is not involved.
 */
export function MouseRateCheck() {
  const headingId = useId();
  const [check, setCheck] = useState<Check>({ status: "idle" });
  const [elapsed, setElapsed] = useState(0);
  const stop = useRef<(() => void) | null>(null);

  useEffect(() => () => stop.current?.(), []);

  const start = () => {
    stop.current?.();
    const times: number[] = [];
    let first: number | null = null;
    const timers: ReturnType<typeof setTimeout>[] = [];
    const finish = () => {
      stop.current?.();
      setCheck({ status: "done", rate: mouseRate(times) });
    };
    // Each report the mouse sent; a browser without the raw event hands them
    // over bundled in one move event.
    const raw = "onpointerrawupdate" in window;
    const onReport = (e: Event) => {
      const p = e as PointerEvent;
      if (p.pointerType && p.pointerType !== "mouse") return;
      const now = performance.now();
      const n = raw ? 1 : Math.max(1, p.getCoalescedEvents?.().length ?? 1);
      for (let i = 0; i < n; i++) times.push(now);
      if (first === null) {
        first = now;
        setCheck({ status: "counting", started: true });
        timers.push(setTimeout(finish, CHECK_MS));
      }
    };
    const type = raw ? "pointerrawupdate" : "pointermove";
    window.addEventListener(type, onReport);
    timers.push(
      setTimeout(() => {
        if (first === null) finish();
      }, WAIT_MS),
    );
    const tick = setInterval(() => setElapsed(first === null ? 0 : performance.now() - first), 200);
    stop.current = () => {
      window.removeEventListener(type, onReport);
      timers.forEach(clearTimeout);
      clearInterval(tick);
      stop.current = null;
    };
    setElapsed(0);
    setCheck({ status: "counting", started: false });
  };

  const counting = check.status === "counting";
  const rate = check.status === "done" ? check.rate : null;
  const left = Math.max(0, Math.ceil((CHECK_MS - elapsed) / 1000));

  return (
    <section aria-labelledby={headingId} className="relative isolate overflow-hidden rounded-2xl border border-line bg-surface-1 p-5">
      <div aria-hidden className="pointer-events-none absolute -top-16 -left-16 -z-10 size-56 rounded-full bg-violet/15 blur-3xl" />
      <div className="flex flex-wrap items-center gap-2">
        <MousePointer2 aria-hidden className="size-4 text-lime" strokeWidth={2} />
        <h3 id={headingId} className="font-display text-lg font-extrabold tracking-tight">
          Mouse report rate
        </h3>
      </div>
      <p className="mt-2 text-sm text-ink-muted">
        How many times a second your mouse reports its movement, as set in its own software (125 to 8000 Hz). Press the button,
        then move the mouse in quick circles for {CHECK_MS / 1000} seconds; PeakTweaks counts the reports that reach this window.
      </p>
      <div className="mt-4 flex flex-wrap items-center gap-4">
        <Button variant="go" busy={counting} disabled={counting} onClick={start} icon={<MousePointer2 aria-hidden className="size-4" />}>
          {check.status === "done" ? "Count again" : "Count my mouse's reports"}
        </Button>
        {counting && (
          <p className="text-sm font-bold text-lime" role="status">
            {check.started ? `Keep moving: ${left} s left` : "Move the mouse in quick circles now"}
          </p>
        )}
      </div>
      {check.status === "done" && (
        <div className="mt-4" role="status">
          {rate ? (
            <>
              <p className="font-display text-3xl font-extrabold tabular-nums">
                {formatNumber(rate.perSecond, 0)} <span className="text-base font-bold text-ink-muted">reports a second</span>
              </p>
              <p className="mt-1 text-sm">
                {rate.setting
                  ? `That matches a ${formatNumber(rate.setting, 0)} Hz setting.`
                  : "That is not one of the usual settings; try again with quick, steady circles for the whole time."}{" "}
                <span className="text-ink-muted">{formatNumber(rate.reports, 0)} reports counted.</span>
              </p>
            </>
          ) : (
            <p className="text-sm">Too few reports to tell. Move the mouse in quick circles for the whole {CHECK_MS / 1000} seconds.</p>
          )}
        </div>
      )}
      <p className="mt-4 text-xs text-ink-faint">
        Above 1000 a second, Windows may hand this window fewer reports than the mouse sends, so a lower figure there can be this
        check's limit rather than the mouse's.
      </p>
    </section>
  );
}
