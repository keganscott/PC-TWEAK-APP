import { describe, expect, it } from "vitest";

import type { TweakView } from "../generated/TweakView";
import * as fx from "../generated/fixtures";
import { planSetup, readSetupCode, setupCode } from "./setupCode";

const views = fx.tweakViews as TweakView[];
const ids = (list: { id: string }[]) => list.map((t) => t.id);

describe("copying a setup to another PC", () => {
  it("copies the changes PeakTweaks applied here, and reads them back", () => {
    const code = setupCode(views);
    // Already set by something else is not part of this PC's setup.
    expect(JSON.parse(code)).toEqual({ "peaktweaks-setup": 1, changes: ["fixture.applied"] });
    expect(readSetupCode(code)).toEqual({ ids: ["fixture.applied"] });
    expect(readSetupCode(`  ${code}\n`)).toEqual({ ids: ["fixture.applied"] });
  });

  it("refuses text that is not a setup, and one from a newer version", () => {
    const notOne = { problem: "That is not a setup copied from PeakTweaks." };
    for (const text of ["", "hello", "[]", "null", '{"changes":["a"]}', '{"peaktweaks-setup":1,"changes":"a"}']) {
      expect(readSetupCode(text)).toEqual(notOne);
    }
    expect(readSetupCode('{"peaktweaks-setup":1,"changes":[1]}')).toEqual(notOne);
    expect(readSetupCode(`{"peaktweaks-setup":1,"changes":["${"x".repeat(201)}"]}`)).toEqual(notOne);
    expect(readSetupCode('{"peaktweaks-setup":1,"changes":["a"]}' + " ".repeat(20_000))).toEqual(notOne);
    expect(readSetupCode('{"peaktweaks-setup":2,"changes":[]}')).toEqual({
      problem: "That setup is from a newer PeakTweaks. Update this one first.",
    });
    expect(readSetupCode('{"peaktweaks-setup":1,"changes":["a","a"]}')).toEqual({ ids: ["a"] });
  });

  it("plans against this PC's own list: what to apply, what needs Tools, what is in place or not here", () => {
    const risky = { ...views.find((t) => t.id === "fixture.default")!, id: "risky", safety: "extreme" as const };
    const gated = {
      ...views.find((t) => t.id === "fixture.drifted")!,
      id: "gated",
      blocked: { code: "no_restore_point" as const, trigger: null, message: "SAMPLE" },
    };
    const looks = { ...views.find((t) => t.id === "fixture.default")!, id: "looks", category: "appearance" };
    const policy = { ...gated, id: "policy", blocked: { code: "set_by_policy" as const, trigger: null, message: "SAMPLE" } };
    const list: TweakView[] = [...views, risky, gated, policy, looks];
    const plan = planSetup(
      ["fixture.default", "fixture.drifted", "gated", "fixture.applied", "fixture.foreign", "fixture.unknown", "risky", "policy", "looks", "fixture.blocked", "elsewhere.only"],
      list,
    );
    expect(ids(plan.apply)).toEqual(["fixture.default", "fixture.drifted", "gated"]);
    expect(ids(plan.already)).toEqual(["fixture.applied", "fixture.foreign"]);
    expect(ids(plan.yourself)).toEqual(["fixture.unknown", "risky", "policy", "looks"]);
    expect(plan.notHere).toEqual([views.find((t) => t.id === "fixture.blocked")!.name, "elsewhere.only"]);
  });
});
