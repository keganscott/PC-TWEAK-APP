import { describe, expect, it } from "vitest";

import { BLOCKED_HINT, whyBlocked } from "./blocked";
import { explain } from "./errors";

describe("blocked reasons", () => {
  it("every code has its own guidance, and errors carry it as the hint", () => {
    const hints = Object.values(BLOCKED_HINT);
    expect(new Set(hints).size).toBe(hints.length);
    for (const [code, hint] of Object.entries(BLOCKED_HINT)) {
      expect(hint.length).toBeGreaterThan(20);
      const text = explain({
        kind: "blocked",
        reason: { code: code as keyof typeof BLOCKED_HINT, trigger: null, message: "Engine words." },
      });
      expect(text.title).toBe("Engine words.");
      expect(text.hint).toBe(hint);
    }
  });

  it("what this PC is comes before the plan", () => {
    const plan = { code: "tier_required", trigger: null, message: "Pro." } as const;
    const isProtected = { code: "protected_program", trigger: null, message: "This starts Windows Security." } as const;
    expect(whyBlocked({ blocked: plan, state: { status: "blocked", reason: isProtected } })).toBe(isProtected);
    expect(whyBlocked({ blocked: plan, state: { status: "default" } as never })).toBe(plan);
    expect(whyBlocked({ blocked: null, state: { status: "default" } as never })).toBeNull();
  });
});
