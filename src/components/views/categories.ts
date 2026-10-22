import {
  Cog,
  Cpu,
  EyeOff,
  Gamepad2,
  Gpu,
  Monitor,
  Mouse,
  Network,
  Palette,
  SlidersHorizontal,
  Timer,
  Zap,
  type LucideIcon,
} from "lucide-react";

/** Tools' categories (each tweak's `category`), with an icon and a display name. */
const CATEGORY: Readonly<Record<string, { icon: LucideIcon; name: string }>> = {
  input: { icon: Mouse, name: "Input" },
  gaming: { icon: Gamepad2, name: "Gaming" },
  display: { icon: Monitor, name: "Display" },
  appearance: { icon: Palette, name: "Appearance" },
  system: { icon: Cpu, name: "System" },
  scheduling: { icon: Timer, name: "Scheduling" },
  network: { icon: Network, name: "Network" },
  power: { icon: Zap, name: "Power" },
  services: { icon: Cog, name: "Services" },
  privacy: { icon: EyeOff, name: "Privacy" },
  graphics: { icon: Gpu, name: "Graphics" },
};

export function categoryIcon(category: string): LucideIcon {
  return CATEGORY[category.toLowerCase()]?.icon ?? SlidersHorizontal;
}

export function categoryName(category: string): string {
  return CATEGORY[category.toLowerCase()]?.name ?? category.charAt(0).toUpperCase() + category.slice(1);
}
