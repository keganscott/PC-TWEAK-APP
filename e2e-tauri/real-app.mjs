// End to end on the real thing: the release peaktweaks.exe, its WebView2
// window, the real engine and this machine, driven through WebDriver
// (WebDriver) via msedgedriver. Runs on the Windows CI runner after the release build.
//
// Usage: node e2e-tauri/real-app.mjs <path to peaktweaks.exe>
//        DEBUGGER_ADDRESS=localhost:9222 to attach to an already running test build
// Needs msedgedriver (matching the WebView2 runtime's version) on 127.0.0.1:4444.
//
// Prints what each screen shows (evidence for NOTES.md) and fails on the first
// broken expectation.

import assert from "node:assert/strict";

import { Builder, By, until } from "selenium-webdriver";

const app = process.argv[2];
if (!app) {
  console.error("usage: node e2e-tauri/real-app.mjs <path to peaktweaks.exe>");
  process.exit(2);
}

// The WebView2 runtime ignores WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS for this
// app (it uses the arguments the app sets; NOTES.md N46), so msedgedriver cannot
// launch it with a debugging port. CI instead builds a test variant that differs
// in two ways (e2e-tauri/tauri.e2e.conf.json): `--remote-debugging-port` in the
// window's browser arguments, and a capability without allow-revert-all so the
// boundary check below has a registered command to be refused. It starts that
// build and attaches here. With
// DEBUGGER_ADDRESS unset this falls back to letting msedgedriver launch `app`.
const debuggerAddress = process.env.DEBUGGER_ADDRESS;
const edgeOptions = debuggerAddress ? { debuggerAddress } : { binary: app, webviewOptions: {} };
const driver = await new Builder()
  .usingServer(process.env.WEBDRIVER_URL ?? "http://127.0.0.1:4444/")
  .withCapabilities({ browserName: "webview2", "ms:edgeOptions": edgeOptions })
  .build();

const heading = (text) => By.xpath(`//h1[normalize-space()='${text}']`);
const text = (t) => By.xpath(`//*[contains(normalize-space(), "${t}")]`);
const bodyText = () => driver.findElement(By.css("body")).getText();
const step = async (name, fn) => {
  process.stdout.write(`- ${name} ... `);
  await fn();
  console.log("ok");
};

/** Call a command from inside the page, as any script running there could. */
const ipc = (cmd, args) =>
  driver.executeAsyncScript(
    `const [cmd, args, done] = arguments;
     window.__TAURI_INTERNALS__.invoke(cmd, args ?? {}).then(
       (value) => done({ ok: true, value }),
       (error) => done({ ok: false, error }),
     );`,
    cmd,
    args ?? null,
  );

async function open(view) {
  const nav = await driver.findElement(By.css('nav[aria-label="Main"]'));
  await nav.findElement(By.xpath(`.//button[normalize-space()='${view}']`)).click();
  await driver.wait(until.elementLocated(heading(view)), 15_000);
}

async function show(view) {
  const t = await bodyText();
  console.log(`\n===== ${view} =====\n${t.slice(0, 1500)}\n`);
  return t;
}

try {
  await step("the real engine starts and Home renders", async () => {
    // Either Home, or the start-up failure screen with the engine's reason.
    const started = await driver.wait(
      async () =>
        (await driver.findElements(heading("Home"))).length > 0 ||
        (await driver.findElements(heading("PeakTweaks could not start"))).length > 0,
      60_000,
    );
    assert.ok(started);
    if ((await driver.findElements(heading("PeakTweaks could not start"))).length > 0) {
      await show("Start-up failure");
      assert.fail("the engine did not start; its reason is printed above");
    }
  });

  await step("no SAMPLE data inside the real app", async () => {
    const t = await bodyText();
    assert.ok(!t.includes("Demo data, not this PC"), "the SAMPLE banner is showing inside the real app");
    assert.ok(!t.includes("SAMPLE"), "a SAMPLE label is showing inside the real app");
  });

  await step("the scan of this machine arrives", async () => {
    await driver.wait(until.elementLocated(text("What the scan found")), 120_000);
  });
  const home = await show("Home");

  await step("the restore lock reflects this machine", async () => {
    // GitHub's runners are Windows Server: no System Restore. A client PC shows Step 1.
    const server = home.includes("This edition of Windows has no System Restore.");
    const client = home.includes("Step 1: make a restore point") || home.includes("Restore point ready");
    assert.ok(server || client, "neither the restore step nor the no-System-Restore notice is shown");
  });

  await step("Tools lists the real catalogue", async () => {
    await open("Tools");
    await driver.wait(until.elementLocated(text("Pointer precision")), 15_000);
  });
  await show("Tools");

  await step("Games shows the firmware security features", async () => {
    await open("Games");
    await driver.wait(until.elementLocated(text("Secure Boot")), 30_000);
  });
  await show("Games");

  await step("Proof opens", async () => {
    await open("Proof");
  });

  await step("Backups shows the change record", async () => {
    await open("Backups");
    await driver.wait(until.elementLocated(text("Change record")), 15_000);
  });
  await show("Backups");

  // N10: the permission boundary, probed from inside the page the way injected
  // script would. This build's capability is the shipped one minus
  // allow-revert-all (tauri.e2e.conf.json; command_audit.rs keeps them in sync).
  await step("the IPC permission boundary holds and forged state is ignored", async () => {
    await driver.manage().setTimeouts({ script: 60_000 });
    const lines = [];
    const brief = (v) =>
      String(typeof v === "string" ? v : (JSON.stringify(v) ?? v))
        .replace(/\s+/g, " ")
        .slice(0, 240);

    const ctx = await ipc("engine_context");
    assert.ok(ctx.ok, `engine_context failed: ${brief(ctx.error)}`);
    const title = await ipc("plugin:window|title", { label: "main" });
    assert.ok(title.ok, `a core:default read was refused: ${brief(title.error)}`);
    lines.push(`allowed (control): engine_context, plugin:window|title -> ${brief(title.value)}`);

    for (const [why, cmd, args] of [
      ["registered, permission removed in this build", "revert_all", undefined],
      ["not registered", "delete_everything", undefined],
      ["core command outside core:default", "plugin:window|set_title", { label: "main", value: "changed by the page" }],
      ["plugin not in the app", "plugin:shell|execute", { program: "cmd", args: ["/c", "echo"] }],
      ["plugin not in the app", "plugin:fs|read_text_file", { path: "C:\\Windows\\win.ini" }],
      ["plugin not in the app", "plugin:http|fetch", { clientConfig: { url: "https://example.com", method: "GET" } }],
    ]) {
      const r = await ipc(cmd, args);
      lines.push(`${why}: ${cmd} -> ${r.ok ? "ALLOWED " + brief(r.value) : "refused: " + brief(r.error)}`);
      assert.ok(!r.ok, `${cmd} was allowed (${why})`);
      assert.match(brief(r.error), /not allowed/i, `${cmd} failed for another reason than the ACL`);
    }
    const after = await ipc("plugin:window|title", { label: "main" });
    assert.equal(after.value, title.value, "the window title changed");

    // Plan section 12: env, licence, tier and gate state never come from the UI.
    const tweaks = await ipc("list_tweaks");
    assert.ok(tweaks.ok && tweaks.value.length > 0, "no tweaks listed");
    const before = await ipc("list_journal");
    const id = tweaks.value[0].id;
    const forged = await ipc("apply_tweak", {
      id,
      tier: "ultimate",
      license: { tier: "ultimate" },
      gateOpen: true,
      restoreGateOpen: true,
      env: { restoreGateOpen: true },
    });
    const journal = await ipc("list_journal");
    lines.push(
      `apply_tweak ${id} with forged tier/licence/gate/env -> ${forged.ok ? "APPLIED" : "refused: " + brief(forged.error)}`,
    );
    assert.ok(!forged.ok, "a forged apply went through");
    assert.equal(
      forged.error?.kind,
      "blocked",
      "the forged apply failed for another reason than the engine's own checks",
    );
    assert.equal(journal.value.records.length, before.value.records.length, "the journal changed");

    console.log(`\n===== IPC boundary =====\n${lines.join("\n")}\n`);
  });

  await step("the activity log opens and has the engine's messages", async () => {
    await driver.findElement(By.css('button[aria-controls="execution-bus"]')).click();
    const log = await driver.findElement(By.id("execution-bus"));
    await driver.wait(until.elementIsVisible(log), 5_000);
    console.log(`\n===== Activity log =====\n${(await log.getText()).slice(0, 800)}\n`);
  });

  console.log("\nAll real-app checks passed.");
} finally {
  await driver.quit();
}
