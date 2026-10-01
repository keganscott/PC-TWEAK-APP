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
