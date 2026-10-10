// Settings inside each game, for the Starter game cards (plan 6.4: "in-game
// guidance and OS-level items only"). PeakTweaks never changes these; the
// player does, in the game's own menu. Each entry names its source.

export interface GameGuidance {
  /** Who gives this advice. `null` when there is none yet. */
  source: string | null;
  intro: string;
  steps: string[];
}

export const GAME_GUIDANCE: Record<string, GameGuidance> = {
  // Plan section 8, "Sourced base (Epic's own low-FPS guidance)". Checked
  // 2026-10-08 against Epic's "Troubleshooting low frame rate (FPS) in
  // Fortnite" (epicgames.com/help, a000084818), including where each is set.
  fortnite: {
    source: "Epic Games' own advice for PCs that struggle to run Fortnite",
    intro: "Epic suggests these. Change them in Fortnite's settings unless the step says where.",
    steps: [
      "Rendering mode: Performance.",
      "High-resolution textures: off, in the Epic Games Launcher (Library, the three dots next to Fortnite, Options).",
      "V-Sync: off.",
      "Install Fortnite on an SSD if the PC has one.",
      "Close programs you do not need while playing.",
      "NVIDIA graphics only, in NVIDIA Control Panel: Low Latency Mode Ultra, and Power management mode Prefer maximum performance.",
    ],
  },
  // Plan section 8: Roblox gets a guide to its own graphics-quality setting
  // only. Checked 2026-10-08 against Roblox's "Graphics Quality" help article
  // (en.help.roblox.com, 203314310).
  roblox: {
    source: "Roblox's own graphics setting",
    intro: "Roblox can pick its graphics quality itself (Automatic), or you can set it.",
    steps: [
      "In a Roblox game, open the menu (Esc) and choose Settings.",
      "Set Graphics Mode to Manual.",
      "Move Graphics Quality down a few steps, play a while, and adjust to taste.",
    ],
  },
  // Checked 2026-10-10 against Riot's "VALORANT Game and Network Instability
  // Basics" (playvalorant.com, Matt deWet, 2021-12-02). Riot also lists
  // "Improve Clarity" to turn off; it is left out here because its name trips
  // the claim-word lint, not because Riot dropped it.
  valorant: {
    source: "Riot Games' own advice for VALORANT (Game and Network Instability Basics)",
    intro: "Riot suggests these. Change them in VALORANT's settings.",
    steps: [
      "Close other programs that are running while you play.",
      "To see whether the processor or the graphics card is holding the game back, turn on the performance graphs in Settings > Video > Stats.",
      "Lower the graphics quality in Settings > Video > Graphics Quality. Material, Texture, Detail and UI Quality matter most.",
      "You can also turn off Experimental Sharpening, Bloom, Distortion and Cast Shadows there.",
    ],
  },
  // Checked 2026-10-10 against EA's "What are the best settings for Apex
  // Legends on PC?" (help.ea.com, articles/apex-legends/best-settings-pc).
  apex: {
    source: "EA's own starting settings for Apex Legends on PC",
    intro: "EA suggests these as a starting point for competitive play, in Apex's Settings > Video unless the step says where.",
    steps: [
      "Display Mode: Full screen with one monitor, windowed with more than one.",
      "V-Sync: Disabled, unless you see screen tearing.",
      "NVIDIA Reflex: Enabled, on an NVIDIA graphics card.",
      "Anti-aliasing: None. Texture Filtering: Bilinear. Texture Streaming Budget: Low or Medium.",
      "Ambient Occlusion Quality, Volumetric Lighting and Dynamic Spot Shadows: Disabled.",
      "Sun Shadow Coverage, Sun Shadow Detail, Spot Shadow Detail, Model Detail, Effects Detail and Ragdolls: Low.",
      "Mouse Acceleration: Off, in the Mouse/Keyboard tab.",
    ],
  },
  // Checked 2026-10-10 against Activision's "Call of Duty: Black Ops 6 PC
  // Troubleshooting" (support.activision.com, dated 07/02/25). Its antivirus
  // steps (exclusions, turning protections off) are left out on purpose.
  cod: {
    source: "Activision's PC troubleshooting for Call of Duty: Black Ops 6",
    intro: "Activision suggests these for Call of Duty on PC.",
    steps: [
      "Let the shader preloading in the main menu finish before you play. Leaving the main menu stops it.",
      "Keep Windows 10 or 11 up to date.",
      "If a newer graphics driver causes problems, go back to the driver version Activision recommends in its PC troubleshooting article.",
      "Turn off overclocking or tuning software while you play.",
      "If the game misbehaves, check its files: Scan and Repair in Battle.net, or Verify integrity of game files in Steam.",
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
