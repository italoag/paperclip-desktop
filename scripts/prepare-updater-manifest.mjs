#!/usr/bin/env node

/**
 * Builds the `latest.json` manifest that `tauri-plugin-updater` polls, and
 * gathers the release artefacts into one directory.
 *
 * Replaces `prepare-macos-release-assets.mjs`, which produced the
 * `electron-updater` format. The two are not interchangeable:
 *
 * | | electron-updater | tauri-plugin-updater |
 * |---|---|---|
 * | Manifest | `latest-mac.yml` (YAML, sha512 per file) | `latest.json` (JSON, minisign per platform) |
 * | Artifact | `.zip` of the `.app` | `.app.tar.gz` |
 * | Integrity | sha512 in the manifest | detached minisign signature, verified before install |
 *
 * Because the signature is what authorises an install, an artefact without its
 * `.sig` is treated as a hard error rather than skipped.
 *
 * Usage:
 *   node scripts/prepare-updater-manifest.mjs \
 *     --version 3.3.0 \
 *     --bundle-dir src-tauri/target/aarch64-apple-darwin/release/bundle \
 *     --bundle-dir src-tauri/target/x86_64-apple-darwin/release/bundle \
 *     --output-dir release/updater \
 *     [--base-url https://github.com/<owner>/<repo>/releases/download/v3.3.0]
 */

import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");
const args = process.argv.slice(2);

/** Rust target triple -> the platform key the updater looks up. */
const PLATFORM_KEYS = {
  "aarch64-apple-darwin": "darwin-aarch64",
  "x86_64-apple-darwin": "darwin-x86_64",
};

const DEFAULT_BASE_URL = "https://github.com/aronprins/paperclip-desktop/releases/download";

function option(name, { required = false, multiple = false } = {}) {
  const values = [];
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name) values.push(args[index + 1]);
  }

  if (required && values.length === 0) throw new Error(`Missing required option ${name}.`);
  return multiple ? values : (values[0] ?? null);
}

/** Infers the target triple from a `target/<triple>/release/bundle` path. */
function tripleOf(bundleDir) {
  const normalized = path.resolve(bundleDir).split(path.sep);
  const targetIndex = normalized.lastIndexOf("target");
  const candidate = targetIndex >= 0 ? normalized[targetIndex + 1] : null;

  if (candidate && PLATFORM_KEYS[candidate]) return candidate;

  throw new Error(
    `Could not infer a target triple from ${bundleDir}. ` +
    `Expected a path like target/<${Object.keys(PLATFORM_KEYS).join("|")}>/release/bundle.`,
  );
}

/**
 * Adds a platform suffix, unless the name already identifies the architecture.
 *
 * Tauri names DMGs `..._aarch64.dmg` / `..._x64.dmg`, so blindly appending
 * `-darwin-aarch64` produced `Paperclip Desktop_3.2.11_aarch64-darwin-aarch64.dmg`.
 */
function suffixed(name, platformKey) {
  const arch = platformKey.split("-")[1];
  const alreadyTagged = arch === "aarch64"
    ? /_(aarch64|arm64)\./.test(name)
    : /_(x86_64|x64)\./.test(name);

  return alreadyTagged ? name : name.replace(/(\.[^.]+)$/, `-${platformKey}$1`);
}

function findUpdaterArtifact(bundleDir) {
  const macosDir = path.join(bundleDir, "macos");
  if (!existsSync(macosDir)) {
    throw new Error(`${macosDir} does not exist; was the bundle built?`);
  }

  const archive = readdirSync(macosDir).find((name) => name.endsWith(".app.tar.gz"));
  if (!archive) {
    throw new Error(
      `No .app.tar.gz in ${macosDir}. Is "createUpdaterArtifacts" enabled in tauri.conf.json?`,
    );
  }

  const archivePath = path.join(macosDir, archive);
  const signaturePath = `${archivePath}.sig`;
  if (!existsSync(signaturePath)) {
    throw new Error(
      `${archive} has no .sig next to it. The build needs TAURI_SIGNING_PRIVATE_KEY set; ` +
      "an unsigned artefact would be rejected by every client.",
    );
  }

  return { archivePath, signaturePath, archiveName: archive };
}

function main() {
  const version = option("--version", { required: true }).replace(/^v/, "");
  const bundleDirs = option("--bundle-dir", { required: true, multiple: true });
  const outputDir = path.resolve(projectRoot, option("--output-dir") ?? "release/updater");
  const baseUrl = (option("--base-url") ?? `${DEFAULT_BASE_URL}/v${version}`).replace(/\/$/, "");
  const notes = option("--notes") ?? `Paperclip Desktop v${version}`;

  rmSync(outputDir, { recursive: true, force: true });
  mkdirSync(outputDir, { recursive: true });

  const platforms = {};

  for (const bundleDir of bundleDirs) {
    const triple = tripleOf(bundleDir);
    const platformKey = PLATFORM_KEYS[triple];
    const { archivePath, signaturePath, archiveName } = findUpdaterArtifact(bundleDir);

    // Per-arch filename: both architectures land in one GitHub release, so the
    // names have to differ. The bundler names `.app.tar.gz` identically for
    // every target, but `.dmg` already carries an arch suffix — see `suffixed`.
    const publishedName = archiveName.replace(/\.app\.tar\.gz$/, `-${platformKey}.app.tar.gz`);
    copyFileSync(archivePath, path.join(outputDir, publishedName));

    platforms[platformKey] = {
      signature: readFileSync(signaturePath, "utf8").trim(),
      url: `${baseUrl}/${encodeURIComponent(publishedName)}`,
    };

    // Ship the installers alongside the update artefacts.
    const dmgDir = path.join(bundleDir, "dmg");
    if (existsSync(dmgDir)) {
      for (const dmg of readdirSync(dmgDir).filter((name) => name.endsWith(".dmg"))) {
        copyFileSync(path.join(dmgDir, dmg), path.join(outputDir, suffixed(dmg, platformKey)));
      }
    }

    console.log(`[updater-manifest] ${platformKey}: ${publishedName}`);
  }

  const manifest = {
    version,
    notes,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    platforms,
  };

  writeFileSync(path.join(outputDir, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`[updater-manifest] wrote ${path.join(outputDir, "latest.json")}`);
}

main();
