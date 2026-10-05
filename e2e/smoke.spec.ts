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
  await expect(page.getByText("Demo data, not this PC")).toBeVisible();
});

test("every screen loads and passes an accessibility scan", async ({ page }) => {
  await open(page);
  await expect(page.getByRole("heading", { name: "What the scan found" })).toBeVisible();
  await expectNoSeriousA11yIssues(page);
  for (const name of ["Games", "Tools", "Proof", "Backups"]) {
    await nav(page, name);
    await expectNoSeriousA11yIssues(page);
  }
});

test("changes stay locked until a restore point exists, then apply and undo", async ({ page }) => {
  await open(page);
  await nav(page, "Tools");
  await expect(page.getByText("Changes are locked until there is a restore point.")).toBeVisible();
  const card = page.getByRole("listitem").filter({ hasText: "Sample setting A" });
  await expect(card.getByRole("button", { name: "Apply" })).toBeDisabled();

  await page.getByRole("button", { name: "Make one on Home" }).click();
  await page.getByRole("button", { name: "Make a restore point" }).click();
  await expect(page.getByText(/Restore point #\d+ is ready\./)).toBeVisible();
  await expect(page.getByText("Restore point ready")).toBeVisible();

  await nav(page, "Tools");
  await card.getByRole("button", { name: "Apply" }).click();
  await expect(card.getByText("Applied", { exact: true })).toBeVisible();

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
  await page.getByRole("button", { name: "Compare" }).click();
  await expect(page.getByTestId("verdict-headline")).toContainText(/^(Better|Worse|No measurable change|Not enough data)/);
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
