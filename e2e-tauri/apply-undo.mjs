// Apply and Undo a real change through the real app, on the real registry, and
// check that the registry and the journal agree afterwards (agent brief,
// Phase 3 "done when"). Windows CI only.
//
// Runs against a `dev-stubs` test build: the restore gate is open (Windows
// Server has no System Restore) and the licence covers Pro, because the only
// change in the catalogue that a runner can make is the Pro mouse tweak. Every
// other part is the shipped code: user resolution, Transaction, journal, .reg
// backups, WinRegistry. Such a build is never uploaded or shipped.
//
// Usage: DEBUGGER_ADDRESS=127.0.0.1:9222 node e2e-tauri/apply-undo.mjs
// Needs msedgedriver on 127.0.0.1:4444 and the test build already running.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { Builder, By, until } from "selenium-webdriver";

const KEY = "HKCU\\Control Panel\\Mouse";
const VALUES = ["MouseSpeed", "MouseThreshold1", "MouseThreshold2"];
const TWEAK = "input.mouseaccel";
const DATA = join(process.env.ProgramData ?? "C:\\ProgramData", "PeakTweaks");

/** The value as `reg query` reports it, or null when it does not exist. */
function regValue(name) {
  try {
    const out = execFileSync("reg", ["query", KEY, "/v", name], { encoding: "utf8" });
    const line = out.split(/\r?\n/).find((l) => l.trim().startsWith(name));
    const m = line?.trim().match(/^\S+\s+(REG_\w+)\s*(.*)$/);
    return m ? `${m[1]} ${m[2]}` : null;
  } catch {
    return null;
  }
}
const snapshot = () => Object.fromEntries(VALUES.map((v) => [v, regValue(v)]));

function journal() {
  return readFileSync(join(DATA, "journal.jsonl"), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l))
    .filter((r) => r.tweakId === TWEAK);
}

const driver = await new Builder()
  .usingServer(process.env.WEBDRIVER_URL ?? "http://127.0.0.1:4444/")
  .withCapabilities({ browserName: "webview2", "ms:edgeOptions": { debuggerAddress: process.env.DEBUGGER_ADDRESS } })
  .build();

const heading = (t) => By.xpath(`//h1[normalize-space()='${t}']`);
const text = (t) => By.xpath(`//*[contains(normalize-space(), "${t}")]`);
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
const card = () => driver.findElement(By.xpath(`//li[.//h3[normalize-space()='Pointer precision']]`));

try {
  const before = snapshot();
  const recordsBefore = existsSync(join(DATA, "journal.jsonl")) ? journal().length : 0;
  console.log(`registry before: ${JSON.stringify(before)}`);

  await step("the dev-stubs build starts with changes unlocked", async () => {
    await driver.wait(until.elementLocated(heading("Home")), 60_000);
    await open("Tools");
    await driver.wait(until.elementLocated(text("Pointer precision")), 15_000);
    assert.equal((await driver.findElements(text("Changes are locked until there is a restore point."))).length, 0);
  });

  await step("Apply writes 0/0/0 through the journal, with backups", async () => {
    await (await card()).findElement(By.xpath(".//button[normalize-space()='Apply']")).click();
    await driver.wait(async () => (await (await card()).getText()).includes("Applied"), 30_000);
    const after = snapshot();
    console.log(`\n  registry after Apply: ${JSON.stringify(after)}`);
    for (const v of VALUES) assert.equal(after[v], "REG_SZ 0", v);
    const writes = journal().filter((r) => r.record === "write" && r.action === "apply");
    assert.ok(writes.length >= 3, `expected 3 apply writes, saw ${writes.length}`);
    for (const w of writes.slice(-3)) assert.ok(existsSync(join(DATA, w.backupFile)), `missing ${w.backupFile}`);
    const day = join(DATA, writes.at(-1).backupFile, "..");
    const session = readdirSync(day).filter((f) => f.startsWith("session_") && f.includes("mouseaccel"));
    assert.ok(session.length >= 1, `no session .reg in ${day}`);
    console.log(`  journal: ${writes.length} apply writes, session file ${session.at(-1)}`);
  });

  await step("Undo from Backups restores the exact prior values", async () => {
    await open("Backups");
    const row = await driver.wait(
      until.elementLocated(By.xpath(`//li[.//span[normalize-space()='Pointer precision']]`)),
      15_000,
    );
    await row.findElement(By.xpath(".//button[normalize-space()='Undo']")).click();
    await driver.wait(until.elementLocated(text("Nothing PeakTweaks changed is in effect.")), 30_000);
    const after = snapshot();
    console.log(`\n  registry after Undo: ${JSON.stringify(after)}`);
    assert.deepEqual(after, before);
  });

  await step("the journal agrees: the last commit is a revert and nothing is outstanding", async () => {
    const records = journal();
    const commits = records.filter((r) => r.record === "commit");
    assert.equal(commits.at(-1)?.action, "revert", JSON.stringify(commits.at(-1)));
    const lastRevertTx = commits.at(-1).txId;
    const outstanding = records.filter((r) => r.record === "write" && r.action === "apply" && r.seq > lastRevertTx);
    assert.equal(outstanding.length, 0, "apply writes after the revert");
    await open("Tools");
    await driver.wait(async () => (await (await card()).getText()).includes("Not applied"), 15_000);
    console.log(
      `  journal: ${records.length - recordsBefore} new records for ${TWEAK}; last commit revert (tx ${lastRevertTx})`,
    );
  });

  console.log("\nApply and Undo end to end: registry and journal agree.");
} finally {
  await driver.quit();
}
