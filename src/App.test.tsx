import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import * as fx from "./generated/fixtures";
import type { LiveReadings } from "./generated/LiveReadings";
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

  it("Home shows live readings, and says plainly when the graphics card is not read", async () => {
    const base = createMockBackend();
    let gpus: LiveReadings["gpus"] | null = null;
    renderApp({
      ...base,
      liveReadings: async () => {
        const r = await base.liveReadings();
        return gpus ? { ...r, gpus } : r;
      },
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    const section = (await screen.findByRole("heading", { name: "Right now" })).closest("section") as HTMLElement;
    expect(await within(section).findByRole("meter", { name: /^Processor: \d+ percent busy$/ })).toBeTruthy();
    expect(within(section).getByRole("meter", { name: "Memory: 57 percent in use" })).toBeTruthy();
    expect(within(section).getByText("Sample graphics card")).toBeTruthy();
    expect(within(section).getByText("SAMPLE")).toBeTruthy();
    cleanup();

    gpus = { state: "unknown", reason: "nvml.dll was not found" };
    renderApp({ ...base, liveReadings: async () => ({ ...(await base.liveReadings()), gpus: gpus! }) });
    expect(await screen.findByText("Not read: nvml.dll was not found. AMD and Intel cards are not read yet.")).toBeTruthy();
  });

  it("Home makes the restore point first, then applies the basic changes, in one click", async () => {
    renderApp();
    await screen.findByRole("heading", { name: "Home", level: 1 });
    expect(screen.queryByRole("button", { name: /^Apply all basic changes/ })).toBeNull();
    const card = screen.getByRole("region", { name: /basic change/ });
    await userEvent.click(within(card).getByRole("button", { name: "Make a restore point, then apply 1" }));
    expect(await screen.findByText(/Restore point #\d+ is ready\./)).toBeTruthy();
    expect(await within(card).findByText("Every basic change is in place.")).toBeTruthy();
  });

  it("Home applies nothing when the restore point fails", async () => {
    const backend = createMockBackend({ failures: { createRestorePoint: { kind: "command", what: "System Restore", exitCode: null, detail: "SAMPLE: Windows said no" } } });
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    const card = screen.getByRole("region", { name: /basic change/ });
    await userEvent.click(within(card).getByRole("button", { name: "Make a restore point, then apply 1" }));
    await waitFor(() => expect(within(card).getByRole("button", { name: "Make a restore point, then apply 1" }).hasAttribute("disabled")).toBe(false));
    expect(within(card).getByText("1 basic change to make")).toBeTruthy();
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

  it("Tools search narrows the list, points to Advanced matches, and every tool says what it changes", async () => {
    const base = createMockBackend();
    renderApp({
      ...base,
      listTweaks: async () =>
        (await base.listTweaks()).map((t) => (t.id === "fixture.blocked" ? { ...t, safety: "moderate" as const } : t)),
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const search = await screen.findByRole("searchbox", { name: "Search the tools" });
    expect(screen.getByText("Sample setting B")).toBeTruthy();

    await userEvent.type(search, "not applied");
    expect(screen.getByText("Sample setting A")).toBeTruthy();
    expect(screen.queryByText("Sample setting B")).toBeNull();
    const card = screen.getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(card).getByText("What this changes")).toBeTruthy();
    expect(within(card).getByText(/PeakTweaks\\Sample\\fixture\.default/)).toBeTruthy();

    await userEvent.clear(search);
    await userEvent.type(search, "not available");
    expect(screen.getByText("No tools match \"not available\".")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Show Advanced" }));
    expect(await screen.findByText("Sample setting D")).toBeTruthy();
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
    expect(await within(section).findByText("Watching for Fortnite, Roblox, Valorant, Counter-Strike 2, Apex Legends and Minecraft (Bedrock Edition).")).toBeTruthy();
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
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ gamingMode: true, gameTimer: false }), expect.anything());
    expect(await within(section).findByText(/Gaming Mode is on\./)).toBeTruthy();

    await userEvent.click(within(section).getByRole("switch", { name: "Game timer" }));
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ gamingMode: true, gameTimer: true }), expect.anything());
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
    expect(await within(section).findByText("4 of 6 start when you sign in.")).toBeTruthy();

    const chat = within(section).getByRole("switch", { name: "Sample chat app" }) as HTMLInputElement;
    expect(chat.checked).toBe(true);
    await userEvent.click(chat);
    expect(apply).toHaveBeenLastCalledWith("startup.user_run:Sample chat app");
    await waitFor(() => expect(chat.checked).toBe(false));
    expect(within(section).getByText("3 of 6 start when you sign in.")).toBeTruthy();

    await userEvent.click(chat);
    expect(revert).toHaveBeenLastCalledWith("startup.user_run:Sample chat app");
    await waitFor(() => expect(chat.checked).toBe(true));

    // A Store app's own startup task is listed with the rest.
    const store = within(section).getByRole("switch", { name: "Sample Store app" }) as HTMLInputElement;
    expect(store.checked).toBe(true);
    expect(store.disabled).toBe(false);
    expect(within(section).getAllByText(/A Microsoft Store app\./).length).toBe(1);

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

  it("Backups says when a change in effect was applied", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Tools");
    const card = (await screen.findAllByRole("listitem")).find((li) => li.textContent?.includes("Sample setting A"))!;
    await userEvent.click(within(card).getByRole("button", { name: "Apply" }));
    await within(card).findByText("Optimized");
    await goTo("Backups");
    const list = await screen.findByRole("region", { name: /Applied now/ });
    const row = within(list).getByText("Sample setting A").closest("li") as HTMLElement;
    expect(within(row).getByText(/^Applied /)).toBeTruthy();
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
        // Only the hard drive here; the SAMPLE scan's own one-click fixes are left out.
        return { ...a, scan: { findings: [hdd, ...a.scan.findings.filter((f) => f.id !== hdd.id && f.fixBy !== "us")] } };
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
        return { ...a, scan: { findings: [plan, ...a.scan.findings.filter((f) => f.id !== plan.id && f.fixBy !== "us")] } };
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
        return { ...a, scan: { findings: [plan, ...a.scan.findings.filter((f) => f.id !== plan.id && f.fixBy !== "us")] } };
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

    // A game from the list gets its card once it is the main game.
    expect(screen.getAllByRole("heading", { level: 3 }).some((h) => h.textContent === "Minecraft")).toBe(false);
    await userEvent.selectOptions(screen.getByLabelText("Or another game"), "minecraft");
    await waitFor(() => expect(screen.getAllByRole("heading", { level: 3 }).some((h) => h.textContent === "Minecraft")).toBe(true));
    const minecraft = card("Minecraft");
    expect(within(minecraft).getByText("Not found in the places PeakTweaks looks.")).toBeTruthy();
    expect(within(minecraft).getByText(/no Minecraft settings advice yet/)).toBeTruthy();
    expect(within(minecraft).queryByText(/Source:/)).toBeNull();
  });

  it("the main game is one of five shooters, or any game from the list of the most played", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Games");
    const buttons = screen.getAllByRole("radio").map((r) => r.closest("label")!.textContent);
    // The sample PC has Fortnite and Counter-Strike 2 installed, and the picker says so.
    expect(buttons).toEqual(["Fortnite on this PC", "Valorant", "Counter-Strike 2 on this PC", "Apex Legends", "Call of Duty"]);
    expect(screen.getByRole("radio", { name: "Fortnite on this PC" })).toBeTruthy();
    const list = screen.getByLabelText("Or another game") as HTMLSelectElement;
    // A placeholder, then 25 games in name order, none of them a button.
    const names = Array.from(list.options).slice(1).map((o) => o.text);
    expect(names).toHaveLength(25);
    expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b)));
    expect(names.filter((n) => fx.games.some((g) => g.featured && g.name === n))).toEqual([]);

    await userEvent.click(screen.getByRole("radio", { name: "Apex Legends" }));
    await waitFor(() => expect((screen.getByRole("radio", { name: "Apex Legends" }) as HTMLInputElement).checked).toBe(true));
    expect(list.value).toBe("");
    // The main game's card comes first, marked as such.
    const first = screen.getAllByRole("heading", { level: 3 })[0]!;
    expect(first.textContent).toBe("Apex Legends");
    expect(within(first.closest("li") as HTMLElement).getByText("Your main game")).toBeTruthy();
    expect(screen.getAllByText("Your main game")).toHaveLength(1);

    await userEvent.selectOptions(list, "rust");
    await waitFor(() => expect(list.value).toBe("rust"));
    expect(screen.getAllByRole("radio").every((r) => !(r as HTMLInputElement).checked)).toBe(true);
    // Looked for in Steam's libraries and not there.
    const card = (name: string) =>
      screen.getAllByRole("heading", { level: 3 }).find((h) => h.textContent === name)!.closest("li") as HTMLElement;
    expect(within(card("Rust")).getByText("Not found in the places PeakTweaks looks.")).toBeTruthy();
    // Not on Steam and not looked for, so the card does not claim it is missing.
    await userEvent.selectOptions(list, "league");
    await waitFor(() => expect(list.value).toBe("league"));
    expect(within(card("League of Legends")).queryByText("Not found in the places PeakTweaks looks.")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "No main game" }));
    await waitFor(() => expect(list.value).toBe(""));
    expect(screen.queryByRole("button", { name: "No main game" })).toBeNull();
  });

  it("a game found in a Steam library has a Play button that asks Steam to start it", async () => {
    const backend = createMockBackend({ gateOpen: true });
    const launch = vi.spyOn(backend, "launchGame");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Games");
    const card = async (name: string) =>
      (await screen.findAllByRole("heading", { level: 3 })).find((h) => h.textContent === name)!.closest("li") as HTMLElement;
    // Fortnite is found, but not through Steam: no Play button.
    expect(within(await card("Fortnite")).queryByRole("button", { name: /^Play/ })).toBeNull();
    const cs2 = await card("Counter-Strike 2");
    expect(within(cs2).getByText(/without PeakTweaks' administrator rights/)).toBeTruthy();
    expect(within(cs2).getByText(/^Gaming Mode is (on|off)/)).toBeTruthy();
    await userEvent.click(within(cs2).getByRole("button", { name: "Play Counter-Strike 2" }));
    expect(await within(cs2).findByText("Steam was asked to start it.")).toBeTruthy();
    expect(launch).toHaveBeenCalledWith("cs2");
  });

  it("a Play button that Steam could not be reached for says so", async () => {
    renderApp(
      createMockBackend({
        gateOpen: true,
        failures: { launchGame: { kind: "command", what: "Steam", exitCode: null, detail: "could not find the Windows desktop: SAMPLE" } },
      }),
    );
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Games");
    const heading = (await screen.findAllByRole("heading", { level: 3 })).find((h) => h.textContent === "Counter-Strike 2")!;
    const cs2 = heading.closest("li") as HTMLElement;
    await userEvent.click(within(cs2).getByRole("button", { name: "Play Counter-Strike 2" }));
    expect(await within(cs2).findByText("PeakTweaks could not ask Steam to start the game.")).toBeTruthy();
  });

  it("Proof explains a comparison until one is open, and starts one from there", async () => {
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Proof");
    const how = await screen.findByRole("region", { name: "How a comparison works" });
    expect(within(how).getAllByRole("listitem")).toHaveLength(4);
    await userEvent.click(within(how).getByRole("button", { name: "Start one" }));
    expect(await screen.findByLabelText("Game program name")).toBeTruthy();
    expect(screen.queryByRole("region", { name: "How a comparison works" })).toBeNull();
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
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({ welcomeSeen: true }), expect.anything()));
  });

  it("Settings keeps a choice not saved yet when the tray switches Gaming Mode", async () => {
    let push: ((s: import("./generated/Settings").Settings) => void) | undefined;
    const mock = createMockBackend({ gateOpen: true });
    renderApp({
      ...mock,
      onSettings: async (handler) => {
        push = handler;
        return () => {};
      },
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await userEvent.click(within(dialog).getByLabelText(/Technical/));
    const before = await mock.getSettings();
    act(() => push!({ ...before, gamingMode: !before.gamingMode }));
    expect((within(dialog).getByLabelText(/Technical/) as HTMLInputElement).checked).toBe(true);
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("Home waits for the first check before offering to make a restore point", async () => {
    let finish: (() => void) | undefined;
    const mock = createMockBackend({ gateOpen: true });
    renderApp({
      ...mock,
      auditSystem: async () => {
        await new Promise<void>((r) => (finish = r));
        return mock.auditSystem();
      },
    });
    expect(await screen.findByText("Checking for a restore point")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Make a restore point, then apply/ })).toBeNull();
    act(() => finish!());
    expect(await screen.findByRole("button", { name: /Apply all basic changes/ })).toBeTruthy();
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

  it("Home gently reminds about junk files and an old driver, and Not now or Settings put them away", async () => {
    const backend = createMockBackend({ gateOpen: true });
    const save = vi.spyOn(backend, "setSettings");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    // The sample change record cleared junk files in 2023; the sample driver is dated 2025-08-20.
    const reminders = await screen.findByRole("region", { name: "Reminders" });
    expect(within(reminders).getByText(/^Junk files were last cleared \d+ days ago\.$/)).toBeTruthy();
    expect(within(reminders).getByText("The Example GPU driver is dated 2025-08-20.")).toBeTruthy();
    expect(within(reminders).getByText(/PeakTweaks does not install drivers/)).toBeTruthy();

    await userEvent.click(within(reminders).getAllByRole("button", { name: "Not now" })[0]!);
    await waitFor(() => expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ cleanupReminderSnoozedUntil: expect.any(Number) }), expect.anything()));
    await waitFor(() => expect(screen.queryByText(/^Junk files were last cleared/)).toBeNull());
    expect(screen.getByText("The Example GPU driver is dated 2025-08-20.")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    await userEvent.click(await screen.findByRole("checkbox", { name: /Gentle reminders on Home/ }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Reminders" })).toBeNull());
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ remindersOff: true }), expect.anything());
  });

  it("after a restart, Home says whether the changes are still in place and names those that waited for it", async () => {
    const base = createMockBackend({ gateOpen: true });
    const save = vi.spyOn(base, "setSettings");
    // Sample setting B waits for a restart and was applied before the sample PC's last start.
    const waits = (list: Awaited<ReturnType<Backend["listTweaks"]>>) =>
      list.map((t) => (t.id === "fixture.applied" ? { ...t, requiresReboot: true } : t));
    const appliedBefore = async () => {
      const j = await base.listJournal();
      const write = j.records.find((r) => r.record === "write")!;
      return { ...j, records: [...j.records, { ...write, tweakId: "fixture.applied", unixMs: fx.contextInfo.bootedUnixMs! - 60_000 }] };
    };
    renderApp({
      ...base,
      listTweaks: async () => waits(await base.listTweaks()),
      rescan: async () => waits(await base.rescan()),
      listJournal: appliedBefore,
    });
    await screen.findByRole("heading", { name: "Home", level: 1 });
    const check = await screen.findByRole("region", { name: "After the restart" });
    expect(within(check).getByText(/^Windows restarted, and/)).toBeTruthy();
    expect(within(check).getByText("This change waited for the restart: Sample setting B.")).toBeTruthy();
    await userEvent.click(within(check).getByRole("button", { name: "Got it" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "After the restart" })).toBeNull());
    expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ restartCheckSeenBoot: fx.contextInfo.bootedUnixMs }), expect.anything());
  });

  it("Backups copies this PC's setup as text and applies a pasted one after checking it against this PC", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const backend = createMockBackend({ gateOpen: true });
    const apply = vi.spyOn(backend, "applyTweak");
    renderApp(backend);
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Backups");
    const card = screen.getByRole("heading", { name: "Copy this setup to another PC" }).closest("section, div") as HTMLElement;
    await userEvent.click(within(card).getByRole("button", { name: /^Copy setup/ }));
    expect(JSON.parse(String(writeText.mock.calls[0]?.[0]))).toEqual({ "peaktweaks-setup": 1, changes: ["fixture.applied"] });

    const paste = within(card).getByLabelText("Paste a setup from another PC");
    await userEvent.type(paste, "not a setup");
    await userEvent.click(within(card).getByRole("button", { name: "Check it" }));
    expect(within(card).getByText("That is not a setup copied from PeakTweaks.")).toBeTruthy();

    await userEvent.clear(paste);
    await userEvent.click(paste);
    await userEvent.paste('{"peaktweaks-setup":1,"changes":["fixture.default","fixture.applied","only.elsewhere"]}');
    await userEvent.click(within(card).getByRole("button", { name: "Check it" }));
    expect(within(card).getByText(/^Already in place here \(1\)/)).toBeTruthy();
    expect(within(card).getByText("Not available on this PC (1): only.elsewhere.")).toBeTruthy();
    await userEvent.click(within(card).getByRole("button", { name: "Apply 1 change" }));
    expect(apply).toHaveBeenCalledWith("fixture.default");
    expect(await within(card).findByText("Nothing in it is left to apply here.")).toBeTruthy();
    Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
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

  it("Copy summary puts a labelled list of this PC and its changes on the clipboard", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    try {
      renderApp(createMockBackend({ gateOpen: true }));
      await screen.findByRole("heading", { name: "Home", level: 1 });
      await goTo("Backups");
      await userEvent.click(screen.getByRole("button", { name: "Copy summary" }));
      expect(await screen.findByRole("button", { name: "Copied" })).toBeTruthy();
      const text = String(writeText.mock.calls[0]?.[0]);
      expect(text.split("\n")[0]).toBe("SAMPLE DATA: made up for testing, not a real PC");
      expect(text).toMatch(/^Processor: .+, \d+ cores/m);
      expect(text).toMatch(/^Changes in place \(\d+\)$/m);
    } finally {
      Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    }
  });

  it("Copy summary shows the text to select by hand when the clipboard refuses", async () => {
    Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    renderApp(createMockBackend({ gateOpen: true }));
    await screen.findByRole("heading", { name: "Home", level: 1 });
    await goTo("Backups");
    await userEvent.click(screen.getByRole("button", { name: "Copy summary" }));
    const box = await screen.findByRole("textbox", { name: "Summary of this PC and its changes" });
    expect((box as HTMLTextAreaElement).value).toContain("PeakTweaks summary, ");
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
