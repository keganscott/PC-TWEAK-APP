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
// launch it with a debugging port. CI instead builds a test variant whose only
// difference is `--remote-debugging-port` in the window's browser arguments
// (e2e-tauri/tauri.e2e.conf.json), starts it, and attaches here. With
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
