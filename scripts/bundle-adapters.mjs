#!/usr/bin/env node

/**
 * Stages the in-repo adapter packages into the server bundle so a fresh install
 * ships with them.
 *
 * They are copied into `server/node_modules/<name>/` on purpose rather than a
 * directory of their own: an adapter's dependencies (`@paperclipai/adapter-utils`,
 * `@paperclipai/adapter-acpx-local`, `acpx`) are already in that flat tree
 * because the server itself depends on them, so Node resolves them from the
 * sibling entries. Any other location would mean shipping a second copy of
 * those packages, and a second copy of adapter-utils would hand the loader
 * types from a different version than the server runs.
 *
 * Only build output is copied — `dist/`, the manifest and the README. Source
 * and tests stay out of the bundle.
 *
 * Usage: node scripts/bundle-adapters.mjs [--arch arm64|x64]
 */

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

import { patchBundle } from "./patch-builtin-adapter-allowlist.mjs";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");
const adaptersRoot = path.join(projectRoot, "adapters");

/** Files/directories copied into the bundle, when present. */
const SHIPPED = ["dist", "package.json", "README.md", "LICENSE"];

function parseArches() {
  const args = process.argv.slice(2);
  const index = args.indexOf("--arch");
  if (index === -1) return ["arm64", "x64"];
  const arch = args[index + 1];
  if (arch !== "arm64" && arch !== "x64") {
    throw new Error(`Unknown --arch "${arch}". Expected arm64 or x64.`);
  }
  return [arch];
}

function listAdapters() {
  if (!existsSync(adaptersRoot)) return [];
  return readdirSync(adaptersRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .filter((entry) => existsSync(path.join(adaptersRoot, entry.name, "package.json")))
    .map((entry) => path.join(adaptersRoot, entry.name));
}

/**
 * Reads the adapter's `type` by loading its built entry point.
 *
 * Imported rather than pattern-matched out of the source: the type key is what
 * the store record is looked up by, so a value that drifts from what
 * `createServerAdapter()` actually returns would register a ghost adapter.
 */
function adapterType(adapterDir) {
  const entry = path.join(adapterDir, "dist", "index.js");
  const type = execFileSync(
    "node",
    ["--input-type=module", "-e", `const m = await import("${entry}"); process.stdout.write(m.type ?? "");`],
    { encoding: "utf8" },
  ).trim();

  if (!type) {
    throw new Error(`[bundle-adapters] ${path.basename(adapterDir)} exports no "type"`);
  }
  return type;
}

function build(adapterDir) {
  const name = path.basename(adapterDir);
  console.log(`[bundle-adapters] Building ${name}...`);

  // A stale dist would silently ship yesterday's adapter.
  execFileSync("pnpm", ["install", "--frozen-lockfile"], {
    cwd: adapterDir,
    stdio: "inherit",
  });
  execFileSync("pnpm", ["build"], { cwd: adapterDir, stdio: "inherit" });

  const distDir = path.join(adapterDir, "dist");
  if (!existsSync(path.join(distDir, "index.js"))) {
    throw new Error(`[bundle-adapters] ${name} produced no dist/index.js`);
  }
}

function stage(adapterDir, variant) {
  const pkgName = JSON.parse(
    execFileSync("node", ["-p", `JSON.stringify(require("${path.join(adapterDir, "package.json")}").name)`], {
      encoding: "utf8",
    }),
  );

  const target = path.join(
    projectRoot,
    "build",
    "server-bundle",
    variant,
    "server",
    "node_modules",
    ...pkgName.split("/"),
  );

  rmSync(target, { recursive: true, force: true });
  mkdirSync(target, { recursive: true });

  for (const entry of SHIPPED) {
    const from = path.join(adapterDir, entry);
    if (existsSync(from)) {
      cpSync(from, path.join(target, entry), { recursive: true });
    }
  }

  // Fails loudly here rather than as a confusing module-resolution error at
  // runtime inside the packaged app.
  for (const dependency of Object.keys(
    JSON.parse(
      execFileSync(
        "node",
        ["-p", `JSON.stringify(require("${path.join(adapterDir, "package.json")}").dependencies ?? {})`],
        { encoding: "utf8" },
      ),
    ),
  )) {
    const resolved = path.join(
      projectRoot,
      "build",
      "server-bundle",
      variant,
      "server",
      "node_modules",
      ...dependency.split("/"),
    );
    if (!existsSync(resolved)) {
      throw new Error(
        `[bundle-adapters] ${pkgName} depends on "${dependency}", which is not in the ` +
          `${variant} server bundle. It must be a dependency of @paperclipai/server, ` +
          "otherwise it has to be vendored into the adapter itself.",
      );
    }
  }

  console.log(`[bundle-adapters]   staged ${pkgName} into ${variant}`);
  return pkgName;
}

function main() {
  const adapters = listAdapters();
  if (adapters.length === 0) {
    console.log("[bundle-adapters] No adapters under adapters/, nothing to do.");
    return;
  }

  for (const adapterDir of adapters) build(adapterDir);

  for (const arch of parseArches()) {
    const variant = `mac-${arch}`;
    const bundleDir = path.join(projectRoot, "build", "server-bundle", variant);
    if (!existsSync(bundleDir)) {
      throw new Error(
        `[bundle-adapters] ${bundleDir} does not exist. Run \`pnpm prepare-server\` first.`,
      );
    }
    const staged = adapters.map((adapterDir) => ({
      packageName: stage(adapterDir, variant),
      type: adapterType(adapterDir),
    }));

    // Manifest for the Rust shell, which registers these in the adapter plugin
    // store on launch. Emitting it here keeps the shell from having to guess
    // which of the hundreds of packages in node_modules are adapters.
    writeFileSync(
      path.join(bundleDir, "server", "paperclip-bundled-adapters.json"),
      `${JSON.stringify(staged, null, 2)}\n`,
    );

    // Paperclip's built-in agents (Reflection Coach, Summarizer) carry a
    // hardcoded list of adapters the UI will let you pick. Without this the
    // bundled adapters are greyed out for exactly the roles a user is most
    // likely to reassign.
    const { patched, found } = patchBundle(
      path.join(bundleDir, "server"),
      staged.map((entry) => entry.type),
    );
    console.log(
      `[bundle-adapters]   built-in allowlists: ${patched}/${found} updated in ${variant}`,
    );
  }

  console.log("[bundle-adapters] Done.");
}

main();
