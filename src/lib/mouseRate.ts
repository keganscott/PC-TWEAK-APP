// The mouse's report rate, from the reports that reach PeakTweaks' window
// while the mouse moves. A mouse sends a report each time it is polled and has
// moved, so quick circles give one report per poll; slow moves or pauses give
// fewer. The window counts them in short slices and takes a busy one.

/** Polling rates mice offer, in reports a second. */
export const MOUSE_RATES = [125, 250, 500, 1000, 2000, 4000, 8000] as const;

/** How long a check counts reports for. */
export const CHECK_MS = 4000;

/** The slice reports are counted in. */
export const SLICE_MS = 250;

/** Fewer reports than this in a whole check means the mouse hardly moved. */
export const MIN_REPORTS = 100;

export interface MouseRate {
  /** Reports a second in a busy slice (the 80th percentile of the slices that had any). */
  perSecond: number;
  /** All reports counted. */
  reports: number;
  /** The offered rate within 15 % of `perSecond`, if one is. */
  setting: (typeof MOUSE_RATES)[number] | null;
}

/**
 * `times` are when each report arrived (ms, any origin), in order. Null when
 * too few arrived to say anything.
 */
export function mouseRate(times: readonly number[]): MouseRate | null {
  if (times.length < MIN_REPORTS) return null;
  const start = times[0]!;
  const counts = new Map<number, number>();
  for (const t of times) {
    const slice = Math.floor((t - start) / SLICE_MS);
    counts.set(slice, (counts.get(slice) ?? 0) + 1);
  }
  // The last slice is cut short by the end of the check (or of the moving),
  // so it is left out when there are others.
  const last = Math.max(...counts.keys());
  const full = [...counts.entries()].filter(([slice]) => slice !== last || counts.size === 1).map(([, n]) => n);
  full.sort((a, b) => a - b);
  const busy = full[Math.min(full.length - 1, Math.floor(full.length * 0.8))]!;
  const perSecond = Math.round((busy * 1000) / SLICE_MS);
  const setting = MOUSE_RATES.find((r) => Math.abs(perSecond - r) <= r * 0.15) ?? null;
  return { perSecond, reports: times.length, setting };
}
