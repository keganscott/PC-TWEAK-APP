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
