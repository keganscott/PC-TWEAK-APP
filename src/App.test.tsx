import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

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
