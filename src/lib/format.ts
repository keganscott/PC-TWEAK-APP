import type { Probe } from "../generated/Probe";
import type { RigClass } from "../generated/RigClass";

export const RIG_LABEL: Record<RigClass, string> = { low: "Low", mid: "Mid", high: "High" };

export function probeValue<T>(p: Probe<T> | null | undefined): T | null {
  return p && p.state === "yes" ? p.value : null;
}

export function formatDateTime(unixMs: number): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(unixMs));
}

export function formatGiB(bytes: number): string {
  return `${Math.round(bytes / 1024 ** 3)} GB`;
}

export function formatNumber(n: number, digits = 1): string {
  return n.toLocaleString(undefined, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** "2.3 GB" from a gigabyte up, then whole megabytes or kilobytes. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 3) return `${formatNumber(bytes / 1024 ** 3)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.ceil(bytes / 1024)} KB`;
}
