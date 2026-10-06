/** The PeakTweaks mark (round 3 logo, option A "Summit"): a violet peak with a
 * lime summit inside it. Decorative wherever it appears next to the name. */
export function LogoMark({ className, mono = false }: { className?: string; mono?: boolean }) {
  return (
    <svg viewBox="0 0 32 32" aria-hidden focusable="false" className={className}>
      <polygon points="16,3 30.5,28 23.2,28 16,15.6 8.8,28 1.5,28" className={mono ? "fill-white" : "fill-violet"} />
      <polygon points="16,20.6 20.3,28 11.7,28" className="fill-lime" />
    </svg>
  );
}

/** Mark and PEAKTWEAKS wordmark. The visible word is the accessible name. */
export function Logo() {
  return (
    <span className="flex items-center gap-2.5">
      <LogoMark className="size-7 shrink-0" />
      <span className="font-display text-[15px] font-extrabold tracking-[0.06em] text-white">
        PEAK<span className="font-medium text-ink-muted">TWEAKS</span>
      </span>
    </span>
  );
}
