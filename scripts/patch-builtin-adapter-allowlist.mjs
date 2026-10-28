#!/usr/bin/env node
/**
 * Lets the bundled adapters run Paperclip's built-in agents.
 *
 * # The problem
 *
 * Each built-in agent definition in `services/built-in-agents.js` carries an
 * `allowedAdapterTypes` array naming the adapters it may use. The server never
 * enforces it — a `PATCH /api/agents/:id` switching Reflection Coach to
 * `devin_local` is accepted — but the UI reads the list off the definition and
 * disables every option outside it:
 *
 * ```js
 * const allowed = new Set(definition.allowedAdapterTypes ?? []);
 * disabled = options.filter((o) => allowed.size > 0 && !allowed.has(o.value));
 * ```
 *
 * So Reflection Coach and Summarizer are greyed out for our adapters, and the
 * only way to assign one is through the API — which is not something a user
 * should have to do.
 *
 * # Why patch here
 *
 * The list is hardcoded upstream with no extension point: no environment
 * variable, no config file, no registry hook. Since this repo is what stages
 * `@paperclipai/server` into the app, appending to the array at package time is
 * the one place the fix can live without forking Paperclip.
 *
 * The patch is deliberately narrow — it appends adapter types to arrays that
 * already exist and never removes an entry, so a built-in keeps every adapter
 * upstream intended. Emptying the arrays would also work (the UI skips the
 * filter when the list is empty) but would silently allow adapters upstream
 * excluded on purpose, including ones that cannot serve these roles.
 *
 * # On upgrades
 *
 * This edits vendored code, so it fails loudly when the shape changes rather
 * than leaving a silently unpatched bundle: a server upgrade that renames the
 * field or restructures the definitions stops the build instead of shipping an
 * app where the built-ins quietly refuse our adapters again.
 */

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const TARGET = "dist/services/built-in-agents.js";
const FIELD = "allowedAdapterTypes";

/** Matches `allowedAdapterTypes: [ ... ]` with a flat list of string literals. */
const ALLOWLIST = new RegExp(`${FIELD}:\\s*\\[([^\\]]*)\\]`, "g");

export function patchAllowlists(source, adapterTypes) {
  if (adapterTypes.length === 0) return { source, patched: 0, found: 0 };

  let found = 0;
  let patched = 0;

  const next = source.replace(ALLOWLIST, (match, body) => {
    found += 1;
    const existing = [...body.matchAll(/"([^"]+)"/g)].map((entry) => entry[1]);
    const missing = adapterTypes.filter((type) => !existing.includes(type));
    if (missing.length === 0) return match;

    patched += 1;
    const merged = [...existing, ...missing].map((type) => `"${type}"`).join(", ");
    return `${FIELD}: [${merged}]`;
  });

  return { source: next, patched, found };
}

export function patchBundle(serverDir, adapterTypes) {
  const file = path.join(serverDir, TARGET);

  let source;
  try {
    source = readFileSync(file, "utf8");
  } catch (error) {
    throw new Error(
      `[patch-builtin-allowlist] cannot read ${TARGET}: ${error.message}\n` +
        "The bundled server no longer has this file. Check whether upstream moved " +
        "the built-in agent definitions before removing this patch.",
    );
  }

  const result = patchAllowlists(source, adapterTypes);

  if (result.found === 0) {
    throw new Error(
      `[patch-builtin-allowlist] no "${FIELD}" arrays found in ${TARGET}.\n` +
        "Upstream changed how built-in agents restrict adapters. Re-check the UI " +
        "gate before assuming the restriction is gone — until then the built-in " +
        "agents may silently refuse the bundled adapters again.",
    );
  }

  if (result.patched > 0) writeFileSync(file, result.source);
  return result;
}

function main() {
  const args = process.argv.slice(2);
  const serverDir = args[0];
  const adapterTypes = args.slice(1);

  if (!serverDir || adapterTypes.length === 0) {
    console.error(
      "usage: patch-builtin-adapter-allowlist.mjs <server-dir> <adapter-type>...",
    );
    process.exit(1);
  }

  const { patched, found } = patchBundle(serverDir, adapterTypes);
  console.log(
    `[patch-builtin-allowlist] ${patched}/${found} built-in allowlists updated ` +
      `with ${adapterTypes.join(", ")}`,
  );
}

if (import.meta.url === `file://${process.argv[1]}`) main();
