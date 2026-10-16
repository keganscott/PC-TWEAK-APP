import { describe, expect, it } from "vitest";
import type { TweakView } from "../generated/TweakView";
import { PRESETS, planPreset } from "./presets";

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
