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

  it("makes the restore point from the lock message on Tools, in one click", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const lock = (await screen.findByText("Changes are locked until there is a restore point.")).closest(
      "[role=note]",
    ) as HTMLElement;
    await userEvent.click(within(lock).getByRole("button", { name: "Make a restore point" }));
    await waitFor(() => expect(screen.queryByText("Changes are locked until there is a restore point.")).toBeNull());
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    await waitFor(() => expect(within(card).getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(false));
  });

  it("lists a setting the PC already has as done, with nothing to apply", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting C").closest("li") as HTMLElement;
    expect(within(card).getByText("Already optimized")).toBeTruthy();
    expect(within(card).queryByRole("button", { name: "Apply" })).toBeNull();
    expect(screen.getByText(/of \d+ already optimized on this PC\./)).toBeTruthy();
  });

  it("Home lists the basic changes and applies them in one click; the result card undoes it", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    const card = await screen.findByRole("region", { name: "1 basic change to make" });
    expect(within(card).getByText("Sample setting A")).toBeTruthy();
    expect(within(card).getByText("Already optimized")).toBeTruthy();
    await userEvent.click(within(card).getByRole("button", { name: "Apply all basic changes (1)" }));
    expect(await screen.findByText("Applied: Sample setting A")).toBeTruthy();
    expect(within(card).getByRole("heading", { name: "Every basic change is in place." })).toBeTruthy();
    expect(within(card).getAllByText("Optimized").length).toBeGreaterThan(0);
    await userEvent.click(screen.getByRole("button", { name: "Undo these" }));
    expect(await screen.findByText("Undid: Sample setting A")).toBeTruthy();
  });

  it("Home lists changes set back outside PeakTweaks and applies them again in one click", async () => {
    const backend = createMockBackend({ gateOpen: true });
    const apply = vi.spyOn(backend, "applyTweak");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    const notice = await screen.findByRole("region", { name: "Changes set back" });
    expect(within(notice).getByText("1 change was set back outside PeakTweaks.")).toBeTruthy();
    expect(within(notice).getByText("Sample setting F")).toBeTruthy();
    await userEvent.click(within(notice).getByRole("button", { name: "Apply again" }));
    expect(apply).toHaveBeenCalledWith("fixture.drifted");
    await waitFor(() => expect(screen.queryByRole("region", { name: "Changes set back" })).toBeNull());
  });

  it("Home keeps the basic changes locked until there is a restore point", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    const button = await screen.findByRole("button", { name: "Apply all basic changes (1)" });
    expect(button.hasAttribute("disabled")).toBe(true);
    expect(screen.getByText("Make a restore point first, in the step above.")).toBeTruthy();
  });

  it("Home offers the basic changes whenever the gate is open, also before the restore status is read", async () => {
    // A dev-stubs build opens the gate without reading System Restore (real
    // app e2e, Windows CI run 37841431870): Home stayed on "Checking this PC".
    const backend = createMockBackend({ gateOpen: true });
    const auditSystem = backend.auditSystem.bind(backend);
    backend.auditSystem = async () => {
      const a = await auditSystem();
      return { ...a, env: { ...a.env, restore: null } };
    };
    renderApp(backend);
    expect(await screen.findByRole("button", { name: "Apply all basic changes (1)" })).toBeTruthy();
  });

  it("each Tools category has Apply recommended", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    await userEvent.click(await screen.findByRole("button", { name: "Apply recommended (1)" }));
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    await waitFor(() => expect(within(card).getByText("Optimized")).toBeTruthy());
  });

  it("Tools empties the standby list without a restore point and shows before and after", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = await screen.findByRole("region", { name: "Empty the standby list" });
    expect(within(card).getByText("SAMPLE")).toBeTruthy();
    await userEvent.click(within(card).getByRole("button", { name: "Empty it now" }));
    expect(await within(card).findByText(/6\.0 GB before, 1\.0 GB after/)).toBeTruthy();
  });

  it("Tools checks the connection and says where echoes were lost", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = await screen.findByRole("region", { name: "Check the connection" });
    expect(within(card).getByText("SAMPLE")).toBeTruthy();
    await userEvent.click(within(card).getByRole("button", { name: "Check now" }));
    expect(await within(card).findByText("Your router answered every echo, but some sent past it were lost.")).toBeTruthy();
    const router = within(card).getByRole("rowheader", { name: /Your router/ }).closest("tr") as HTMLElement;
    expect(within(router).getByText("20 of 20")).toBeTruthy();
    expect(within(router).getByText("1.8 ms")).toBeTruthy();
    expect(within(router).getByText("Through Wi-Fi")).toBeTruthy();
    const cloudflare = within(card).getByRole("rowheader", { name: /Cloudflare DNS/ }).closest("tr") as HTMLElement;
    expect(within(cloudflare).getByText("19 of 20")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Check again" })).toBeTruthy();
  });

  it("Tools says which games it watches for, before any runs", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /While you play/ });
    expect(await within(section).findByText("Watching for Fortnite, Roblox and Minecraft (Bedrock Edition).")).toBeTruthy();
    expect((within(section).getByRole("switch", { name: "Gaming Mode" }) as HTMLInputElement).checked).toBe(false);
  });

  it("Tools turns Gaming Mode and the game timer on from their switches and says what is in effect", async () => {
    const backend = createMockBackend({ playing: "fortnite", gateOpen: true });
    const save = vi.spyOn(backend, "setSettings");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /While you play/ });
    expect(await within(section).findByText("Fortnite is running.")).toBeTruthy();

    await userEvent.click(within(section).getByRole("switch", { name: "Gaming Mode" }));
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ gamingMode: true, gameTimer: false }));
    expect(await within(section).findByText(/Gaming Mode is on\./)).toBeTruthy();

    await userEvent.click(within(section).getByRole("switch", { name: "Game timer" }));
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ gamingMode: true, gameTimer: true }));
    expect(await within(section).findByText(/The game timer is held at 0\.5 ms\./)).toBeTruthy();
    expect(within(section).getAllByText("On now")).toHaveLength(2);
  });

  it("Tools says when the running game is on Wi-Fi only", async () => {
    renderApp(createMockBackend({ playing: "roblox", onWifi: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /While you play/ });
    expect(await within(section).findByText("This game is running over Wi-Fi.")).toBeTruthy();
  });

  it("Tools says nothing about Wi-Fi while no game runs", async () => {
    renderApp(createMockBackend({ onWifi: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /While you play/ });
    await within(section).findByText(/Watching for/);
    expect(within(section).queryByText("This game is running over Wi-Fi.")).toBeNull();
  });

  it("Tools says why Gaming Mode is not in effect without a restore point", async () => {
    renderApp(createMockBackend({ playing: "roblox" }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /While you play/ });
    expect(within(section).getByText(/it needs a restore point first/)).toBeTruthy();
    await userEvent.click(within(section).getByRole("switch", { name: "Gaming Mode" }));
    expect(await within(section).findByText(/no verified restore point/)).toBeTruthy();
    expect(within(section).queryByText("On now")).toBeNull();
  });

  it("Tools lists startup apps; a switch turns one off and back on, and Windows Security is not offered", async () => {
    const backend = createMockBackend({ gateOpen: true });
    const apply = vi.spyOn(backend, "applyTweak");
    const revert = vi.spyOn(backend, "revertTweak");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /Startup apps/ });
    expect(await within(section).findByText("3 of 5 start when you sign in.")).toBeTruthy();

    const chat = within(section).getByRole("switch", { name: "Sample chat app" }) as HTMLInputElement;
    expect(chat.checked).toBe(true);
    await userEvent.click(chat);
    expect(apply).toHaveBeenLastCalledWith("startup.user_run:Sample chat app");
    await waitFor(() => expect(chat.checked).toBe(false));
    expect(within(section).getByText("2 of 5 start when you sign in.")).toBeTruthy();

    await userEvent.click(chat);
    expect(revert).toHaveBeenLastCalledWith("startup.user_run:Sample chat app");
    await waitFor(() => expect(chat.checked).toBe(true));

    const security = within(section).getByRole("switch", { name: "SecurityHealth" }) as HTMLInputElement;
    expect(security.checked).toBe(true);
    expect(security.disabled).toBe(true);
    expect(within(section).getByText("This starts Windows Security.", { exact: false })).toBeTruthy();
    expect(within(section).getByText(/never turns off security software/)).toBeTruthy();
    // Turned off in Task Manager: shown as off, and can be turned back on;
    // switching it off again undoes that.
    const updater = within(section).getByRole("switch", { name: "Sample updater" }) as HTMLInputElement;
    expect([updater.checked, updater.disabled]).toEqual([false, false]);
    expect(within(section).getByText(/Turned off outside PeakTweaks/)).toBeTruthy();
    await userEvent.click(updater);
    expect(apply).toHaveBeenLastCalledWith("startup.user_run.on:Sample updater");
    await waitFor(() => expect(updater.checked).toBe(true));
    expect(within(section).getByText("Turned back on by PeakTweaks.")).toBeTruthy();
    await userEvent.click(updater);
    expect(revert).toHaveBeenLastCalledWith("startup.user_run.on:Sample updater");
    await waitFor(() => expect(updater.checked).toBe(false));
  });

  it("Tools lists MSI mode per device only under Advanced, and applying one asks first", async () => {
    const backend = createMockBackend({ gateOpen: true });
    const list = vi.spyOn(backend, "listMsiDevices");
    const apply = vi.spyOn(backend, "applyTweak");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    expect(screen.queryByRole("region", { name: /Devices/ })).toBeNull();
    expect(list).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("switch", { name: /Advanced/ }));
    const section = await screen.findByRole("region", { name: /Devices/ });
    const gpu = (await within(section).findByText("MSI mode: Sample graphics card")).closest("li") as HTMLElement;
    const nic = within(section).getByText("MSI mode: Sample network adapter").closest("li") as HTMLElement;
    expect(within(nic).getByText("Already optimized")).toBeTruthy();
    expect(within(nic).queryByRole("button", { name: "Apply" })).toBeNull();

    expect(within(gpu).getByText("Takes effect after a restart.")).toBeTruthy();
    const button = within(gpu).getByRole("button", { name: "Apply" });
    expect(button.hasAttribute("disabled")).toBe(true);
    await userEvent.click(within(gpu).getByLabelText("I have read this"));
    await userEvent.click(button);
    expect(apply).toHaveBeenLastCalledWith("msi.PCI\\VEN_10DE&DEV_2484&SUBSYS_146710DE&REV_A1\\4&2b0b1f0c&0&0008");
    expect(await within(gpu).findByText("Optimized")).toBeTruthy();
    expect(within(gpu).getByRole("button", { name: "Undo" })).toBeTruthy();
  });

  it("Tools says plainly when the devices could not be listed", async () => {
    const base = createMockBackend();
    renderApp({ ...base, listMsiDevices: async () => ({ devices: [], problem: "SAMPLE: the device query did not answer." }) });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    await userEvent.click(screen.getByRole("switch", { name: /Advanced/ }));
    const section = await screen.findByRole("region", { name: /Devices/ });
    expect(await within(section).findByText("The devices could not be listed.")).toBeTruthy();
    expect(within(section).getByText("SAMPLE: the device query did not answer.")).toBeTruthy();
    expect(within(section).queryByText(/No graphics card/)).toBeNull();
  });

  it("Tools keeps startup switches locked until there is a restore point", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const section = await screen.findByRole("region", { name: /Startup apps/ });
    const chat = (await within(section).findByRole("switch", { name: "Sample chat app" })) as HTMLInputElement;
    await waitFor(() => expect(chat.disabled).toBe(true));
    // Ours to turn back on, whatever the gate; turned off elsewhere needs one.
    expect((within(section).getByRole("switch", { name: "Sample game launcher" }) as HTMLInputElement).disabled).toBe(false);
    expect((within(section).getByRole("switch", { name: "Sample updater" }) as HTMLInputElement).disabled).toBe(true);
  });

  it("Tools shows junk sizes first, asks once, then says what it deleted and what was left", async () => {
    const backend = createMockBackend();
    const run = vi.spyOn(backend, "cleanupRun");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = await screen.findByRole("region", { name: "Clear out junk files" });
    expect(within(card).getByText("SAMPLE")).toBeTruthy();
    expect(await within(card).findByText("2.3 GB")).toBeTruthy();
    // Clearing shader caches has a cost, so they start unselected.
    expect((within(card).getByRole("checkbox", { name: /Shader caches/ }) as HTMLInputElement).checked).toBe(false);
    expect(within(card).getByText("Selected: 6.1 GB")).toBeTruthy();

    await userEvent.click(within(card).getByRole("button", { name: "Delete selected files" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete these files?" });
    expect(within(dialog).getByText(/cannot be undone/)).toBeTruthy();
    expect(run).not.toHaveBeenCalled();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete 6.1 GB" }));

    expect(await within(card).findByText(/Deleted 6\.0 GB in 5,036 files/)).toBeTruthy();
    expect(within(card).getByText(/22 files were left/)).toBeTruthy();
    expect(run).toHaveBeenCalledWith(["user_temp", "windows_temp", "thumbnails", "crash_dumps"]);
  });

  it("Tools optimizes the Windows drive, holds the junk cleanup meanwhile, then says how long Windows took", async () => {
    const base = createMockBackend();
    let finish = () => {};
    const held = new Promise<void>((resolve) => (finish = resolve));
    renderApp({ ...base, optimizeDrive: async () => held.then(() => base.optimizeDrive()) });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = await screen.findByRole("region", { name: "Optimize the Windows drive" });
    expect(within(card).getByText("SAMPLE")).toBeTruthy();
    // The sample PC's Windows drive is an SSD.
    expect(await within(card).findByText(/is an SSD, so Windows tells it which space is no longer in use/)).toBeTruthy();

    await userEvent.click(within(card).getByRole("button", { name: "Optimize now" }));
    expect(await within(card).findByText(/Windows is working on it/)).toBeTruthy();
    // The engine runs long work one at a time.
    const junk = screen.getByRole("region", { name: "Clear out junk files" });
    expect(within(junk).getByText("Available again when the drive optimization finishes.")).toBeTruthy();
    expect(within(junk).getByRole("button", { name: "Delete selected files" }).hasAttribute("disabled")).toBe(true);

    await act(async () => finish());
    expect(await within(card).findByText(/\(drive C:\)\. Windows took 41 seconds\./)).toBeTruthy();
    expect(within(junk).queryByText("Available again when the drive optimization finishes.")).toBeNull();
  });

  it("Backups lists each one-time action in the change record with what it did", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Backups");
    // The SAMPLE record already holds a cleanup.
    expect(await screen.findByText("Cleared junk files: deleted 5.8 GB in 5,027 files, 22 left in place")).toBeTruthy();

    await goTo("Tools");
    const card = await screen.findByRole("region", { name: "Optimize the Windows drive" });
    await userEvent.click(within(card).getByRole("button", { name: "Optimize now" }));
    await within(card).findByText(/Windows took 41 seconds/);
    await goTo("Backups");
    expect(await screen.findByText("Optimized the Windows drive: drive C:, Windows took 41 seconds")).toBeTruthy();
  });

  it("a safe change with a cost shows one line and needs no confirmation", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) => (t.id === "fixture.default" ? { ...t, tradeoff: "Needs a restart." } : t)),
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(card).getByText("Needs a restart.")).toBeTruthy();
    expect(within(card).queryByLabelText("I have read this")).toBeNull();
    await waitFor(() => expect(within(card).getByRole("button", { name: "Apply" }).hasAttribute("disabled")).toBe(false));
  });

  it("an Advanced change cannot be applied until it is acknowledged", async () => {
    const base = createMockBackend({ gateOpen: true });
    const withTradeoff: Backend = {
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) =>
          t.id === "fixture.default" ? { ...t, tradeoff: "Sample trade-off text.", safety: "moderate" as const } : t,
        ),
    };
    renderApp(withTradeoff);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    await userEvent.click(screen.getByRole("switch", { name: /Advanced/ }));
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

  it("a restart is said once: not twice beside a cost line that says it, nor for a setting already there", async () => {
    const base = createMockBackend({ gateOpen: true });
    const listTweaks = async () =>
      (await base.listTweaks()).map((t) =>
        t.id === "fixture.default"
          ? { ...t, safety: "moderate" as const, tradeoff: "Needs a restart.", requiresReboot: true }
          : t.id === "fixture.foreign"
            ? { ...t, requiresReboot: true }
            : t,
      );
    renderApp({ ...base, listTweaks, rescan: listTweaks });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    await userEvent.click(screen.getByRole("switch", { name: /Advanced/ }));
    const card = (await screen.findByText("Sample setting A")).closest("li") as HTMLElement;
    expect(within(card).getByText("Needs a restart.")).toBeTruthy();
    expect(within(card).queryByText("Takes effect after a restart.")).toBeNull();
    const foreign = screen.getByText("Sample setting C").closest("li") as HTMLElement;
    expect(within(foreign).queryByText("Takes effect after a restart.")).toBeNull();

    await userEvent.click(within(card).getByLabelText("I have read this"));
    await userEvent.click(within(card).getByRole("button", { name: "Apply" }));
    expect(await within(card).findByText("Takes effect after a restart.")).toBeTruthy();
    expect(within(card).queryByText("Needs a restart.")).toBeNull();
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

  it("a finding PeakTweaks can fix carries its tool, and Apply there applies it", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({
      ...base,
      auditSystem: async () => {
        const a = await base.auditSystem();
        const plan = {
          id: "power.plan",
          status: "attention" as const,
          title: "The power plan is not a performance plan",
          reading: "SAMPLE reading.",
          remedy: "SAMPLE remedy.",
          guidedOnly: false,
          fixTweakId: "fixture.default",
          fixBy: "us" as const,
        };
        return { ...a, scan: { findings: [plan, ...a.scan.findings.filter((f) => f.id !== plan.id)] } };
      },
    });
    const us = await screen.findByRole("heading", { name: "PeakTweaks can fix (1)" });
    const group = us.closest("section")!;
    expect(within(group).getByText("The power plan is not a performance plan")).toBeTruthy();
    expect(within(group).getByText("Sample setting A")).toBeTruthy();
    await userEvent.click(within(group).getByRole("button", { name: "Apply" }));
    expect(await within(group).findByRole("button", { name: "Undo" })).toBeTruthy();
    expect(within(group).getByText("Optimized")).toBeTruthy();
  });

  it("a finding whose tool the engine does not list shows no card", async () => {
    const base = createMockBackend({ gateOpen: true });
    renderApp({
      ...base,
      auditSystem: async () => {
        const a = await base.auditSystem();
        const plan = {
          id: "power.plan",
          status: "attention" as const,
          title: "The power plan is not a performance plan",
          reading: "SAMPLE reading.",
          remedy: "SAMPLE remedy.",
          guidedOnly: false,
          fixTweakId: "no.such.tool",
          fixBy: "us" as const,
        };
        return { ...a, scan: { findings: [plan, ...a.scan.findings.filter((f) => f.id !== plan.id)] } };
      },
    });
    const group = (await screen.findByRole("heading", { name: "PeakTweaks can fix (1)" })).closest("section")!;
    expect(within(group).getByText("SAMPLE remedy.")).toBeTruthy();
    expect(within(group).queryByRole("button")).toBeNull();
  });

  it("Print opens the folded list first, and a printed demo scan says it is SAMPLE data", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => {});
    renderApp();
    const button = await screen.findByRole("button", { name: "Print this scan" });
    const folded = screen.getByText(/Already good/).closest("details")!;
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

  it("a change this PC cannot take (seen only when its state is read) shows why and offers no Apply", async () => {
    const base = createMockBackend({ gateOpen: true });
    const reason = { code: "hardware_unsupported" as const, trigger: null, message: "This PC does not have both a network cable port and Wi-Fi." };
    const listTweaks = async () =>
      (await base.listTweaks()).map((t) => (t.id === "fixture.default" ? { ...t, state: { status: "blocked" as const, reason }, blocked: null } : t));
    renderApp({ ...base, listTweaks, rescan: listTweaks });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    // Listed apart, folded, with the engine's reason and nothing to press.
    const folded = (await screen.findByText("Sample setting A")).closest("details") as HTMLDetailsElement;
    expect(folded.open).toBe(false);
    expect(within(folded).getByText("1 change does not apply to this PC")).toBeTruthy();
    expect(within(folded).getByText(`: ${reason.message}`, { exact: false })).toBeTruthy();
    expect(within(folded).queryByRole("button")).toBeNull();
    // Not counted: Sample setting A and D (blocked in the sample) cannot be made here.
    const count = screen.getByText(/already optimized on this PC\.$/).textContent ?? "";
    expect(count).toMatch(new RegExp(` of ${(await base.listTweaks()).length - 2} already`));
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
