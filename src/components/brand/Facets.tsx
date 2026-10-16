import { useId } from "react";

/** Flat violet diamond facets for the top of a violet card (the violet tile of
 * Kegan's palette sheet). No glow: solid tints of the violet, fading out to the
 * left so text over the card stays on plain violet. */
const TILES: [string, string][] = [
  ["150,0 200,50 150,100 100,50", "#8a4cf0"],
  ["250,0 300,50 250,100 200,50", "#7234dd"],
  ["200,50 250,100 200,150 150,100", "#9257f3"],
  ["300,50 300,150 250,100", "#8a4cf0"],
  ["100,50 150,100 100,150 50,100", "#7234dd"],
  ["150,100 200,150 150,200 100,150", "#8a4cf0"],
  ["250,100 300,150 250,200 200,150", "#6c2fd6"],
  ["200,0 250,0 200,50 150,0", "#6c2fd6"],
  ["50,100 100,150 50,200 0,150", "#8a4cf0"],
  ["100,150 150,200 50,200", "#9257f3"],
  ["200,150 250,200 150,200", "#7234dd"],
];

export function Facets({ className }: { className?: string }) {
  // useId may contain characters a url(#…) reference does not accept.
  const id = `pt${useId().replace(/[^A-Za-z0-9_-]/g, "")}`;
  return (
    <svg
      viewBox="0 0 300 200"
      preserveAspectRatio="xMaxYMid slice"
      aria-hidden
      focusable="false"
      className={className}
    >
      <defs>
        <linearGradient id={`${id}-fade`} x1="0" x2="1" y1="0" y2="0">
          <stop offset="0" stopColor="#fff" stopOpacity="0" />
          <stop offset="0.55" stopColor="#fff" stopOpacity="1" />
        </linearGradient>
        <mask id={`${id}-mask`}>
          <rect width="300" height="200" fill={`url(#${id}-fade)`} />
        </mask>
      </defs>
      <g mask={`url(#${id}-mask)`}>
        {TILES.map(([points, fill]) => (
          <polygon key={points} points={points} fill={fill} />
        ))}
        <path
          d="M0 50L50 0M0 150L150 0M50 200L250 0M150 200L300 50M250 200L300 150M100 0L300 200M0 0L200 200M0 100L100 200M200 0L300 100"
          fill="none"
          stroke="rgb(0 0 0 / 0.14)"
        />
      </g>
    </svg>
  );
}
