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
  // Checked 2026-10-10 against Epic's Rocket League Support articles "How do I
  // fix graphic problems and frame rate drops in Rocket League on PC?"
  // (a202300000014901), "How do I adjust the in-game video settings in Rocket
  // League?" (a20386380) and its Windows power mode article (a22052207).
  rocketleague: {
    source: "Epic Games' own Rocket League Support articles on graphics problems on PC",
    intro: "Epic suggests these for Rocket League on PC. The game's settings are under Settings > Video, opened from the lobby.",
    steps: [
      "Update the graphics driver and Windows.",
      "On a PC with two graphics chips, make sure the game uses the right one.",
      "Close background programs you do not need.",
      "Basic settings: Anti-aliasing FXAA Low or Off, Render Quality and Render Detail set to Performance.",
      "Advanced settings: Texture Detail High Performance, World Detail and Particle Detail Performance.",
      "Windows power plan: High performance, or Ultimate Performance where Windows offers it (Power & sleep settings > Additional power settings).",
    ],
  },
  // The entries below were read 2026-10-10 on each publisher's own support
  // pages (named in the comment). Steps that turn off security software, edit
  // config files, use launch options or third-party tools, or touch only the
  // network are left out on purpose.
  // Riot Support, "Low FPS & Framerate Drops - Windows & Mac" (2025-03-07).
  league: {
    source: "Riot Games' own League of Legends support article on low frame rates",
    intro: "Riot suggests these for League of Legends.",
    steps: [
      "Check that the PC meets the game's minimum requirements.",
      "Update the graphics driver.",
      "Lower the graphics settings a step at a time until the game runs well enough and still looks good to you.",
      "Do a clean boot of Windows, so background programs are not running beside the game.",
    ],
  },
  // PUBG Support, "General crashing and performance guide" (updated
  // 2026-02-26). Its clean reinstall deletes BattlEye's service file, so it is
  // left out along with the BIOS and launch-option steps.
  pubg: {
    source: "KRAFTON's own PUBG support guide for crashes and slowdowns on PC",
    intro: "KRAFTON suggests these for PUBG: Battlegrounds on PC.",
    steps: [
      "Check the minimum specs, then restart the PC.",
      "Check the game's files: in Steam, right-click PUBG, Properties > Local Files > Verify integrity of game files.",
      "Update Windows and the graphics driver.",
      "Turn off fullscreen optimizations for TslGame.exe: right-click it, Properties > Compatibility.",
    ],
  },
  // Marvel Rivals, "Common Technical Issues FAQ" (2024-12-01) and "Experimental
  // Feature: Switch Shader Compilation Mode" (2025-04-10).
  marvelrivals: {
    source: "NetEase's own Marvel Rivals technical FAQ and launcher notes",
    intro: "NetEase suggests these for Marvel Rivals when the game stutters.",
    steps: [
      "Check that the PC meets the game's requirements.",
      "Reinstall the latest graphics driver with its clean installation option.",
      "Windows Settings > System > Display > Graphics: add Marvel.exe and choose the high-performance graphics option (this matters on a PC with two graphics chips).",
      "With 16 GB of memory or less, try the other shader compilation mode in the game's PC launcher.",
    ],
  },
  // HoYoverse Support, "How do I fix stuttering and frame rate drops during
  // gameplay?" (updated 2025-11-11).
  genshin: {
    source: "HoYoverse's own Genshin Impact support article on stuttering",
    intro: "HoYoverse suggests these for Genshin Impact.",
    steps: [
      "Lower Shadows, Texture Quality and Resolution in the game's graphics options.",
      "Turn on V-Sync in the graphics options.",
      "Keep the PC's air vents and fans clean so it does not run hot.",
      "Update the graphics driver and close browsers and streaming apps while you play.",
    ],
  },
  // EA Help, "How to troubleshoot EA SPORTS FC 27 PC performance issues"
  // (no date shown). The BIOS check of memory channels is left out.
  eafc: {
    source: "EA's own EA SPORTS FC troubleshooting for PC",
    intro: "EA suggests trying these in order.",
    steps: [
      "Lower the frame rate target, for example from 60 to 30.",
      "Lower the graphics quality and the resolution.",
      "Close browsers, overlays, recorders and other launchers.",
      "Check that the memory sticks are in the slots the motherboard's manual recommends, so both channels are used (Home shows whether they are).",
      "Update the graphics driver.",
      "Check the Windows power mode, a laptop's own power mode and the graphics card's control panel.",
    ],
  },
  // EA Help, "How to change your PC and controller settings in Battlefield 6"
  // (no date shown); a settings guide, not a troubleshooting article.
  battlefield6: {
    source: "EA's own Battlefield 6 PC settings guide",
    intro: "EA's settings guide for Battlefield 6, in Settings (the cog) > Graphics.",
    steps: [
      "Modify: set Undergrowth, Effects, Volumetric, Lighting, Local Light & Shadow, Sun Shadow and Post Process Quality to Low, or every setting to Low.",
      "Display: Vertical Sync off.",
      "Display: Fullscreen Resolution and Refresh Rate the same as Windows uses.",
    ],
  },
  // Arrowhead Support, "General Tech troubleshooting tips" (updated
  // 2026-09-01). Deleting its settings folder is left out.
  helldivers2: {
    source: "Arrowhead's own Helldivers 2 troubleshooting tips",
    intro: "Arrowhead suggests these for Helldivers 2 on PC.",
    steps: [
      "Check the minimum specs.",
      "Update the graphics driver, then the other drivers.",
      "Check the game's files: in Steam, right-click the game, Properties > Installed Files > Verify integrity of game files.",
      "Try Async Compute in the game's settings both on and off: which works better depends on the hardware.",
    ],
  },
  // Rockstar Support, "GTAV PC General Troubleshooting" (updated 2025-10-07).
  // Its antivirus steps are left out.
  gta5: {
    source: "Rockstar's own GTA V PC troubleshooting",
    intro: "Rockstar suggests these for GTA V on PC.",
    steps: [
      "Start the game while online once, so the latest update installs.",
      "Update the graphics driver.",
      "Check the game's files in the launcher you bought it from.",
      "Run the DirectX and Visual C++ installers in the game's Redistributables folder.",
    ],
  },
  // Embark Support, "Preloading Shaders - PC & PlayStation/Xbox" (updated
  // 2026-02-16).
  thefinals: {
    source: "Embark's own THE FINALS note on preloading shaders",
    intro: "Embark suggests these while THE FINALS prepares its shaders.",
    steps: [
      "After an update, give the shader preloading a few minutes to finish.",
      "Close background programs that keep the processor or graphics card busy.",
      "Update the graphics driver.",
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
