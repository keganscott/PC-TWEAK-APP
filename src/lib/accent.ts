import type { Accent } from "../generated/Accent";

/**
 * The accent colours (Settings > Colour; Game Plan idea 13). The swatch is
 * the colour the app draws with; the shades live in `index.css`, where the
 * accent points the theme's violet family at the picked colour. Lime stays
 * "ready / done" and red stays errors, whatever is picked.
 */
export const ACCENTS: readonly { id: Accent; name: string; swatch: string }[] = [
  { id: "violet", name: "Violet", swatch: "#7e3bed" },
  { id: "blue", name: "Blue", swatch: "#2563eb" },
  { id: "teal", name: "Teal", swatch: "#0d9488" },
  { id: "green", name: "Green", swatch: "#16a34a" },
  { id: "amber", name: "Amber", swatch: "#d97706" },
  { id: "pink", name: "Pink", swatch: "#db2777" },
];

/** Put the accent on the page (or take it off for violet, the theme's own). */
export function applyAccent(accent: Accent): void {
  const root = document.documentElement;
  if (accent === "violet") root.removeAttribute("data-accent");
  else root.dataset.accent = accent;
}
