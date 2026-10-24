import type { CSSProperties } from "react";

import { cx } from "../ui/primitives";

/** Short marks for the games whose initials would not read well. */
const MARK: Readonly<Record<string, string>> = {
  fortnite: "FN",
  valorant: "VAL",
  cs2: "CS2",
  apex: "APEX",
  cod: "COD",
  arcraiders: "ARC",
  minecraft: "MC",
  roblox: "RBX",
  league: "LOL",
  dota2: "DOTA",
  pubg: "PUBG",
  overwatch: "OW2",
  r6siege: "R6",
  rocketleague: "RL",
  gta5: "GTA",
  tf2: "TF2",
  wow: "WOW",
  eafc: "FC",
  poe2: "POE2",
  helldivers2: "HD2",
  thefinals: "THE FINALS",
  battlefield6: "BF6",
};

/** A hue for each game, so tiles tell apart at a glance. Colours only: no
 * logos or game art, which belong to the publishers. */
const HUE: Readonly<Record<string, number>> = {
  fortnite: 262,
  valorant: 352,
  cs2: 36,
  apex: 8,
  cod: 92,
  arcraiders: 188,
  minecraft: 118,
  roblox: 214,
};

/** The mark shown on a game's tile: a set one, else the initials. */
export function gameMark(id: string, name: string): string {
  if (MARK[id]) return MARK[id];
  const words = name
    .replace(/[^\p{L}\p{N}\s]/gu, " ")
    .split(/\s+/)
    .filter(Boolean);
  if (words.length === 1) return words[0]!.slice(0, 4).toUpperCase();
  return words
    .slice(0, 3)
    .map((w) => w[0])
    .join("")
    .toUpperCase();
}

/** A stable hue for a game without a set one, in the violet-to-cyan range. */
export function gameHue(id: string): number {
  if (HUE[id] !== undefined) return HUE[id];
  let h = 0;
  for (const c of id) h = (h * 31 + c.charCodeAt(0)) % 997;
  return 180 + (h % 110);
}

/** The background of a game's tile or banner. Decorative only. */
export function GameArt({ id, name, className, big = false }: { id: string; name: string; className?: string; big?: boolean }) {
  const hue = gameHue(id);
  const style: CSSProperties = {
    backgroundImage: [
      // copy-lint-allow: CSS colour stops, not text
      `radial-gradient(circle at 18% 22%, hsl(${hue} 95% 62% / 0.55), transparent 55%)`,
      // copy-lint-allow: CSS colour stops, not text
      `radial-gradient(circle at 92% 110%, hsl(${(hue + 40) % 360} 90% 55% / 0.35), transparent 50%)`,
      // copy-lint-allow: CSS colour stops, not text
      `linear-gradient(135deg, hsl(${hue} 65% 22%), #000 88%)`,
    ].join(", "),
  };
  return (
    <div aria-hidden className={cx("absolute inset-0 overflow-hidden", className)} style={style}>
      <div className="absolute inset-0 texture-lines" />
      {/* Drawn by CSS, so the mark is not part of the tile's text or name. */}
      <span
        data-mark={gameMark(id, name)}
        className={cx(
          "absolute -right-2 -bottom-3 font-display leading-none font-black tracking-tighter text-transparent italic select-none",
          "[-webkit-text-stroke:1.5px_rgb(255_255_255/0.28)] before:content-[attr(data-mark)]",
          big ? "text-[7rem]" : "text-[4.5rem]",
        )}
      />
    </div>
  );
}
