#!/usr/bin/env node
/**
 * Size tuning for the release build, without touching anything else.
 *
 * Node rather than Python on purpose: `npm ci` already guarantees Node, while a
 * fresh Windows box (and the GitHub runner image) has no `python3` on PATH — a
 * build script that depends on an interpreter nobody installed is a build
 * script that breaks on someone else's machine.
 *
 * Two levers, applied *in place* inside the tables that already exist (this
 * script never inserts or deletes a table, so it cannot break the manifest):
 *
 *   [profile.release]
 *       strip = true        already upstream; ensured
 *       panic = "abort"     drops unwinding tables (~3-6 MB on a binary this size)
 *   [profile.release.package."*"]
 *       opt-level = 3       dependencies (image decode, Tantivy, PDF, vectors)
 *                           keep full speed: size tuning must not slow search
 *
 * `opt-level = "z"` is deliberately not used: it shaves ~2-3% more and costs
 * 10-20% on the indexer and the vector scan.
 *
 *   node tools/compact-profile.mjs          # size tuning only
 *   node tools/compact-profile.mjs --slim   # also default = [] (drops PP-OCR: -10 MB)
 *   node tools/compact-profile.mjs --full   # restore default = ["accurate-ocr"]
 *
 * Idempotent: running it twice changes nothing.
 */
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const manifestPath = join(here, "..", "src-tauri", "Cargo.toml");
const args = new Set(process.argv.slice(2));

if (!existsSync(manifestPath)) {
  console.error(`not found: ${manifestPath}`);
  process.exit(1);
}

const original = readFileSync(manifestPath, "utf8");
const lines = original.split("\n");

/** Renders a TOML value: strings get quotes, everything else is written raw. */
const render = (value) =>
  typeof value === "string" && !value.startsWith("__raw__")
    ? `"${value}"`
    : value.replace(/^__raw__/, "");

const raw = (text) => `__raw__${text}`;

/** Index range of a table body: [header, end). */
function tableBounds(header) {
  const start = lines.findIndex((line) => line.trim() === header);
  if (start === -1) return null;
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i += 1) {
    if (lines[i].trimStart().startsWith("[")) {
      end = i;
      break;
    }
  }
  return { start, end };
}

/** Sets `key = value` inside `header`. Returns true when the file changed. */
function setKey(header, key, value) {
  const bounds = tableBounds(header);
  if (!bounds) return false;
  const wanted = `${key} = ${render(value)}`;

  for (let i = bounds.start + 1; i < bounds.end; i += 1) {
    if (lines[i].trim().startsWith(`${key} =`) || lines[i].trim().startsWith(`${key}=`)) {
      if (lines[i] === wanted) return false;
      lines[i] = wanted;
      return true;
    }
  }

  // Append before the blank line that closes the table, if there is one.
  let insertAt = bounds.end;
  while (insertAt - 1 > bounds.start && lines[insertAt - 1].trim() === "") insertAt -= 1;
  lines.splice(insertAt, 0, wanted);
  return true;
}

/** Ensures [profile.release.package."*"] exists with opt-level = 3. */
function ensureDependencyProfile() {
  const header = '[profile.release.package."*"]';
  if (tableBounds(header)) return setKey(header, "opt-level", raw("3"));

  const release = tableBounds("[profile.release]");
  if (!release) return false;
  lines.splice(
    release.end,
    0,
    "",
    "# tuned by tools/compact-profile.mjs: dependencies stay at full speed.",
    header,
    'opt-level = 3',
  );
  return true;
}

let changed = false;
changed = setKey("[profile.release]", "strip", raw("true")) || changed;
changed = setKey("[profile.release]", "panic", "abort") || changed;
changed = ensureDependencyProfile() || changed;

if (args.has("--full")) {
  changed = setKey("[features]", "default", raw('["accurate-ocr"]')) || changed;
}
if (args.has("--slim")) {
  changed = setKey("[features]", "default", raw("[]")) || changed;
}

const updated = lines.join("\n");
if (updated === original) {
  console.log("release profile already tuned; nothing to do");
  process.exit(0);
}

writeFileSync(manifestPath, updated);
console.log(`patched ${relative(join(here, ".."), manifestPath)}`);
if (args.has("--slim")) console.log("slim: features.default = [] (PP-OCR/ONNX left out, ~10 MB smaller)");
if (args.has("--full")) console.log('full: features.default = ["accurate-ocr"]');
