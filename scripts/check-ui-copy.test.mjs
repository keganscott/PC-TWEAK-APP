// Tests for the UI copy lint. Run with: node --test scripts/
import { test } from "node:test";
import assert from "node:assert/strict";

import { findClaim, lintSource, loadClaimWords } from "./check-ui-copy.mjs";

const words = loadClaimWords();
const lint = (src, file = "x.tsx") => lintSource(file, src, words).map((h) => h.word);

test("the shared list loads", () => {
  assert.ok(words.length >= 15);
  assert.ok(words.includes("fps"));
});

test("words match at word starts; symbols match anywhere (same rule as Rust)", () => {
  assert.equal(findClaim("No more lag", words), "lag");
  assert.equal(findClaim("Laggy games", words), "lag");
  assert.equal(findClaim("Feature flag", words), null);
  assert.equal(findClaim("It improves things", words), "improv");
  assert.equal(findClaim("Up to 30% more", words), "% ");
  assert.equal(findClaim("Turns off pointer acceleration", words), null);
});

test("a Windows feature's own name is not a claim, the same word elsewhere is", () => {
  assert.equal(findClaim("\\Microsoft\\Windows\\Customer Experience Improvement Program\\UsbCeip", words), null);
  assert.equal(findClaim("The Customer Experience Improvement Program improves games", words), "improv");
});

test("JSX text, attributes shown to users, and plain strings are copy", () => {
  assert.deepEqual(lint(`export const A = () => <p>Boost your games</p>;`), ["boost"]);
  assert.deepEqual(lint(`export const A = () => <button aria-label="Get more FPS" />;`), ["fps"]);
  assert.deepEqual(lint(`const t = "Makes it faster";`, "x.ts"), ["faster"]);
  assert.deepEqual(lint("const t = `Gains ${n}% more`;", "x.ts"), ["% "]);
});

test("code that is never shown is not copy", () => {
  assert.deepEqual(
    lint(
      [
        `import fps from "./fps-boost";`,
        `// boost in a comment`,
        `type Mode = "boost" | "fps";`,
        `const m = { "fps": 1 };`,
        `const v = m["fps"];`,
        `export const A = () => <div className="boost-panel" data-testid="fps" />;`,
      ].join("\n"),
    ),
    [],
  );
});

test("an allow comment needs a reason and covers its own or the next line", () => {
  assert.deepEqual(
    lint(
      [
        `// copy-lint-allow: shows the stored verdict for runId`,
        `const a = "measured on this PC";`,
        `const b = "measured on this PC"; // copy-lint-allow: bound to runId`,
      ].join("\n"),
      "x.ts",
    ),
    [],
  );
  assert.deepEqual(lint(`// copy-lint-allow:\nconst a = "measured";`, "x.ts"), ["measured"]);
});

test("hits report the line of the offending text", () => {
  const hits = lintSource("x.ts", `const a = 1;\nconst b = "boost";`, words);
  assert.equal(hits[0].line, 2);
});
