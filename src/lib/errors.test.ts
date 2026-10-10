import { describe, expect, it } from "vitest";
import { explain } from "./errors";

const command = (what: string, detail: string) => explain({ kind: "command", what, exitCode: null, detail });

describe("engine errors in plain words", () => {
  it("names restore point failures plainly and says what to check", () => {
    const t = command("Checkpoint-Computer", "Windows reported success but no new restore point appeared.");
    expect(t.title).toBe("Windows did not make a restore point.");
    expect(t.hint).toContain("System Protection");
    expect(t.detail).toContain("Checkpoint-Computer");
  });

  it("points a refused graphics driver change at the vendor's own panel", () => {
    expect(command("NVIDIA settings", "NvAPI_DRS_SetSetting failed with NvAPI code -160")).toMatchObject({
      title: "The NVIDIA driver did not accept the change.",
      hint: expect.stringContaining("NVIDIA Control Panel"),
      detail: "NVIDIA settings: NvAPI_DRS_SetSetting failed with NvAPI code -160",
    });
    expect(command("AMD graphics settings", "x").hint).toContain("AMD Software");
  });

  it("says a timed-out step was Windows being busy", () => {
    const t = command("service SysMain", "did not finish within 30 s and was stopped");
    expect(t.title).toBe("Windows did not change the service SysMain.");
    expect(t.hint).toContain("Try again in a moment");
  });

  it("only blames focus when a test run recorded nothing", () => {
    expect(command("PresentMon", "PresentMon wrote no data").hint).toContain("in focus");
    expect(command("PresentMon", "exit 5: access denied").hint).toBeNull();
  });

  it("keeps the engine's own name for steps it does not know", () => {
    expect(command("Energy-Efficient Ethernet", "x")).toMatchObject({ title: "Energy-Efficient Ethernet did not finish.", hint: null });
  });

  it("explains common Windows codes and keeps the rest generic", () => {
    const win32 = (code: number) => explain({ kind: "win32", call: "RegSetValueExW", code, detail: "d" });
    expect(win32(5).title).toBe("Windows refused access.");
    expect(win32(112).hint).toContain("disk space");
    expect(win32(87)).toMatchObject({ title: "A Windows call failed.", hint: null, detail: "RegSetValueExW (87): d" });
  });

  it("says a change made only in part is on the record and Undo puts it back (audit I5)", () => {
    const t = explain({ kind: "partly_applied", tweakId: "power.plan", detail: "registry HKLM\\X: denied", undoDetail: "registry HKLM\\X: denied" });
    expect(t.title).toBe("This change was made only in part, and PeakTweaks could not put that part back.");
    expect(t.hint).toContain("Press Undo");
    expect(t.detail).toBe("power.plan: registry HKLM\\X: denied; putting it back: registry HKLM\\X: denied");
    // A refused change never claims more than the record shows.
    expect(explain({ kind: "registry", path: "HKLM\\X", value: null, detail: "d" }).hint).toContain("Undo");
  });
});
