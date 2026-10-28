#!/usr/bin/env node

/**
 * Regenerates src/models.ts from `devin models list`.
 *
 * The catalogue is baked into the package rather than fetched at runtime so the
 * config form renders instantly and works when Devin is installed on a
 * different host than the board UI. Re-run this when Devin ships new models.
 */

import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const target = path.resolve(__dirname, "..", "src", "models.ts");

const command = process.argv[2] ?? "devin";

const raw = execFileSync(command, ["models", "list"], { encoding: "utf8" });
const cliVersion = execFileSync(command, ["version"], { encoding: "utf8" }).trim();

// "Claude Opus 5 (claude-opus-5)" starts a family;
// "  claude-opus-5-low   Claude Opus 5 Low  [1M context, ...]" is a model.
const FAMILY = /^(\S.*?) \(([a-z0-9._-]+)\)$/;
const MODEL = /^ {2}([a-z0-9._-]+)\s{2,}(.+?)(?:\s*\[(.+)\])?$/;

const models = [];
let family = null;

for (const line of raw.split("\n")) {
  const trimmedEnd = line.replace(/\s+$/, "");

  const familyMatch = FAMILY.exec(trimmedEnd);
  if (familyMatch) {
    family = familyMatch[1];
    continue;
  }

  if (trimmedEnd.trim().startsWith("aliases:")) continue;

  const modelMatch = MODEL.exec(trimmedEnd);
  if (modelMatch && family) {
    models.push({ id: modelMatch[1], label: modelMatch[2].trim(), family });
  }
}

if (models.length === 0) {
  throw new Error(`Parsed no models from \`${command} models list\` — has the output format changed?`);
}

const escape = (value) => value.replaceAll("\\", "\\\\").replaceAll('"', '\\"');

const contents = `// GENERATED from \`devin models list\` — regenerate with scripts/refresh-models.mjs
// Devin CLI ${cliVersion}

export interface DevinModel {
  id: string;
  label: string;
  /** Model family, used to group the dropdown. */
  family: string;
}

export const DEVIN_MODELS: DevinModel[] = [
${models
  .map((m) => `  { id: "${escape(m.id)}", label: "${escape(m.label)}", family: "${escape(m.family)}" },`)
  .join("\n")}
];
`;

writeFileSync(target, contents);
console.log(`[refresh-models] wrote ${models.length} models to ${target}`);
