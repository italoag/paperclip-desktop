#!/usr/bin/env node

/**
 * Bumps every Paperclip pin to a target server version, in one deterministic
 * pass. Called by `.github/workflows/server-version-check.yml` to turn "the
 * server is behind" into a pull request; also runnable by hand:
 *
 *   node scripts/bump-paperclip-server.mjs 2026.1023.0
 *
 * Three things always move together:
 *
 *   1. `devDependencies["@paperclipai/server"]` in the root package.json — the
 *      pin `prepare-server.mjs` installs into the bundle.
 *   2. Every `@paperclipai/*` dependency in `adapters/<name>/package.json` —
 *      bundled adapters share the server's adapter-utils, and a version skew
 *      hands the plugin loader types from a different release.
 *   3. The desktop `version`, patch-bumped — `tauri-plugin-updater` compares
 *      the app version in `latest.json`, not the server version, so without
 *      this no client would ever see the new build.
 *
 * Lockfiles are intentionally NOT touched here: the workflow runs
 * `pnpm install --lockfile-only` afterwards, which regenerates them without
 * downloading the tree or tripping the ignored-builds gate that a full
 * install can hit.
 */

import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function readJson(file) {
  return JSON.parse(readFileSync(file, "utf8"));
}

function writeJson(file, value) {
  writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

export function bumpPatch(version) {
  const match = /^(\d+)\.(\d+)\.(\d+)(.*)$/.exec(version);
  if (!match) throw new Error(`Cannot parse desktop version "${version}"`);
  return `${match[1]}.${match[2]}.${Number(match[3]) + 1}${match[4] ?? ""}`;
}

/**
 * Applies the bump under `rootDir` (defaults to the repo root).
 * Returns { previousServer, server, previousDesktop, desktop, changed } —
 * `changed` lists every file edit, `server` is the version now pinned.
 */
export function bumpPaperclipServer(target, rootDir = projectRoot) {
  const changed = [];

  const rootPkgPath = path.join(rootDir, "package.json");
  const rootPkg = readJson(rootPkgPath);

  const previousServer = rootPkg.devDependencies?.["@paperclipai/server"];
  if (!previousServer) {
    throw new Error('devDependencies["@paperclipai/server"] is missing in package.json');
  }

  const previousDesktop = rootPkg.version;
  if (previousServer === target) {
    return { previousServer, server: target, previousDesktop, desktop: previousDesktop, changed };
  }

  rootPkg.devDependencies["@paperclipai/server"] = target;
  rootPkg.version = bumpPatch(previousDesktop);
  writeJson(rootPkgPath, rootPkg);
  changed.push(
    `package.json: @paperclipai/server ${previousServer} -> ${target}, version ${previousDesktop} -> ${rootPkg.version}`,
  );

  const adaptersDir = path.join(rootDir, "adapters");
  for (const entry of readdirSync(adaptersDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const pkgPath = path.join(adaptersDir, entry.name, "package.json");
    if (!existsSync(pkgPath)) continue;

    const pkg = readJson(pkgPath);
    const touched = [];
    for (const section of ["dependencies", "devDependencies"]) {
      const deps = pkg[section];
      if (!deps) continue;
      for (const [name, version] of Object.entries(deps)) {
        if (!name.startsWith("@paperclipai/") || version === target) continue;
        deps[name] = target;
        touched.push(`${name} ${version} -> ${target}`);
      }
    }

    if (touched.length > 0) {
      writeJson(pkgPath, pkg);
      changed.push(`${entry.name}/package.json: ${touched.join(", ")}`);
    }
  }

  return { previousServer, server: target, previousDesktop, desktop: rootPkg.version, changed };
}

function isMain() {
  return process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);
}

if (isMain()) {
  const target = process.argv[2];
  if (!target || !/^\d+\.\d+\.\d+(-[\w.]+)?$/.test(target)) {
    console.error(`Usage: node scripts/bump-paperclip-server.mjs <server-version>
Example: node scripts/bump-paperclip-server.mjs 2026.1023.0`);
    process.exit(1);
  }

  const result = bumpPaperclipServer(target);
  if (result.changed.length === 0) {
    console.log(`[bump] @paperclipai/server is already ${target}; nothing to do.`);
  }
  for (const line of result.changed) console.log(`[bump] ${line}`);
  console.log(
    JSON.stringify({
      previousServer: result.previousServer,
      server: result.server,
      previousDesktop: result.previousDesktop,
      desktop: result.desktop,
    }),
  );
}
