import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import * as fx from "./generated/fixtures";
import { App } from "./App";
import type { Backend } from "./services/backend";
import { createMockBackend, type MockOptions } from "./services/mockIpc";
import { StoreProvider } from "./store/hooks";
import { createAppStore } from "./store/store";

function renderApp(backend: Backend = createMockBackend()) {
  const store = createAppStore(backend);
  render(
    <StoreProvider store={store}>
      <App />
    </StoreProvider>,
  );
  return store;
}

async function goTo(name: string) {
  await userEvent.click(within(screen.getByRole("navigation", { name: "Main" })).getByRole("button", { name }));
  await screen.findByRole("heading", { name, level: 1 });
}

describe("App", () => {
  it("shows a failure screen with the reason, and Try again recovers", async () => {
    const options: MockOptions = { failures: { context: { kind: "not_elevated" } } };
    renderApp(createMockBackend(options));
    expect(await screen.findByRole("heading", { name: "PeakTweaks could not start" })).toBeTruthy();
    expect(screen.getByText("PeakTweaks is not running as administrator.")).toBeTruthy();
    expect(screen.getByText("%LOCALAPPDATA%\\PeakTweaks\\startup-error.log")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "Home", level: 1 })).toBeTruthy();
  });

  it("labels sample data everywhere it is shown", async () => {
    renderApp();
    expect(await screen.findByText("Demo data, not this PC")).toBeTruthy();
  });

  it("keeps Apply locked until a restore point exists", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting A").closest("section, li") as HTMLElement;
    expect(within(card).getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(true);

    await goTo("Home");
    await userEvent.click(await screen.findByRole("button", { name: "Make a restore point" }));
    await screen.findByText(/Restore point #\d+ is ready\./);

    await goTo("Tools");
    const after = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    await waitFor(() => expect(within(after).getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(false));
  });

  it("a change with a trade-off cannot be applied until it is acknowledged", async () => {
    const base = createMockBackend({ gateOpen: true });
    const withTradeoff: Backend = {
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) =>
          t.id === "fixture.default" ? { ...t, tradeoff: "Sample trade-off text.", safety: "safe" as const } : t,
        ),
    };
    renderApp(withTradeoff);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    const apply = within(card).getByRole("button", { name: "Apply" });
    await waitFor(() => expect(screen.queryByText("Changes are locked until there is a restore point.")).toBeNull());
    expect(apply.hasAttribute("disabled")).toBe(true);
    await act(async () => {
      await userEvent.click(within(card).getByLabelText("I have read this"));
    });
    expect(apply.hasAttribute("disabled")).toBe(false);
  });
});

describe("agent brief", () => {
  it("an applied change that is now blocked keeps its Undo and says why it cannot be applied again", async () => {
    const base = createMockBackend({ gateOpen: true });
    const reason = { code: "anti_cheat_requirement" as const, trigger: "fortnite", message: "SAMPLE: blocked for the chosen game." };
    renderApp({
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) =>
          t.id === "fixture.default" ? { ...t, safety: "safe" as const, state: { status: "applied" as const }, blocked: reason } : t,
        ),
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(card).getByText("SAMPLE: blocked for the chosen game.")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Undo" }).hasAttribute("disabled")).toBe(false);
  });

  it("a change altered outside PeakTweaks offers both Apply again and Undo", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) =>
          t.id === "fixture.default" ? { ...t, safety: "safe" as const, state: { status: "drifted" as const } } : t,
        ),
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(card).getByText("Changed outside PeakTweaks since it was applied")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Apply again" }).hasAttribute("disabled")).toBe(false);
    expect(within(card).getByRole("button", { name: "Undo" }).hasAttribute("disabled")).toBe(false);
  });

  it("a change the plan does not include says so and cannot be applied, without an error first", async () => {
    const base = createMockBackend({ gateOpen: true });
    const reason = { code: "tier_required" as const, trigger: "pro", message: "This change needs the Pro plan." };
    renderApp({
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) => (t.id === "fixture.default" ? { ...t, safety: "safe" as const, blocked: reason } : t)),
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(card).getByText("This change needs the Pro plan.")).toBeTruthy();
    expect(within(card).getByText("Not applied")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(true);
  });
});

describe("restore point shown on Home", () => {
  it("names the recent restore point that unlocked changes, with when Windows made it", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    const callout = await screen.findByText(/Restore point #\d+ is ready\./);
    expect(callout.closest("[role]") ?? callout.parentElement).toBeTruthy();
    expect(await screen.findByText(/Windows made it on/)).toBeTruthy();
  });
});

describe("Starter scan (plan 6.4)", () => {
  it("groups what needs a look by who can fix it", async () => {
    const base = createMockBackend();
    renderApp({
      ...base,
      auditSystem: async () => {
        const a = await base.auditSystem();
        const hdd = {
          id: "storage.boot_disk",
          status: "attention" as const,
          title: "Windows is on a hard drive",
          reading: "SAMPLE reading.",
          remedy: "SAMPLE remedy.",
          guidedOnly: true,
          fixTweakId: null,
          fixBy: "hardware" as const,
        };
        return { ...a, scan: { findings: [hdd, ...a.scan.findings.filter((f) => f.id !== hdd.id)] } };
      },
    });
    const you = await screen.findByRole("heading", { name: /^You can fix \(\d+\)$/ });
    const hardware = screen.getByRole("heading", { name: "Needs different hardware (1)" });
    expect(within(hardware.closest("section")!).getByText("Windows is on a hard drive")).toBeTruthy();
    expect(within(you.closest("section")!).queryByText("Windows is on a hard drive")).toBeNull();
    expect(screen.queryByRole("heading", { name: /^PeakTweaks can fix/ })).toBeNull();
  });

  it("Print opens the folded list first, and a printed demo scan says it is SAMPLE data", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => {});
    renderApp();
    const button = await screen.findByRole("button", { name: "Print this scan" });
    const folded = screen.getByText(/What is already right/).closest("details")!;
    expect(folded.open).toBe(false);
    await userEvent.click(button);
    expect(print).toHaveBeenCalledOnce();
    expect(folded.open).toBe(true);
    expect(screen.getByText(/SAMPLE: demo data, not this PC\./)).toBeTruthy();
    print.mockRestore();
  });
});

describe("review regressions", () => {
  it("the restore step shows success as soon as the engine confirms, even while the audit re-reads", async () => {
    let slowAudit = false;
    const base = createMockBackend();
    renderApp({
      ...base,
      auditSystem: async () => {
        if (slowAudit) await new Promise((r) => setTimeout(r, 2_000));
        return base.auditSystem();
      },
    });
    const make = await screen.findByRole("button", { name: "Make a restore point" });
    slowAudit = true;
    await userEvent.click(make);
    expect(await screen.findByText(/Restore point #\d+ is ready\./)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Make a restore point" })).toBeNull();
  });

  it("a failed Undo on Backups shows why", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({ ...base, revertTweak: () => Promise.reject({ kind: "registry", path: "HKLM\\X", value: null, detail: "denied" }) });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Backups");
    const row = (await screen.findByText("Sample setting B")).closest("li") as HTMLElement;
    await userEvent.click(within(row).getByRole("button", { name: "Undo" }));
    expect(await within(row).findByText("Windows refused a settings change.")).toBeTruthy();
  });

  it("game cards say where each game is and what to change in its own settings", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Games");
    const card = (name: string) =>
      screen.getAllByRole("heading", { level: 3 }).find((h) => h.textContent === name)!.closest("li") as HTMLElement;
    await screen.findByRole("heading", { name: "Per game" });

    const fortnite = card("Fortnite");
    expect(within(fortnite).getByText(/Installed at D:\\Epic Games\\Fortnite \(D:, a hard drive\)/)).toBeTruthy();
    expect(within(fortnite).getByText("Rendering mode: Performance.")).toBeTruthy();
    expect(within(fortnite).getByText(/Source: Epic Games/)).toBeTruthy();
    // The sample PC is not a two-chip laptop, so no graphics-chip line.
    expect(within(fortnite).queryByText(/Graphics chip/)).toBeNull();

    const minecraft = card("Minecraft");
    expect(within(minecraft).getByText("Not found in the places PeakTweaks looks.")).toBeTruthy();
    expect(within(minecraft).getByText(/no Minecraft settings advice yet/)).toBeTruthy();
    expect(within(minecraft).queryByText(/Source:/)).toBeNull();
  });

  it("the proof guide walks before runs, one change, then after runs", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Proof");
    await userEvent.click(screen.getByRole("button", { name: "New comparison" }));
    await userEvent.type(screen.getByLabelText("Game program name"), "GuideTest.exe");
    await userEvent.click(screen.getByRole("button", { name: "Start" }));

    await screen.findByRole("heading", { name: "Steps" });
    const guide = () => screen.getByRole("heading", { name: "Steps" }).closest("section, div") as HTMLElement;
    const step = (title: string) => within(guide()).getByText(title).closest("li") as HTMLElement;
    expect(within(step("Record the game before the change")).getByText("0 of 3 runs.")).toBeTruthy();
    expect((screen.getByRole("radio", { name: "Before the change" }) as HTMLInputElement).checked).toBe(true);
    for (let i = 1; i <= 3; i += 1) {
      await userEvent.click(screen.getByRole("button", { name: "Record" }));
      await waitFor(() => within(step("Record the game before the change")).getByText(`${i} of 3 runs.`));
    }
    // Nothing changed since the before runs: the guide says so and points at Tools.
    expect(within(guide()).getByText("The same PeakTweaks changes are in place as during the before runs.")).toBeTruthy();
    expect((screen.getByRole("radio", { name: "Before the change" }) as HTMLInputElement).checked).toBe(true);

    await userEvent.click(within(guide()).getByRole("button", { name: "Open Tools" }));
    await screen.findByRole("heading", { name: "Tools", level: 1 });
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    await userEvent.click(within(card).getByRole("button", { name: "Apply" }));
    await within(card).findByRole("button", { name: "Undo" });

    await goTo("Proof");
    await userEvent.click(await screen.findByRole("button", { name: /GuideTest\.exe/ }));
    expect(await within(guide()).findByText("Changed since the before runs: Sample setting A.")).toBeTruthy();
    await waitFor(() =>
      expect((screen.getByRole("radio", { name: "After the change" }) as HTMLInputElement).checked).toBe(true),
    );
  });

  it("a new comparison fills in the program name of a game found on this PC, never over typed text", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Proof");
    await userEvent.click(screen.getByRole("button", { name: "New comparison" }));
    const program = screen.getByLabelText("Game program name") as HTMLInputElement;
    const game = screen.getByLabelText("Game (optional)");

    await userEvent.selectOptions(game, "fortnite");
    expect(program.value).toBe("FortniteClient-Win64-Shipping.exe");
    expect(screen.getByText(/Found on this PC: D:\\Epic Games\\Fortnite\\FortniteGame/)).toBeTruthy();

    // Not found in the sample: the filled-in name is cleared, not left behind.
    await userEvent.selectOptions(game, "minecraft");
    expect(program.value).toBe("");

    await userEvent.type(program, "javaw.exe");
    await userEvent.selectOptions(game, "fortnite");
    expect(program.value).toBe("javaw.exe");
  });

  it("a first launch shows the welcome once, step by step, and remembers it was closed", async () => {
    const backend = createMockBackend({ gateOpen: true, firstRun: true });
    const save = vi.spyOn(backend, "setSettings");
    renderApp(backend);
    const dialog = await screen.findByRole("dialog", { name: /Welcome to PeakTweaks: What PeakTweaks does/ });
    expect(within(dialog).getByText(/collects no data/)).toBeTruthy();
    expect(within(dialog).getByText("Step 1 of 3")).toBeTruthy();

    await userEvent.click(within(dialog).getByRole("button", { name: "Next" }));
    expect(screen.getByRole("dialog", { name: /Your safety net/ })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Back" }));
    expect(screen.getByRole("dialog", { name: /What PeakTweaks does/ })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    await userEvent.click(screen.getByRole("button", { name: "Get started" }));

    expect(screen.queryByRole("dialog")).toBeNull();
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ welcomeSeen: true })));
  });

  it("the welcome stays away once seen and can be shown again from Settings", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    expect(screen.queryByRole("dialog")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    await userEvent.click(await screen.findByRole("button", { name: "Show the welcome again" }));
    expect(await screen.findByRole("dialog", { name: /What PeakTweaks does/ })).toBeTruthy();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Backups lists the change PeakTweaks makes for a restore point, and Undo all covers it", async () => {
    renderApp(createMockBackend({ gateOpen: false }));
    await userEvent.click(await screen.findByRole("button", { name: "Make a restore point" }));
    expect(await screen.findByText(/Restore point #\d+ is ready\./)).toBeTruthy();
    await goTo("Backups");
    const row = (await screen.findByText("Allow a restore point on demand")).closest("li") as HTMLElement;
    expect(within(row).getByText(/Made by PeakTweaks so it can create a restore point/)).toBeTruthy();
    expect(within(row).getByRole("button", { name: "Undo" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Undo all" }).hasAttribute("disabled")).toBe(false);
  });

  it("Backups says where the undo files are for a PC that will not start", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Backups");
    const card = (await screen.findByRole("heading", { name: "If Windows will not start" })).closest("section, div") as HTMLElement;
    expect(within(card).getByText(/recover\.cmd/)).toBeTruthy();
    expect(within(card).getByText(/Safe Mode/)).toBeTruthy();
    expect(within(card).queryByText(/could not be brought up to date/)).toBeNull();
  });

  it("Backups says when the offline undo files could not be updated at start-up", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({ ...base, listJournal: () => Promise.resolve(fx.journalView) });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Backups");
    expect(await screen.findByText(/offline undo files could not be brought up to date/)).toBeTruthy();
  });

  it("Tools does not lock changes while it does not yet know about restore points", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({ ...base, auditSystem: () => new Promise(() => {}) }); // never answers
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    expect(screen.queryByText("Changes are locked until there is a restore point.")).toBeNull();
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(card).getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(false);
  });
});

describe("Home dashboard", () => {
  it("keeps each view's button named by the view alone, with its count as the description", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "What the scan found" });
    const nav = screen.getByRole("navigation", { name: "Main" });
    const home = within(nav).getByRole("button", { name: "Home" });
    const attention = (await createMockBackend().auditSystem()).scan.findings.filter((f) => f.status === "attention").length;
    expect(attention).toBeGreaterThan(0);
    await waitFor(() => expect(home.getAttribute("data-count")).toBe(String(attention)));
    expect(document.getElementById(home.getAttribute("aria-describedby")!)?.textContent).toBe(`${attention} worth a look`);
    // The headline counts the same findings.
    expect(screen.getByText(`${attention} things`)).toBeTruthy();
  });

  it("shows the hardware readings the engine reported, flagged by the scan, and says when one could not be read", async () => {
    const base = createMockBackend();
    renderApp({
      ...base,
      auditSystem: async () => {
        const a = await base.auditSystem();
        const hw = a.env.hardware!;
        return { ...a, env: { ...a.env, hardware: { ...hw, cpu: { state: "unknown" as const, reason: "SAMPLE: WMI failed" } } } };
      },
    });
    const pc = (await screen.findByRole("heading", { name: "Your PC" })).closest("section")!;
    const tile = (label: string) => within(pc).getByText(label).closest("li")!;
    await waitFor(() => expect(within(tile("Memory")).getByText("/ 3200 MT/s")).toBeTruthy());
    expect(within(tile("Memory")).getByText("2400")).toBeTruthy();
    expect(within(tile("Memory")).getByRole("img", { name: "Running at 2400 of a rated 3200 MT/s" })).toBeTruthy();
    expect(within(tile("Memory")).getByText("Worth a look")).toBeTruthy();
    expect(within(tile("Display")).getByText("/ 144 Hz")).toBeTruthy();
    expect(within(tile("Processor")).getByText("Could not tell")).toBeTruthy();
  });

  it("finds the graphics driver by vendor when Windows names the card differently, and shows memory speed without a rating", async () => {
    const base = createMockBackend();
    renderApp({
      ...base,
      auditSystem: async () => {
        const a = await base.auditSystem();
        const hw = a.env.hardware!;
        if (hw.gpuDrivers.state !== "yes" || hw.memory.state !== "yes") throw new Error("fixture changed");
        const drivers = hw.gpuDrivers.value.map((d) => ({ ...d, name: `${d.name} (WDDM)` }));
        const sticks = hw.memory.value.sticks.map((s) => ({ ...s, ratedMhz: null }));
        return {
          ...a,
          env: {
            ...a.env,
            hardware: {
              ...hw,
              gpuDrivers: { state: "yes" as const, value: drivers },
              memory: { state: "yes" as const, value: { ...hw.memory.value, sticks } },
            },
          },
        };
      },
    });
    const pc = (await screen.findByRole("heading", { name: "Your PC" })).closest("section")!;
    const tile = (label: string) => within(pc).getByText(label).closest("li")!;
    await waitFor(() => expect(within(tile("Graphics")).getByText(/driver 581\.80/)).toBeTruthy());
    expect(within(tile("Memory")).getByText("2400")).toBeTruthy();
    expect(within(tile("Memory")).getByText("MT/s")).toBeTruthy();
  });

  it("says the check did not finish when the first scan fails, instead of still checking", async () => {
    renderApp(createMockBackend({ failures: { auditSystem: { kind: "registry", path: "HKEY_LOCAL_MACHINE\\SAMPLE", value: null, detail: "SAMPLE" } } }));
    expect(await screen.findAllByText("The check did not finish.")).toHaveLength(2);
    expect(screen.queryByText(/Checking this PC\./)).toBeNull();
    await userEvent.click(screen.getAllByRole("button", { name: "Check again" })[0]!);
    await screen.findByRole("heading", { name: "What the scan found" });
    expect(screen.queryByText("The check did not finish.")).toBeNull();
  });

  it("puts the restore point first, then sends the user to Tools", async () => {
    renderApp();
    const step = await screen.findByRole("region", { name: "Next step" });
    await userEvent.click(await within(step).findByRole("button", { name: "Make a restore point" }));
    await userEvent.click(await within(step).findByRole("button", { name: "Open Tools" }));
    await screen.findByRole("heading", { name: "Tools", level: 1 });
  });
});

describe("a second copy of PeakTweaks", () => {
  it("says the app is already open and offers no Try again that cannot work", async () => {
    renderApp(createMockBackend({ failures: { context: { kind: "already_running" } } }));
    expect(await screen.findByRole("heading", { name: "PeakTweaks could not start" })).toBeTruthy();
    expect(screen.getByText("PeakTweaks is already open.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Try again" })).toBeNull();
  });
});

describe("tester build", () => {
  it("says so on every screen when the engine reports a tester build, and not otherwise", async () => {
    const base = createMockBackend();
    renderApp({ ...base, context: async () => ({ ...(await base.context()), testerBuild: true }) });
    await screen.findByRole("heading", { name: "What the scan found" });
    expect(screen.getByText("Tester build")).toBeTruthy();
    expect(screen.getByText(/Every plan is unlocked for testing/)).toBeTruthy();
    await goTo("Backups");
    expect(screen.getByText("Tester build")).toBeTruthy();
  });

  it("shows no tester label in a normal build", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "What the scan found" });
    expect(screen.queryByText("Tester build")).toBeNull();
  });
});
