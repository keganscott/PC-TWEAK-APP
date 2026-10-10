import { CheckCircle2, ExternalLink, Gpu, HardDriveDownload } from "lucide-react";
import { useId, type ReactNode } from "react";

import type { DriverVendor } from "../../generated/DriverVendor";
import type { GpuDriver } from "../../generated/GpuDriver";
import { explain } from "../../lib/errors";
import { probeValue } from "../../lib/format";
import { otherLongWork, type LongWork } from "../../store/store";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { RestorePointButton } from "../shell/RestorePointButton";
import { Button, cx, ErrorCallout, SampleBadge, Spinner } from "../ui/primitives";

/** The card makers PeakTweaks has a driver page for, by PCI vendor id. */
const VENDOR: Readonly<Record<number, { id: DriverVendor; name: string }>> = {
  0x10de: { id: "nvidia", name: "NVIDIA" },
  0x1002: { id: "amd", name: "AMD" },
  0x8086: { id: "intel", name: "Intel" },
};

export function driverVendor(d: GpuDriver): { id: DriverVendor; name: string } | null {
  return d.vendorId === null ? null : (VENDOR[d.vendorId] ?? null);
}

/** "Driver 581.80, dated 2025-08-20" (the package date, as Home's reminder shows it). */
function driverLine(d: GpuDriver): string {
  const version = d.nvidiaVersion ?? d.driverVersion;
  const parts = [version ? `Driver ${version}` : "Driver version not read"];
  if (d.driverDate) parts.push(`dated ${d.driverDate}`);
  return parts.join(", ");
}

/** Why the long one-time action that holds the engine blocks this one. */
const BUSY: Partial<Record<LongWork, string>> = {
  proof: "Available again when the Proof recording finishes.",
  cleanup: "Available again when the junk cleanup finishes.",
  drive: "Available again when the drive optimization finishes.",
};

function Step({ n, done, title, children }: { n: number; done?: boolean; title: string; children: ReactNode }) {
  return (
    <li className="relative flex gap-3">
      <span
        aria-hidden
        className={cx(
          "flex size-7 shrink-0 items-center justify-center rounded-full text-sm font-extrabold",
          done ? "bg-lime text-black" : "bg-linear-to-br from-violet to-violet-strong text-white",
        )}
      >
        {done ? <CheckCircle2 className="size-4" strokeWidth={2.5} /> : n}
      </span>
      <div className="min-w-0 flex-1 pb-1">
        <h4 className="font-bold text-ink">{title}</h4>
        <div className="mt-1 text-sm text-ink-muted">{children}</div>
      </div>
    </li>
  );
}

/**
 * The graphics driver tool (Kegan, 2026-10-10: "a tool that allows you to
 * uninstall your graphics driver and install one that you select"), inside
 * plan section 1's "Link to vendor pages only": PeakTweaks opens the maker's
 * page in the browser and downloads nothing. For an NVIDIA file the user
 * downloaded, the engine does a clean install (`gpu_install.rs`): Windows'
 * own Open dialog, NVIDIA's signature checked, a restore point and a journal
 * line first. Nothing here says one driver version plays better.
 */
export function DriverTool() {
  const audit = useStore((s) => s.audit);
  const sample = useStore((s) => s.sample);
  const pageOp = useStore((s) => s.driverPageOp);
  const op = useStore((s) => s.driverInstallOp);
  const busy = useStore((s) => otherLongWork(s, "driver"));
  const technical = useTechnical();
  const { openDriverPage, installGpuDriver } = useActions();
  const headingId = useId();

  const probe = audit?.env.hardware?.gpuDrivers;
  const drivers = probeValue(probe) ?? [];
  const gateOpen = audit?.env.restoreGateOpen ?? null;
  const vendors = [...new Map(drivers.flatMap((d) => (driverVendor(d) ? [[driverVendor(d)!.id, driverVendor(d)!]] : []))).values()];
  const hasNvidia = vendors.some((v) => v.id === "nvidia");
  const running = op.status === "running";
  const result = op.status === "done" ? op.value : null;

  return (
    <section aria-labelledby={headingId} className="relative isolate overflow-hidden rounded-2xl border border-line bg-surface-1 p-5">
      <div aria-hidden className="pointer-events-none absolute -bottom-20 -left-16 -z-10 size-56 rounded-full bg-lime/10 blur-3xl" />
      <div className="flex flex-wrap items-center gap-2">
        <Gpu aria-hidden className="size-4 text-lime" strokeWidth={2} />
        <h3 id={headingId} className="font-display text-lg font-extrabold tracking-tight">
          Graphics driver
        </h3>
        {sample && <SampleBadge />}
      </div>

      {drivers.length > 0 ? (
        <ul className="mt-3 flex flex-col gap-2">
          {drivers.map((d) => (
            <li key={`${d.name}-${d.driverVersion}`} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 rounded-lg bg-surface-2 px-3 py-2">
              <span className="font-bold text-ink">{d.name}</span>
              <span className="text-sm text-ink-muted tabular-nums">{driverLine(d)}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-3 text-sm text-ink-muted">
          {probe && probe.state !== "yes" ? `Graphics cards not read: ${probe.reason}` : "Reading the graphics cards…"}
        </p>
      )}

      <ol className="mt-5 flex flex-col gap-4">
        <Step n={1} done={gateOpen === true} title="Make a restore point">
          {gateOpen === true ? (
            <p>Ready. System Restore puts the old driver back if the new one is not right.</p>
          ) : (
            <>
              <p>System Restore puts the old driver back if the new one is not right, so the install waits for one.</p>
              <RestorePointButton compact variant="secondary" className="mt-2" />
            </>
          )}
        </Step>
        <Step n={2} title="Download the driver you want">
          <p>
            Pick your card under Manual Driver Search and save the file. For an older version, NVIDIA's driver help points to its Beta
            and Archived Drivers list. PeakTweaks downloads nothing itself.
          </p>
          <div className="mt-2 flex flex-wrap gap-2">
            {(vendors.length > 0 ? vendors : [VENDOR[0x10de]!]).map((v) => (
              <Button
                key={v.id}
                variant={v.id === "nvidia" ? "primary" : "secondary"}
                busy={pageOp.status === "running"}
                onClick={() => void openDriverPage(v.id)}
                icon={<ExternalLink aria-hidden className="size-4" />}
              >
                Open {v.name}'s driver page
              </Button>
            ))}
          </div>
          {pageOp.status === "failed" && (
            <div className="mt-2">
              <ErrorCallout text={explain(pageOp.error)} technical={technical} />
            </div>
          )}
        </Step>
        <Step n={3} done={!!result} title="Install it as a clean install">
          {hasNvidia ? (
            <p>
              PeakTweaks checks that NVIDIA signed the file, then runs NVIDIA's installer with its clean-install option: the old
              driver and its settings are removed first. NVIDIA Control Panel settings go back to their defaults, PeakTweaks'
              NVIDIA changes included; Tools then shows them as not in effect. The screen goes black for a moment while the driver
              changes.
            </p>
          ) : (
            <p>The clean install is for NVIDIA drivers only. For another card, run its maker's installer yourself.</p>
          )}
          <div className="mt-2 flex flex-wrap items-center gap-3">
            <Button
              variant="go"
              busy={running}
              disabled={!hasNvidia || gateOpen !== true || busy !== null}
              onClick={() => void installGpuDriver()}
              icon={<HardDriveDownload aria-hidden className="size-4" />}
            >
              Choose the file and install
            </Button>
            {running && <Spinner label="Installing. Keep PeakTweaks open; NVIDIA's installer takes a few minutes." />}
          </div>
          {busy && <p className="mt-2">{BUSY[busy]}</p>}
          {result && (
            <p className="mt-2 text-ink" role="status">
              Driver {result.version ?? result.file} installed{result.restart ? ". Restart Windows to finish." : "."}
            </p>
          )}
          {op.status === "failed" && (
            <div className="mt-2">
              <ErrorCallout text={explain(op.error)} technical={technical} />
            </div>
          )}
        </Step>
      </ol>
      <p className="mt-4 text-xs text-ink-faint">
        PeakTweaks makes no claim about which driver version suits a game. Proof can compare a game before and after on this PC.
      </p>
    </section>
  );
}
