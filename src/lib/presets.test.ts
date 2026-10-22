import { describe, expect, it } from "vitest";
import type { TweakView } from "../generated/TweakView";
import { gameToolIds, planGameBundle, PRESETS, planPreset } from "./presets";

function tool(id: string, over: Partial<TweakView> = {}): TweakView {
  return {
    context: "service",
    state: { status: "default" },
    blocked: null,
    id,
    name: id,
    summary: "",
    target: "",
    category: "scheduling",
    tier: "pro",
    safety: "moderate",
    impact: "moderate",
    tradeoff: "a cost",
    requiresReboot: true,
    ...over,
  } as TweakView;
}

const competitive = PRESETS.find((p) => p.id === "competitive")!;
const lowEnd = PRESETS.find((p) => p.id === "lowend")!;

describe("the Low-end PC preset", () => {
  it("holds the visual, background and graphics driver changes for a low-end PC", () => {
    for (const id of [
      "display.animations",
      "display.transparency",
      "system.backgroundapps",
      "power.plan",
      "nvidia.framecap",
      "amd.framecap",
      "amd.rsr",
      "nvidia.texturefiltering",
    ]) {
      expect(lowEnd.holds(tool(id)), id).toBe(true);
    }
  });

  it("leaves out the competitive network and latency changes", () => {
    for (const id of ["network.nagle", "display.mpo", "nvidia.vsyncoff", "privacy.telemetry"]) {
      expect(lowEnd.holds(tool(id)), id).toBe(false);
    }
  });
});

describe("the Competitive preset", () => {
  it("holds the pro latency tools and the power plan", () => {
    for (const id of ["display.mpo", "scheduling.foreground", "power.plan"]) {
      expect(competitive.holds(tool(id)), id).toBe(true);
    }
  });

  it("sorts its changes into to make, in place and not here", () => {
    const plan = planPreset(competitive, [
      tool("display.mpo"),
      tool("scheduling.foreground", { state: { status: "applied" } }),
      tool("power.plan", {
        blocked: { code: "restorePointRequired", message: "x", trigger: null },
      } as unknown as Partial<TweakView>),
      tool("privacy.telemetry", { category: "privacy", safety: "safe" }),
    ]);
    expect(plan.toApply.map((t) => t.id)).toEqual(["display.mpo"]);
    expect(plan.inPlace.map((t) => t.id)).toEqual(["scheduling.foreground"]);
    expect(plan.notHere.map((t) => t.id)).toEqual(["power.plan"]);
  });
});

describe("a game's pro setup", () => {
  it("names the game's own tools, and no other game's", () => {
    expect(gameToolIds("fortnite")).toEqual([
      "priority.ifeo.fortnite",
      "gaming.fullscreen.fortnite",
      "gpu.choice.fortnite",
      "game.fortnite.vsync",
    ]);
  });

  it("sorts only that game's tools", () => {
    const plan = planGameBundle("fortnite", [
      tool("priority.ifeo.fortnite"),
      tool("gaming.fullscreen.fortnite", { state: { status: "foreign" } }),
      tool("gpu.choice.fortnite", {
        blocked: { code: "antiCheatEligibility", message: "x", trigger: null },
      } as unknown as Partial<TweakView>),
      tool("priority.ifeo.apex"),
      tool("display.mpo"),
    ]);
    expect(plan.toApply.map((t) => t.id)).toEqual(["priority.ifeo.fortnite"]);
    expect(plan.inPlace.map((t) => t.id)).toEqual(["gaming.fullscreen.fortnite"]);
    expect(plan.notHere.map((t) => t.id)).toEqual(["gpu.choice.fortnite"]);
  });
});
