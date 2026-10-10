import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

// The UI against the SAMPLE mock: every screen loads, the restore lock gates
// changes, a change can be applied and undone, and no screen has serious
// accessibility problems (plan section 7: WCAG 2.2 AA target).

async function open(page: Page) {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
}

async function nav(page: Page, name: string) {
  await page.getByRole("navigation", { name: "Main" }).getByRole("button", { name }).click();
  await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
}

async function expectNoSeriousA11yIssues(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"]).analyze();
  const serious = results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  expect(serious.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
}

test("the SAMPLE label is always visible outside the app", async ({ page }) => {
  await open(page);
  // Exact: the printed scan's own SAMPLE line also contains these words, hidden
  // on screen, and appears once the scan has loaded.
  await expect(page.getByText("Demo data, not this PC", { exact: true })).toBeVisible();
});

test("every screen loads and passes an accessibility scan", async ({ page }) => {
  await open(page);
  await expect(page.getByRole("heading", { name: "What the scan found" })).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  for (const name of ["Games", "Tools", "Proof", "Backups"]) {
    await nav(page, name);
    await expectNoSeriousA11yIssues(page);
  }
  // Tools with Advanced on adds the Advanced changes and the Devices section.
  await nav(page, "Tools");
  await page.getByRole("switch", { name: /Advanced/ }).check();
  await expect(page.getByText("MSI mode: Sample graphics card")).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  // A connection check's results: the reading and the table of targets.
  await page.getByRole("button", { name: "Check now" }).click();
  await expect(page.getByRole("rowheader", { name: /Your router/ })).toBeVisible();
  await expectNoSeriousA11yIssues(page);
});

test("only the content area scrolls, never the whole window", async ({ page }) => {
  // The page itself scrolling hides the top bar and leaves black space
  // under the app (seen in a maximised window on Tools).
  const pageScrolls = () =>
    page.evaluate(() => {
      const root = document.scrollingElement!;
      return root.scrollHeight > root.clientHeight || root.scrollWidth > root.clientWidth;
    });
  await open(page);
  expect(await pageScrolls()).toBe(false);
  for (const name of ["Games", "Tools", "Proof", "Backups"]) {
    await nav(page, name);
    expect(await pageScrolls(), name).toBe(false);
  }
  await nav(page, "Tools");
  await page.getByRole("switch", { name: /Advanced/ }).check();
  await expect(page.getByText("MSI mode: Sample graphics card")).toBeVisible();
  expect(await pageScrolls(), "Tools with Advanced on").toBe(false);
});

test("changes stay locked until a restore point exists, then apply and undo", async ({ page }) => {
  await open(page);
  await nav(page, "Tools");
  await expect(page.getByText("Changes are locked until there is a restore point.")).toBeVisible();
  const card = page.getByRole("listitem").filter({ hasText: "Sample setting A" });
  await expect(card.getByRole("button", { name: "Apply" })).toBeDisabled();

  // One click from the lock message itself, without leaving Tools (the first
  // such button; the driver tool further down has one too).
  await page.getByRole("main").getByRole("button", { name: "Make a restore point" }).first().click();
  await expect(page.getByText("Restore point ready")).toBeVisible();
  await expect(page.getByText("Changes are locked until there is a restore point.")).toBeHidden();
  await card.getByRole("button", { name: "Apply" }).click();
  await expect(card.getByText("Optimized", { exact: true })).toBeVisible();

  await nav(page, "Backups");
  await page.getByRole("button", { name: "Undo all" }).click();
  const dialog = page.getByRole("dialog", { name: "Undo every change?" });
  await expect(dialog).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  await dialog.getByRole("button", { name: /^Undo \d+ change/ }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("Nothing PeakTweaks changed is in effect.")).toBeVisible();
});

test("the confirmation dialog traps focus and closes on Escape", async ({ page }) => {
  await open(page);
  await nav(page, "Backups");
  const undoAll = page.getByRole("button", { name: "Undo all" });
  await undoAll.click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  for (let i = 0; i < 6; i += 1) {
    await page.keyboard.press("Tab");
    expect(await dialog.evaluate((d) => d.contains(document.activeElement))).toBe(true);
  }
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(undoAll).toBeFocused();
});

test("a proof comparison shows the engine's headline as given", async ({ page }) => {
  await open(page);
  await nav(page, "Proof");
  await page.getByRole("navigation", { name: "Comparisons" }).getByRole("button").first().click();
  await expect(page.getByRole("heading", { name: "Steps" })).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  await page.getByRole("button", { name: "Compare" }).click();
  const headline = page.getByTestId("verdict-headline");
  await expect(headline).toContainText(/^(Better|Worse|No measurable change|Not enough data)/);
  // The copied text carries the headline word for word and the run ids.
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.getByRole("button", { name: "Copy result" }).click();
  await expect(page.getByRole("button", { name: "Copied" })).toBeVisible();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain(await headline.innerText());
  expect(copied).toMatch(/^SAMPLE DATA/);
  expect(copied).toMatch(/Runs before \(\d+\): \S/);
});

test("settings switch to technical wording and show registry targets", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Settings" }).click();
  const dialog = page.getByRole("dialog", { name: "Settings" });
  await dialog.getByLabel(/Technical/).check();
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(dialog).toBeHidden();
  await nav(page, "Tools");
  await expect(page.getByText(/HKEY_LOCAL_MACHINE\\SOFTWARE\\PeakTweaks\\Sample/).first()).toBeVisible();
});

test("a printed scan has no app chrome, keeps every finding and says it is SAMPLE data", async ({ page }) => {
  await open(page);
  await expect(page.getByRole("heading", { name: /^You can fix/ })).toBeVisible();
  await page.emulateMedia({ media: "print" });
  await expect(page.getByRole("navigation", { name: "Main" })).toBeHidden();
  await expect(page.getByRole("button", { name: "Print this scan" })).toBeHidden();
  await expect(page.getByRole("heading", { name: /^You can fix/ })).toBeVisible();
  await expect(page.getByText("SAMPLE: demo data, not this PC.")).toBeVisible();
  // Light on paper: the page background is white, not the dark theme.
  const bg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  expect(bg).toBe("rgb(255, 255, 255)");
});

test("the welcome walks through its three steps and passes an accessibility scan", async ({ page }) => {
  await open(page);
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Show the welcome again" }).click();
  const dialog = page.getByRole("dialog", { name: /Welcome to PeakTweaks/ });
  await expect(dialog).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  for (const step of ["Your safety net", "Your scan"]) {
    await dialog.getByRole("button", { name: "Next" }).click();
    await expect(page.getByRole("dialog", { name: new RegExp(step) })).toBeVisible();
  }
  await page.getByRole("button", { name: "Get started" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
});

test("a setup pasted from another PC is checked against this list, then applied", async ({ page }) => {
  await open(page);
  await nav(page, "Backups");
  const card = page.getByRole("region", { name: "Copy this setup to another PC" });
  await card.getByLabel("Paste a setup from another PC").fill("not a setup");
  await card.getByRole("button", { name: "Check it" }).click();
  await expect(card.getByText("That is not a setup copied from PeakTweaks.")).toBeVisible();

  await card
    .getByLabel("Paste a setup from another PC")
    .fill('{"peaktweaks-setup":1,"changes":["fixture.default","fixture.applied","fixture.blocked","elsewhere.only"]}');
  await card.getByRole("button", { name: "Check it" }).click();
  await expect(card.getByText("Can be applied here (1):")).toBeVisible();
  await expect(card.getByText("Already in place here (1): Sample setting B.")).toBeVisible();
  await expect(card.getByText("Not available on this PC (2): Sample setting D, elsewhere.only.")).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  // Locked until a restore point exists, like every other change.
  await expect(card.getByRole("button", { name: "Apply 1 change" })).toBeDisabled();
  await page.getByRole("button", { name: "Home" }).first().click();
  await page.getByRole("main").getByRole("button", { name: "Make a restore point" }).first().click();
  await expect(page.getByText("Restore point ready")).toBeVisible();
  await nav(page, "Backups");
  await card.getByLabel("Paste a setup from another PC").fill('{"peaktweaks-setup":1,"changes":["fixture.default"]}');
  await card.getByRole("button", { name: "Check it" }).click();
  await card.getByRole("button", { name: "Apply 1 change" }).click();
  await expect(card.getByText("Already in place here (1): Sample setting A.")).toBeVisible();
  await expect(card.getByText("Nothing in it is left to apply here.")).toBeVisible();
});

test("a reminder put off with Not now leaves Home", async ({ page }) => {
  await open(page);
  const reminders = page.getByRole("region", { name: "Reminders" });
  const driver = reminders.getByRole("note").filter({ hasText: "The Example GPU driver is dated" });
  await expect(driver).toBeVisible();
  await driver.getByRole("button", { name: "Not now" }).click();
  await expect(driver).toBeHidden();
});
