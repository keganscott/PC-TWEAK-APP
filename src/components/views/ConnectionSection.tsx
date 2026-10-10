import { useId } from "react";

import type { ConnectionReading } from "../../generated/ConnectionReading";
import type { PingResult } from "../../generated/PingResult";
import type { PingTarget } from "../../generated/PingTarget";
import type { Probe } from "../../generated/Probe";
import type { WifiSignal } from "../../generated/WifiSignal";
import { explain } from "../../lib/errors";
import { formatDateTime, formatNumber } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Callout, Card, ErrorCallout, SampleBadge, type Tone } from "../ui/primitives";

const TARGET: Record<PingTarget, string> = {
  router: "Your router",
  cloudflare: "Cloudflare DNS",
  google: "Google DNS",
};

/** What the pattern of answers points at (`netcheck::read`). */
export const READING: Record<ConnectionReading, { tone: Tone; title: string; text: string }> = {
  all_answered: {
    tone: "ok",
    title: "Every echo was answered.",
    text: "Nothing was lost between this PC and your router, or past it.",
  },
  loss_to_router: {
    tone: "warn",
    title: "Echoes were lost on the way to your router.",
    text: "That points at the home network itself. On Wi-Fi, a weak signal is a common cause; a cable avoids it.",
  },
  loss_past_router: {
    tone: "warn",
    title: "Your router answered every echo, but some sent past it were lost.",
    text: "That points past your router: the line to your internet provider, or further on.",
  },
  no_internet_answers: {
    tone: "bad",
    title: "Neither public server answered.",
    text: "This PC may have no internet connection right now, or something on the way blocks these echoes.",
  },
  router_silent: {
    tone: "neutral",
    title: "Your router did not answer any echo, but the public servers did.",
    text: "Many routers are set not to answer echoes, so that alone is not a problem. The servers' figures are below.",
  },
  router_skipped: {
    tone: "neutral",
    title: "Your router skipped some echoes, but every echo to the public servers came back.",
    text: "Those echoes went through your router too, so nothing was lost on the way. Routers answer echoes to themselves last when they are busy.",
  },
  unclear: {
    tone: "neutral",
    title: "Not every target could be checked.",
    text: "So these figures do not point at one place. The reason is shown next to the target.",
  },
};

/** Whole milliseconds from Windows; 0 means it came back within 1 ms. */
export function ms(n: number | null, digits = 0): string {
  if (n === null) return "–";
  if (n === 0) return "under 1 ms";
  return `${formatNumber(n, digits)} ms`;
}

function Row({ r }: { r: PingResult }) {
  const answered = r.sent > 0 ? `${r.received} of ${r.sent}` : "not asked";
  return (
    <tr className="border-t border-line align-top">
      <th scope="row" className="py-2 pr-3 text-left font-semibold">
        {TARGET[r.target]}
        {r.address && <span className="block font-mono text-xs font-normal text-ink-muted">{r.address}</span>}
        {r.via && <span className="block text-xs font-normal text-ink-muted">Through {r.via}</span>}
        {r.problem && <span className="block text-xs font-normal text-ink-muted">{r.problem}</span>}
      </th>
      <td className="py-2 pr-3">{answered}</td>
      <td className="py-2 pr-3">
        {r.received > 0 ? (
          <>
            {ms(r.avgMs, 1)}
            <span className="block text-xs text-ink-muted">
              {ms(r.minMs)} to {ms(r.maxMs)}
            </span>
          </>
        ) : (
          "–"
        )}
      </td>
      <td className="py-2">{ms(r.jitterMs, 1)}</td>
    </tr>
  );
}

/** Catalogue E4 (ExitLag's Network Analyzer): echoes to the router and two
 * public DNS servers, started by the user. Sends nothing else and changes
 * nothing; the engine refuses while a Proof recording runs. */
export function ConnectionSection() {
  const op = useStore((s) => s.netcheckOp);
  const capturing = useStore((s) => s.proof.capturingSession !== null);
  const sample = useStore((s) => s.sample);
  const technical = useTechnical();
  const { checkConnection } = useActions();
  const headingId = useId();
  const check = op.status === "done" ? op.value : null;
  const reading = check ? READING[check.reading] : null;

  return (
    <section aria-labelledby="cat-connection">
      <div className="mb-3">
        <h2 id="cat-connection" className="text-base font-extrabold tracking-tight">
          Connection
        </h2>
      </div>
      <Card className="p-4" aria-labelledby={headingId}>
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <h3 id={headingId} className="font-bold">
                Check the connection
              </h3>
              {sample && <SampleBadge />}
            </div>
            <p className="mt-1 text-sm text-ink-muted">
              Sends 20 echo requests, what the ping command sends, to your router and to the public DNS servers of
              Cloudflare and Google, and shows how long the answers took, how much that varied (jitter) and how many
              never came back. Nothing else is sent and nothing on this PC is changed.
            </p>
            {capturing && <p className="mt-2 text-sm text-ink-muted">Available again when the Proof recording finishes.</p>}
            {op.status === "running" && (
              <p className="mt-2 text-sm text-ink-muted" role="status">
                Checking. This takes a few seconds, longer when echoes go unanswered.
              </p>
            )}
          </div>
          <Button busy={op.status === "running"} disabled={capturing} onClick={() => void checkConnection()}>
            {check ? "Check again" : "Check now"}
          </Button>
        </div>
        {check && reading && (
          <div className="mt-4 flex flex-col gap-3">
            <Callout tone={reading.tone} title={reading.title}>
              {reading.text}
            </Callout>
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <caption className="sr-only">Connection check, {formatDateTime(check.unixMs)}</caption>
                <thead>
                  <tr className="text-left text-xs text-ink-muted">
                    <th scope="col" className="pb-1 pr-3 font-semibold">
                      Asked
                    </th>
                    <th scope="col" className="pb-1 pr-3 font-semibold">
                      Answered
                    </th>
                    <th scope="col" className="pb-1 pr-3 font-semibold">
                      Round trip
                    </th>
                    <th scope="col" className="pb-1 font-semibold">
                      Jitter
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {check.results.map((r) => (
                    <Row key={r.target} r={r} />
                  ))}
                </tbody>
              </table>
            </div>
            <WifiLine wifi={check.wifi} technical={technical} />
            <p className="text-xs text-ink-muted">Checked {formatDateTime(check.unixMs)}.</p>
          </div>
        )}
        {op.status === "failed" && (
          <div className="mt-3">
            <ErrorCallout text={explain(op.error)} technical={technical} />
          </div>
        )}
      </Card>
    </section>
  );
}

/** The Wi-Fi signal's strength, when this PC is on Wi-Fi. Nothing when it is
 * not; "could not tell" rather than a guess, with the reason in the technical
 * view. */
export function WifiLine({ wifi, technical = false }: { wifi: Probe<WifiSignal>; technical?: boolean }) {
  if (wifi.state === "no") return null;
  if (wifi.state === "unknown") {
    return (
      <p className="text-sm text-ink-muted">
        Wi-Fi signal: could not tell.{technical && ` ${wifi.reason}`}
      </p>
    );
  }
  const { quality, rssiDbm, adapter, carriesCheck } = wifi.value;
  return (
    <p className="text-sm">
      <span className="font-semibold">Wi-Fi signal: {quality}%</span>
      <span className="text-ink-muted"> ({rssiDbm} dBm)</span>
      <span className="text-ink-muted">
        {" "}
        on Windows' scale, where 100 is -50 dBm or stronger. Adapter: {adapter}.
        {carriesCheck === false &&
          " The echoes above left through another connection, so this signal was not their link, unless that connection itself runs over Wi-Fi, as a VPN can."}
      </span>
    </p>
  );
}
