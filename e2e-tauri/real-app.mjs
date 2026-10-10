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

/** The innermost section around a heading, looked up afresh each time (React re-renders it). */
const sectionOf = (title) =>
  By.xpath(`//*[self::h2 or self::h3][normalize-space()='${title}']/ancestor::section[1]`);

/** Wait until the section shows one of `done` and nothing in it is busy, or
 * shows an engine error; fail on the error and return the section's text. */
async function settled(title, done, ms) {
  let error = null;
  await driver.wait(async () => {
    const section = await driver.findElement(sectionOf(title));
    const t = await section.getText();
    if (done.some((d) => t.includes(d)) && (await section.findElements(By.css("[aria-busy='true']"))).length === 0) {
      return true;
    }
    const alerts = await section.findElements(By.css("[role='alert']"));
    if (alerts.length > 0) error = await alerts[0].getText();
    return error !== null;
  }, ms);
  const t = await (await driver.findElement(sectionOf(title))).getText();
  assert.equal(error, null, `${title}: ${error}\n${t}`);
  return t;
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

  await step("a first launch shows the welcome, and closing it is remembered by the engine", async () => {
    const welcome = By.xpath(`//*[@role='dialog'][.//*[contains(normalize-space(), 'Welcome to PeakTweaks')]]`);
    const shown = await driver
      .wait(until.elementLocated(welcome), 10_000)
      .then(() => true)
      .catch(() => false);
    if (!shown) {
      // Settings on this runner may already say it was seen (an earlier run).
      const settings = await ipc("get_settings");
      assert.ok(settings.ok && settings.value.welcomeSeen, "no welcome, and the engine says it was never seen");
      console.log("(not shown: already seen on this machine)");
      return;
    }
    const dialog = await driver.findElement(welcome);
    console.log(`\n===== Welcome =====\n${(await dialog.getText()).slice(0, 600)}\n`);
    for (const label of ["Next", "Next", "Get started"]) {
      await (await driver.findElement(By.xpath(`//*[@role='dialog']//button[normalize-space()='${label}']`))).click();
    }
    await driver.wait(async () => (await driver.findElements(welcome)).length === 0, 10_000);
    await driver.wait(async () => {
      const settings = await ipc("get_settings");
      return settings.ok && settings.value.welcomeSeen === true;
    }, 10_000);
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

  await step("Home shows live readings of this machine", async () => {
    // Real readings: the processor and memory meters, and the graphics card
    // as read (a runner has no NVIDIA card, so it says why).
    const cpu = await driver.wait(
      until.elementLocated(By.xpath("//*[@role='meter' and starts-with(@aria-label, 'Processor: ')]")),
      30_000,
    );
    const memory = await driver.findElement(By.xpath("//*[@role='meter' and starts-with(@aria-label, 'Memory: ')]"));
    const section = await driver.findElement(sectionOf("Right now")).getText();
    console.log(
      `\n===== Right now =====\n${await cpu.getAttribute("aria-label")}\n${await memory.getAttribute("aria-label")}\n${section.slice(0, 600)}\n`,
    );
  });

  await step("Tools lists the real catalogue", async () => {
    await open("Tools");
    await driver.wait(until.elementLocated(text("Pointer precision")), 15_000);
    await driver.wait(until.elementLocated(text("TCP settings: auto-tuning and RSS on, ECN off")), 15_000);
  });
  await show("Tools");

  await step("every tool's state is read on this machine", async () => {
    const tweaks = await ipc("list_tweaks");
    assert.ok(tweaks.ok, `list_tweaks failed: ${JSON.stringify(tweaks.error)}`);
    const why = (s) =>
      s.status === "blocked" ? ` (${s.reason.code}) ${s.reason.message}` : s.status === "unknown" ? `: ${s.detail}` : "";
    const lines = tweaks.value.map((t) => `${t.id}: ${t.state.status}${why(t.state)}`);
    console.log(`\n===== Tool states (${tweaks.value.length}) =====\n${lines.join("\n")}\n`);
    const unread = tweaks.value.filter((t) => t.state.status === "unknown").map((t) => t.id);
    assert.deepEqual(unread, [], "these tools could not be read on this machine");
  });

  // Tools' read-only sections, through the real commands on this machine:
  // each shows what it found or its own error, never "Checking" for good.
  await step("Tools lists this PC's startup apps", async () => {
    const found = await settled(
      "Startup apps",
      ["start when you sign in.", "Nothing starts when you sign in.", "Some places could not be read"],
      60_000,
    );
    console.log(`\n===== Startup apps =====\n${found.slice(0, 800)}\n`);
  });

  await step("Tools measures the junk files without deleting any", async () => {
    const found = await settled("Clear out junk files", ["Selected: "], 180_000);
    console.log(`\n===== Junk files =====\n${found.slice(0, 1200)}\n`);
    assert.ok(!found.includes("Cleared "), "a cleanup ran");
  });

  await step("Presets list what each would change on this PC (N116)", async () => {
    const presets = By.xpath("//h2[normalize-space()='Presets']/ancestor::section[1]");
    const found = await driver.wait(until.elementLocated(presets), 15_000).then((s) => s.getText());
    console.log(`\n===== Presets =====\n${found.slice(0, 900)}\n`);
    const review = await (await driver.findElement(presets)).findElements(By.xpath(".//button[starts-with(normalize-space(), 'Review ')]"));
    if (review.length === 0) {
      console.log("(nothing left to apply in any preset here)");
      return;
    }
    await review[0].click();
    const dialog = await driver.wait(until.elementLocated(By.css("[role='dialog']")), 10_000);
    console.log(`\n===== Preset review =====\n${(await dialog.getText()).slice(0, 1500)}\n`);
    // Looked at only: Cancel applies nothing.
    await dialog.findElement(By.xpath(".//button[normalize-space()='Cancel']")).click();
    await driver.wait(async () => (await driver.findElements(By.css("[role='dialog']"))).length === 0, 10_000);
  });

  await step("Clean memory empties the standby list on this machine (N115)", async () => {
    const section = await driver.findElement(sectionOf("Clean memory"));
    await driver.wait(async () => !(await section.getText()).includes("Reading memory"), 30_000);
    await section.findElement(By.xpath(".//button[normalize-space()='Clean memory']")).click();
    const found = await settled("Clean memory", ["Cleaned "], 120_000);
    console.log(`\n===== Clean memory =====\n${found.slice(0, 1200)}\n`);
  });

  await step("Gaming Mode turns on and off from the top bar, and the engine keeps it (N114)", async () => {
    const toggle = By.xpath("//header//button[@aria-pressed][.//span[normalize-space()='Gaming Mode']]");
    const before = await ipc("get_settings");
    assert.ok(before.ok, `get_settings failed: ${JSON.stringify(before.error)}`);
    const was = before.value.gamingMode;
    for (const want of [!was, was]) {
      await driver.findElement(toggle).click();
      await driver.wait(async () => {
        const button = await driver.findElement(toggle);
        return (
          (await button.getAttribute("aria-pressed")) === String(want) && (await button.getAttribute("aria-busy")) !== "true"
        );
      }, 15_000);
      const now = await ipc("get_settings");
      assert.ok(now.ok && now.value.gamingMode === want, `the engine has gamingMode ${now.value?.gamingMode}, not ${want}`);
      console.log(`\n(Gaming Mode ${want ? "on" : "off"}: ${await driver.findElement(toggle).getAttribute("title")})`);
    }
  });

  await step("Advanced lists this PC's graphics cards and network adapters", async () => {
    const advanced = By.xpath("//label[starts-with(normalize-space(), 'Advanced')]/input");
    await driver.findElement(advanced).click();
    const found = await settled(
      "Devices",
      ["MSI mode: ", "No graphics card or network adapter here can take this change.", "The devices could not be listed."],
      60_000,
    );
    console.log(`\n===== Devices =====\n${found.slice(0, 1200)}\n`);
    await driver.findElement(advanced).click();
  });

  // The one step here that goes online: ICMP echoes to the router and two
  // public DNS servers, started by a click as the user would (catalogue E4).
  // Azure runners may answer none of them; any reading is fine, an error is not.
  await step("the connection check runs on this machine", async () => {
    const check = By.xpath(".//button[normalize-space()='Check now']");
    await (await driver.findElement(sectionOf("Check the connection"))).findElement(check).click();
    const found = await settled("Check the connection", ["Checked "], 180_000);
    console.log(`\n===== Connection check =====\n${found.slice(0, 1200)}\n`);
  });

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

  await step("Copy summary describes this machine", async () => {
    // WebView2 may or may not grant the clipboard write; either outcome is a
    // pass, and which one is printed. Reading the clipboard back would raise
    // a permission prompt, so the text is checked only when it is shown.
    await driver.findElement(By.xpath("//button[normalize-space()='Copy summary']")).click();
    await driver.wait(
      until.elementLocated(
        By.xpath("//button[normalize-space()='Copied'] | //textarea[@aria-label='Summary of this PC and its changes']"),
      ),
      10_000,
    );
    const boxes = await driver.findElements(By.css("textarea[aria-label='Summary of this PC and its changes']"));
    if (boxes.length) {
      const summary = await boxes[0].getAttribute("value");
      console.log(`\n===== Copy summary (clipboard refused, shown to copy by hand) =====\n${summary}\n`);
      assert.ok(summary.startsWith("PeakTweaks summary, "), "a real PC's summary has no SAMPLE line");
    } else {
      console.log("\n===== Copy summary: copied to the clipboard =====\n");
    }
  });

  await step("a pasted setup is checked against this PC's own list (N106)", async () => {
    // Checked only: nothing is applied here (apply-undo.mjs covers applying).
    const paste = await driver.findElement(By.css("textarea#setup-paste"));
    await paste.sendKeys('{"peaktweaks-setup":1,"changes":["gaming.gamemode","not.a.real.change"]}');
    await driver.findElement(By.xpath("//button[normalize-space()='Check it']")).click();
    const plan = await driver.wait(
      until.elementLocated(By.xpath("//*[@role='status'][contains(normalize-space(), 'Not available on this PC')]")),
      10_000,
    );
    const shown = await plan.getText();
    console.log(`\n===== Pasted setup, as checked here =====\n${shown}\n`);
    assert.ok(shown.includes("Not available on this PC (1): not.a.real.change."), "an id this PC lacks is listed as not here");
    assert.ok(!shown.includes("gaming.gamemode"), "a change on this PC's list is named by its name, not its id");
  });

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
