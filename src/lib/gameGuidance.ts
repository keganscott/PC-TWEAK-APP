// Settings inside each game, for the Starter game cards (plan 6.4: "in-game
// guidance and OS-level items only"). PeakTweaks never changes these; the
// player does, in the game's own menu. Sources and limits are in NOTES.md N57.

export interface GameGuidance {
  /** Who gives this advice. `null` when there is none yet. */
  source: string | null;
  intro: string;
  steps: string[];
}

export const GAME_GUIDANCE: Record<string, GameGuidance> = {
  // Plan section 8, "Sourced base (Epic's own low-FPS guidance)". VERIFY
  // against Epic's player-support article (N57).
  fortnite: {
    source: "Epic Games' own advice for PCs that struggle to run Fortnite",
    intro: "Epic suggests these. Change them in Fortnite's Settings menu and in the graphics driver's control panel.",
    steps: [
      "Rendering mode: Performance.",
      "High-resolution textures: off.",
      "V-Sync: off.",
      "Install Fortnite on an SSD if the PC has one.",
      "Close programs you do not need while playing.",
      "NVIDIA graphics only, in NVIDIA Control Panel: Low Latency Mode Ultra, and Power management mode Prefer maximum performance.",
    ],
  },
  // Plan section 8: Roblox gets a guide to its own graphics-quality setting
  // only. Menu names from memory; VERIFY (N57).
  roblox: {
    source: "Roblox's own graphics setting",
    intro: "Roblox picks its graphics quality itself unless you set it.",
    steps: [
      "In a Roblox game, open the menu (Esc) and choose Settings.",
      "Set Graphics Mode to Manual.",
      "Move Graphics Quality down a few steps, play a while, and adjust to taste.",
    ],
  },
  // Plan section 8: research against Mojang's documentation before any advice.
  minecraft: {
    source: null,
    intro:
      "PeakTweaks gives no Minecraft settings advice yet: it will only do so once the advice has been checked against Mojang's own documentation.",
    steps: [],
  },
};
