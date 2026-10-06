# Style mockups (2026-10-06)

Three Home-screen styles for Kegan to choose from, all with the same sample content (labelled "Mockup · sample data"). Static HTML, rendered at 1440×900 to the PNGs here; fonts are Google Fonts (Plus Jakarta Sans, Exo 2, Inter, JetBrains Mono) and were loaded locally when rendering, so the HTML falls back to system fonts if opened as is.

- **A: Obsidian.** EXM/Hone direction: near-black glass cards, violet-to-magenta accent, top summary cards, hardware rings, an Auto panel.
- **B: Core.** Original Xbox (2001) dashboard inspiration kept clean: black-green glow, a glowing "core" orb as the status centrepiece, chamfered panels, pill "blade" menu, A/B button legend.
- **C: Paper.** Claude's own pick: light, editorial, hairline structure, large type, monospaced numbers, one electric-blue accent.

Notes for whichever is chosen:
- Copy follows the app's rules: no efficacy claims, every change undoable, sample data labelled.
- The rings in A and the meters in B show real readings only (memory speed against rated speed, display Hz against the maximum); live CPU/GPU load would need a new read-only monitor, which does not exist yet.
- "Auto" is planned (plan 6.3) and needs the N2 reference documents for its safe set.

## Round 2 (after Kegan's feedback: "like A, but not its colours; Hone/EXM-like with a better colour combo and a better UI")

`v2.html` is one refined layout (custom title bar, Auto as a large circular hero button, restore point and Undo cards, hardware tiles with real readings, findings, game cards) in three palettes, chosen with `?p=aurora|ember|volt`:

- **Aurora** (`v2-aurora.png`): cyan to mint on deep navy.
- **Ember** (`v2-ember.png`): orange to amber on graphite.
- **Volt** (`v2-volt.png`): electric lime to green on near-black; a modern nod to the original Xbox green.

Each palette is a set of CSS variables, so the chosen one maps directly onto the app's existing theme tokens (`src/index.css`).

## Round 3 (after Kegan's feedback: "the big circle looks dumb; cleaner colours, not glowy; a better logo; use my palette")

Kegan's palette: Black `#000000`, Violet `#7E3BED`, White `#FFFFFF`, Lime `#C6FF34`.

- `v3.png` / `v3.html`: the dashboard. Pure black base, near-black cards with hairline borders, no glows or gradients on text. How the colours are used:
  - **Violet** carries actions and selection: the Auto card, the active nav bar, count badges, and "worth a look" flags on hardware tiles.
  - **Lime** marks "ready / done / you can fix" with black text on it, plus the one key number in the headline. It is used sparingly so it stays sharp.
  - **White** is the type; secondary text is white at reduced opacity.
  - Two textures from the palette sheet appear once each, flat: fine diagonal lines (Protected card, restore point card) and violet diamond facets (Auto card).
  - The orb is gone. Auto is a plain card with its four steps and a lime "Run Auto" button.
- `v3-logo.png` / `v3-logo.html`: logo sheet. Three marks, with A as the recommendation:
  - **A, Summit:** a violet peak with a lime summit inside.
  - **B, Rank:** two stacked chevrons.
  - **C, Play P:** a P made from a play button.
  - Mark A is shown as an app icon, on each palette colour, at 16–48 px, and with the PEAKTWEAKS wordmark (Exo 2).
