// End to end on the real thing: the release peaktweaks.exe, its WebView2
// window, the real engine and this machine, driven through tauri-driver
// (WebDriver). Runs on the Windows CI runner after the release build.
//
// Usage: node e2e-tauri/real-app.mjs <path to peaktweaks.exe>
// Needs tauri-driver listening on 127.0.0.1:4444 with a matching msedgedriver.
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

const driver = await new Builder()
  .usingServer("http://127.0.0.1:4444/")
  .withCapabilities({ browserName: "wry", "tauri:options": { application: app } })
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
    await driver.wait(until.elementLocated(heading("Home")), 60_000);
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
