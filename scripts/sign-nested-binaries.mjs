#!/usr/bin/env node

/**
 * Code-signs every Mach-O binary inside the staged server bundle, before Tauri
 * packages it.
 *
 * # Why this exists
 *
 * `app-server/node_modules` contains thousands of native artefacts — the
 * embedded PostgreSQL distribution, `.node` addons, `esbuild` — and every one of
 * them must carry a signature or notarization fails. Tauri's bundler only signs
 * frameworks, `externalBin` sidecars and the `.app` itself; it never walks
 * `Contents/Resources`.
 *
 * A phase-0 spike confirmed that a signature applied *here*, in the staging
 * directory, survives being copied into the bundle: the signature lives inside
 * the Mach-O file (`LC_CODE_SIGNATURE`), not in an extended attribute. Tauri
 * then signs the outer `.app`, which gives the inside-out ordering Apple
 * requires.
 *
 * Ported from the nested-signing half of the old `scripts/after-pack.mjs`.
 *
 * Usage: node scripts/sign-nested-binaries.mjs [--arch arm64|x64]
 */

import { execFileSync } from "node:child_process";
import { closeSync, existsSync, lstatSync, openSync, readSync, readdirSync, readlinkSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");

const MACH_O_MAGICS = new Set([
  "feedface", "cefaedfe", "feedfacf", "cffaedfe",
  "cafebabe", "bebafeca", "cafebabf", "bfbafeca",
]);

function parseArgs() {
  const args = process.argv.slice(2);
  const archIndex = args.indexOf("--arch");
  const arch = archIndex >= 0 ? args[archIndex + 1] : process.arch;
  return { arch: arch === "x64" ? "x64" : "arm64" };
}

function isMachO(target) {
  let fd;
  try {
    fd = openSync(target, "r");
    const header = Buffer.alloc(4);
    if (readSync(fd, header, 0, header.length, 0) < header.length) return false;
    return MACH_O_MAGICS.has(header.toString("hex"));
  } catch {
    return false;
  } finally {
    if (typeof fd === "number") {
      try { closeSync(fd); } catch { /* best effort */ }
    }
  }
}

/** codesign refuses to run over a bundle containing dangling links. */
function removeBrokenSymlinks(dir) {
  if (!existsSync(dir)) return;

  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    let stat;
    try { stat = lstatSync(full); } catch { continue; }

    if (stat.isSymbolicLink()) {
      try {
        const target = readlinkSync(full);
        const resolved = target.startsWith("/") ? target : path.join(dir, target);
        if (!existsSync(resolved)) rmSync(full, { force: true });
      } catch {
        rmSync(full, { force: true });
      }
      continue;
    }

    if (stat.isDirectory()) removeBrokenSymlinks(full);
  }
}

function collectSignable(dir, out = []) {
  if (!existsSync(dir)) return out;

  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    let stat;
    try { stat = lstatSync(full); } catch { continue; }

    // Symlinks are skipped: signing the target once is enough, and signing
    // through a link would just rewrite the same file repeatedly.
    if (stat.isSymbolicLink()) continue;
    if (stat.isDirectory()) {
      collectSignable(full, out);
      continue;
    }

    const looksNative =
      entry.endsWith(".dylib") || entry.endsWith(".node") || (stat.mode & 0o111) !== 0;
    if (!looksNative || !isMachO(full)) continue;

    out.push({ path: full, mode: stat.mode });
  }

  return out;
}

function main() {
  if (process.platform !== "darwin") {
    console.log("[sign-nested] not macOS, nothing to do.");
    return;
  }

  const { arch } = parseArgs();
  const bundleDir = path.join(projectRoot, "build", "server-bundle", `mac-${arch}`);
  if (!existsSync(bundleDir)) {
    throw new Error(
      `[sign-nested] ${bundleDir} does not exist. Run \`pnpm prepare-server\` first.`,
    );
  }

  const identity = process.env.APPLE_CODESIGN_IDENTITY?.trim() || process.env.CSC_NAME?.trim() || "";
  if (!identity) {
    if (process.env.ALLOW_UNSIGNED_MACOS_BUILD === "true") {
      console.log("[sign-nested] unsigned build explicitly allowed, skipping.");
      return;
    }
    throw new Error(
      "[sign-nested] APPLE_CODESIGN_IDENTITY or CSC_NAME is required. " +
      "Set ALLOW_UNSIGNED_MACOS_BUILD=true only for deliberate local unsigned builds.",
    );
  }

  console.log(`[sign-nested] cleaning dangling symlinks in ${bundleDir}...`);
  removeBrokenSymlinks(bundleDir);

  const entitlements = path.join(projectRoot, "build", "entitlements.mac.inherit.plist");

  // Inside out: libraries first, then executables, deepest paths before their
  // parents. Apple requires nested code to be signed before whatever contains it.
  const binaries = collectSignable(bundleDir).sort((left, right) => {
    const leftIsLib = left.path.endsWith(".dylib") || left.path.endsWith(".node");
    const rightIsLib = right.path.endsWith(".dylib") || right.path.endsWith(".node");
    if (leftIsLib !== rightIsLib) return leftIsLib ? -1 : 1;
    return right.path.split("/").length - left.path.split("/").length;
  });

  console.log(`[sign-nested] signing ${binaries.length} binaries for mac-${arch}...`);
  for (const { path: target, mode } of binaries) {
    const isLibrary = target.endsWith(".dylib") || target.endsWith(".node");
    const needsEntitlements = !isLibrary && (mode & 0o111) !== 0;

    const args = ["--force", "--options", "runtime", "--timestamp", "--sign", identity];
    if (needsEntitlements) args.push("--entitlements", entitlements);
    args.push(target);

    execFileSync("codesign", args, { stdio: "inherit" });
  }

  console.log("[sign-nested] done.");
}

main();
