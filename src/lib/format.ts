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

/** "41 seconds", "12 minutes", "1 hour 5 minutes". */
export function formatDuration(seconds: number): string {
  const count = (n: number, unit: string) => `${n.toLocaleString()} ${unit}${n === 1 ? "" : "s"}`;
  if (seconds < 60) return count(Math.max(0, Math.round(seconds)), "second");
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return count(minutes, "minute");
  const hours = Math.floor(minutes / 60);
  return minutes % 60 === 0 ? count(hours, "hour") : `${count(hours, "hour")} ${count(minutes % 60, "minute")}`;
}
