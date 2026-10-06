#!/usr/bin/env node
// UI copy lint (NOTES.md N38; plan section 0.4): no user-facing text may promise
// a result. Only a stored proof run may say a change helped, and the engine
// words that verdict itself.
//
// Uses the TypeScript parser, so it sees exactly the text a user can see:
// string literals, template-literal text and JSX text. Comments, import paths,
// type-level strings and object keys are not copy and are skipped.
//
// The words come from scripts/claim-words.json, the same list the Rust copy
// lints use. A line may opt out only with a reason, for text that is bound to a
// proof run id:
//     // copy-lint-allow: shows the stored verdict for runId
//
// Usage: node scripts/check-ui-copy.mjs [dir ...]   (default: src)

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");

/** Lower-case claim words from the shared list. */
export function loadClaimWords(path = join(here, "claim-words.json")) {
  const parsed = JSON.parse(readFileSync(path, "utf8"));
  if (!Array.isArray(parsed.words) || parsed.words.length === 0) {
    throw new Error(`${path} has no "words" array`);
  }
  return parsed.words.map((w) => String(w).toLowerCase());
}

const isWordChar = (c) => /[a-z0-9]/i.test(c);

/** The first claim word in `text`, or null. Same rule as the Rust `find_claim`. */
export function findClaim(text, words) {
  const lower = text.toLowerCase();
  for (const word of words) {
    const wordLike = isWordChar(word[0] ?? "");
    let from = 0;
    for (;;) {
      const i = lower.indexOf(word, from);
      if (i < 0) break;
      if (!wordLike || i === 0 || !isWordChar(lower[i - 1])) return word;
      from = i + 1;
    }
  }
  return null;
}

const ALLOW = /copy-lint-allow:\s*\S/;

function isNotCopy(node) {
  const p = node.parent;
  if (!p) return false;
  // import x from "..."; export ... from "..."; import("...")
  if (ts.isImportDeclaration(p) || ts.isExportDeclaration(p) || ts.isExternalModuleReference(p)) return true;
  if (ts.isCallExpression(p) && p.expression.kind === ts.SyntaxKind.ImportKeyword) return true;
  // type T = "a" | "b"
  if (ts.isLiteralTypeNode(p)) return true;
  // { "key": value } and obj["key"]
  if (ts.isPropertyAssignment(p) && p.name === node) return true;
  if (ts.isElementAccessExpression(p) && p.argumentExpression === node) return true;
  // className="..." and similar attributes that are never shown
  if (ts.isJsxAttribute(p) && ["className", "id", "key", "data-testid", "href", "src", "type", "role"].includes(p.name.getText())) {
    return true;
  }
  return false;
}

/**
 * Every user-visible piece of text in one source file that contains a claim
 * word and is not allowed by a `copy-lint-allow` comment.
 * @returns {{ line: number, word: string, text: string }[]}
 */
export function lintSource(fileName, source, words) {
  const kind = fileName.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const sf = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, kind);
  const lines = source.split(/\r?\n/);
  const allowed = (line) => ALLOW.test(lines[line] ?? "") || ALLOW.test(lines[line - 1] ?? "");
  const hits = [];

  const check = (node, text) => {
    const word = findClaim(text, words);
    if (!word) return;
    const line = sf.getLineAndCharacterOfPosition(node.getStart(sf)).line;
    if (!allowed(line)) hits.push({ line: line + 1, word, text: text.trim() });
  };

  const visit = (node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      if (!isNotCopy(node)) check(node, node.text);
    } else if (ts.isTemplateExpression(node)) {
      check(node, [node.head.text, ...node.templateSpans.map((s) => s.literal.text)].join(" "));
    } else if (ts.isJsxText(node)) {
      if (node.text.trim()) check(node, node.text);
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return hits;
}

function* sourceFiles(dir) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) {
      // Generated from Rust types; the engine lints its own copy.
      if (name === "generated" || name === "node_modules") continue;
      yield* sourceFiles(path);
    } else if (/\.(ts|tsx)$/.test(name) && !/\.(test|spec)\.(ts|tsx)$/.test(name) && !name.endsWith(".d.ts")) {
      yield path;
    }
  }
}

function main(argv) {
  const dirs = argv.length ? argv : [join(root, "src")];
  const words = loadClaimWords();
  let files = 0;
  const problems = [];
  for (const dir of dirs) {
    for (const file of sourceFiles(resolve(dir))) {
      files += 1;
      for (const hit of lintSource(file, readFileSync(file, "utf8"), words)) {
        problems.push(`${relative(root, file)}:${hit.line}: "${hit.word}" in ${JSON.stringify(hit.text)}`);
      }
    }
  }
  if (problems.length) {
    console.error("UI copy promises a result. Reword it, or bind it to a proof run and add");
    console.error("a `// copy-lint-allow: <reason>` comment on or above the line:\n");
    for (const p of problems) console.error(`  ${p}`);
    return 1;
  }
  console.log(`OK: no claim words in user-facing text of ${files} file(s).`);
  return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exit(main(process.argv.slice(2)));
}
