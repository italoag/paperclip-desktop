#!/usr/bin/env node

/**
 * Installs @paperclipai/server from npm into per-architecture staging
 * directories, then assembles the server bundle that Tauri packages
 * into the app.
 */

import { execFileSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(__dirname, "..");

const stagingRootDir = path.join(projectRoot, "build", "server-staging");
const bundleRootDir = path.join(projectRoot, "build", "server-bundle");

const projectPkg = JSON.parse(readFileSync(path.join(projectRoot, "package.json"), "utf8"));
const serverVersion = projectPkg.devDependencies["@paperclipai/server"];
if (!serverVersion) {
  console.error("[prepare-server] @paperclipai/server not found in devDependencies");
  process.exit(1);
}

const serverBundleOverrides = {
  ...(projectPkg.pnpm?.overrides ?? {}),
  // cssstyle currently resolves a 5.x css-color release that trips
  // Node 22's ERR_REQUIRE_ASYNC_MODULE path in the packaged runtime.
  "@asamuzakjp/css-color": "4.1.2",
};

console.log(`[prepare-server] Target server version: @paperclipai/server@${serverVersion}`);

const platform = process.platform;
const nodePlatform = platform === "win32" ? "win32" : platform;
const ebPlatform = platform === "darwin" ? "mac" : platform === "win32" ? "win" : "linux";
const targetArches = platform === "darwin" ? ["x64", "arm64"] : ["x64"];

rmSync(stagingRootDir, { recursive: true, force: true });
rmSync(bundleRootDir, { recursive: true, force: true });

mkdirSync(stagingRootDir, { recursive: true });
mkdirSync(bundleRootDir, { recursive: true });

function fixDylibSymlinks(libDir) {
  if (!existsSync(libDir)) return;

  for (const file of readdirSync(libDir)) {
    const match = file.match(/^(lib[^.]+)\.(\d+)(\.\d+)+\.dylib$/);
    if (!match) continue;

    const base = match[1];
    const major = match[2];

    for (const alias of [`${base}.${major}.dylib`, `${base}.dylib`]) {
      const aliasPath = path.join(libDir, alias);
      try {
        lstatSync(aliasPath);
        rmSync(aliasPath, { force: true });
      } catch {
        // ignore missing alias
      }

      symlinkSync(file, aliasPath);
      console.log(`[prepare-server]   ${alias} -> ${file}`);
    }
  }
}

function removeFinderDuplicates(rootDir) {
  function* walkDir(dir) {
    for (const entry of readdirSync(dir)) {
      const full = path.join(dir, entry);

      let stat;
      try {
        stat = lstatSync(full);
      } catch {
        continue;
      }

      if (stat.isSymbolicLink()) continue;
      if (stat.isDirectory()) {
        yield* walkDir(full);
      } else {
        yield full;
      }
    }
  }

  const duplicates = [];
  for (const file of walkDir(rootDir)) {
    const base = path.basename(file);
    if (/ \d+(\.[^/]+)?$/.test(base) && / \d+/.test(base)) {
      duplicates.push(file);
    }
  }

  if (duplicates.length === 0) {
    console.log("[prepare-server] No Finder duplicates found.");
    return;
  }

  console.warn(`[prepare-server] WARNING: found ${duplicates.length} Finder duplicate file(s). Removing them now.`);
  for (const file of duplicates) {
    rmSync(file, { force: true });
    console.warn(`[prepare-server]   removed: ${path.relative(rootDir, file)}`);
  }
}

function validateMigrations(bundleServerDir) {
  const migrationsDir = path.join(
    bundleServerDir,
    "node_modules",
    "@paperclipai",
    "db",
    "dist",
    "migrations",
  );

  if (!existsSync(migrationsDir)) {
    console.error(`[prepare-server] ERROR: Migrations directory not found at ${migrationsDir}`);
    process.exit(1);
  }

  const sqlFiles = readdirSync(migrationsDir).filter((file) => file.endsWith(".sql")).sort();
  console.log(`[prepare-server] Migration files validated: ${sqlFiles.length} SQL file(s) present.`);
  if (sqlFiles.length === 0) {
    console.error("[prepare-server] ERROR: No migration SQL files found in @paperclipai/db. The app will fail to initialise the database.");
    process.exit(1);
  }
}

function normalizeEsbuildBinary(bundleServerDir, arch) {
  const packageArch = arch === "x64" ? "x64" : "arm64";
  const esbuildBin = path.join(bundleServerDir, "node_modules", "esbuild", "bin", "esbuild");
  const archEsbuildBin = path.join(
    bundleServerDir,
    "node_modules",
    "@esbuild",
    `darwin-${packageArch}`,
    "bin",
    "esbuild",
  );

  if (!existsSync(esbuildBin) || !existsSync(archEsbuildBin)) return;

  cpSync(archEsbuildBin, esbuildBin);
  console.log(`[prepare-server] Normalized esbuild binary for darwin-${packageArch}.`);
}

/**
 * Strips files the running server never reads.
 *
 * Measured against the shipped v3.2.11 payload: source maps 36 MB and `.d.ts`
 * declarations 27 MB — ~63 MB that exists purely to serve editors and CI.
 * Neither is reachable at runtime, and every megabyte here is a megabyte in the
 * .dmg and in every future update download.
 *
 * Restricted to the two categories that are *provably* dead. Anything a program
 * could plausibly read at runtime stays, however much it looks like docs.
 *
 * Deliberately conservative: it only removes files matched by extension or by
 * a directory name that cannot be a real dependency, and it never touches
 * anything executable.
 */
function pruneBuildTimeArtifacts(bundleDir) {
  // Markdown is NOT in this list, and must not be added: Paperclip ships
  // markdown as runtime *data* — `dist/onboarding-assets/**/*.md` are the
  // default agent instructions read when hiring an agent, and `skills/**/*.md`
  // are the skill definitions. Pruning them produced a 500 on the onboarding
  // "hire" call with a bare ENOENT. `validateRuntimeAssets` now guards this.
  const DROP_EXTENSIONS = [".map", ".d.ts"];
  // Only names that cannot be a require target. `test`, `tests`, `docs` and
  // `examples` are excluded on purpose: real packages do ship modules with
  // those names, and a false positive breaks the server at runtime for 2 MB.
  const DROP_DIRECTORIES = new Set(["__tests__", "__mocks__", ".github", ".vscode"]);
  // Plain `.ts` sources are also left alone: the plugin SDK can compile
  // TypeScript at runtime, so they are not provably dead the way `.d.ts` is.

  let removedBytes = 0;
  let removedFiles = 0;

  function walk(dir) {
    let entries;
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }

    for (const entry of entries) {
      const full = path.join(dir, entry.name);
      if (entry.isSymbolicLink()) continue;

      if (entry.isDirectory()) {
        if (DROP_DIRECTORIES.has(entry.name)) {
          removedBytes += directorySize(full);
          rmSync(full, { recursive: true, force: true });
          removedFiles += 1;
          continue;
        }
        walk(full);
        continue;
      }

      if (!DROP_EXTENSIONS.some((ext) => entry.name.endsWith(ext))) continue;

      let stat;
      try {
        stat = lstatSync(full);
      } catch {
        continue;
      }
      // Never remove something marked executable, whatever its name says.
      if ((stat.mode & 0o111) !== 0) continue;

      removedBytes += stat.size;
      removedFiles += 1;
      rmSync(full, { force: true });
    }
  }

  function directorySize(dir) {
    let total = 0;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isSymbolicLink()) continue;
      try {
        total += entry.isDirectory() ? directorySize(full) : lstatSync(full).size;
      } catch {
        // ignore
      }
    }
    return total;
  }

  walk(bundleDir);
  console.log(
    `[prepare-server]   pruned ${removedFiles} build-time entries (${(removedBytes / 1048576).toFixed(0)} MB)`,
  );
}

/**
 * Fails the build if runtime assets went missing from the bundle.
 *
 * Exists because a pruning pass once deleted `dist/onboarding-assets/**\/*.md`
 * and `skills/**\/*.md`, and nothing noticed until agent hiring returned a 500
 * at runtime. Packaging mistakes must fail here, not in a user's onboarding.
 */
function validateRuntimeAssets(bundleServerDir, referenceServerDir) {
  // `ui-dist` is deliberately absent: it is staged later by `build-ui.mjs`, so
  // it cannot be compared against the npm package at this point.
  const checks = [
    ["dist/onboarding-assets", ".md"],
    ["skills", ".md"],
  ];

  for (const [relative, extension] of checks) {
    const reference = path.join(referenceServerDir, relative);
    if (!existsSync(reference)) continue; // Not shipped by this server version.

    const expected = countFiles(reference, extension);
    const actual = countFiles(path.join(bundleServerDir, relative), extension);

    if (actual < expected) {
      throw new Error(
        `[prepare-server] ${relative} is missing ${expected - actual} of ${expected} ` +
        `${extension} files in the bundle. Runtime assets must not be pruned.`,
      );
    }
  }

  console.log("[prepare-server] Runtime assets validated.");
}

function countFiles(dir, extension) {
  if (!existsSync(dir)) return 0;

  let total = 0;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) total += countFiles(full, extension);
    else if (entry.name.endsWith(extension)) total += 1;
  }
  return total;
}

for (const arch of targetArches) {
  const variant = `${ebPlatform}-${arch}`;
  const stagingDir = path.join(stagingRootDir, variant);
  const bundleDir = path.join(bundleRootDir, variant);
  const bundleServerDir = path.join(bundleDir, "server");

  console.log(`[prepare-server] Installing runtime bundle for ${variant}...`);

  mkdirSync(stagingDir, { recursive: true });
  writeFileSync(
    path.join(stagingDir, "package.json"),
    JSON.stringify(
      {
        private: true,
        dependencies: { "@paperclipai/server": serverVersion },
        overrides: serverBundleOverrides,
      },
      null,
      2,
    ),
  );

  // `--arch` was removed as a CLI flag in newer npm; the npm_config_* env vars
  // below are the supported way to drive a cross-arch install.
  execFileSync(
    "npm",
    ["install", "--production", `--os=${nodePlatform}`, `--cpu=${arch}`],
    {
      cwd: stagingDir,
      stdio: "inherit",
      env: {
        ...process.env,
        npm_config_arch: arch,
        npm_config_platform: nodePlatform,
        npm_config_target_arch: arch,
      },
    },
  );

  console.log(`[prepare-server] Assembling server bundle for ${variant}...`);
  mkdirSync(bundleServerDir, { recursive: true });

  const serverPkgDir = path.join(stagingDir, "node_modules", "@paperclipai", "server");

  cpSync(path.join(serverPkgDir, "dist"), path.join(bundleServerDir, "dist"), { recursive: true });
  cpSync(path.join(serverPkgDir, "package.json"), path.join(bundleServerDir, "package.json"));

  const skillsSrc = path.join(serverPkgDir, "skills");
  if (existsSync(skillsSrc)) {
    cpSync(skillsSrc, path.join(bundleServerDir, "skills"), { recursive: true });
  }

  cpSync(path.join(stagingDir, "node_modules"), path.join(bundleServerDir, "node_modules"), { recursive: true });
  rmSync(path.join(bundleServerDir, "node_modules", ".bin"), { recursive: true, force: true });

  if (platform === "darwin") {
    console.log(`[prepare-server] Fixing dylib symlinks for ${variant} embedded-postgres...`);
    const embeddedPgScope = path.join(bundleServerDir, "node_modules", "@embedded-postgres");
    if (existsSync(embeddedPgScope)) {
      for (const pkgArch of readdirSync(embeddedPgScope)) {
        const libDir = path.join(embeddedPgScope, pkgArch, "native", "lib");
        fixDylibSymlinks(libDir);
      }
    }
    normalizeEsbuildBinary(bundleServerDir, arch);
  }

  console.log(`[prepare-server] Scanning ${variant} bundle for Finder duplicate files...`);
  removeFinderDuplicates(bundleDir);
  console.log(`[prepare-server] Pruning build-time artifacts from ${variant}...`);
  pruneBuildTimeArtifacts(bundleServerDir);
  // Compared against the pristine npm install, which is still on disk at this
  // point — cleanup of the staging directories happens after the whole loop.
  validateRuntimeAssets(
    bundleServerDir,
    path.join(stagingDir, "node_modules", "@paperclipai", "server"),
  );
  validateMigrations(bundleServerDir);
}

const NODE_VERSION = "v22.15.0";
const arches = platform === "darwin" ? ["x64", "arm64"] : ["x64"];
const nodeDownloadPlatform = platform === "win32" ? "win" : platform;

// The Node runtime now lives *inside* each per-arch bundle directory, so a
// single Tauri `bundle.macOS.files` entry maps the whole `app-server` payload
// (`server/` + `node-bin/`) in one go. Under electron-builder these were two
// separate `extraResources` entries.
for (const arch of arches) {
  const destDir = path.join(bundleRootDir, `${ebPlatform}-${arch}`, "node-bin");
  const destBin = path.join(destDir, platform === "win32" ? "node.exe" : "node");

  if (existsSync(destBin)) {
    console.log(`[prepare-server] Node ${NODE_VERSION} ${arch} already downloaded, skipping`);
    continue;
  }

  mkdirSync(destDir, { recursive: true });

  const ext = platform === "win32" ? "zip" : "tar.gz";
  const archiveName = `node-${NODE_VERSION}-${nodeDownloadPlatform}-${arch}`;
  const url = `https://nodejs.org/dist/${NODE_VERSION}/${archiveName}.${ext}`;
  const archivePath = path.join(destDir, `node.${ext}`);

  console.log(`[prepare-server] Downloading Node ${NODE_VERSION} for ${nodeDownloadPlatform}-${arch}...`);

  if (platform === "win32") {
    execFileSync(
      "powershell",
      ["-NoProfile", "-Command", "Invoke-WebRequest -Uri $args[0] -OutFile $args[1]", url, archivePath],
      { stdio: "inherit" },
    );
  } else {
    execFileSync("curl", ["-fsSL", "-o", archivePath, url], { stdio: "inherit" });
  }

  if (platform === "win32") {
    execFileSync(
      "powershell",
      ["-NoProfile", "-Command", "Expand-Archive -Path $args[0] -DestinationPath $args[1] -Force", archivePath, destDir],
      { stdio: "inherit" },
    );
    cpSync(path.join(destDir, archiveName, "node.exe"), destBin);
    rmSync(path.join(destDir, archiveName), { recursive: true, force: true });
  } else {
    execFileSync(
      "tar",
      ["-xzf", archivePath, "-C", destDir, "--strip-components=2", `${archiveName}/bin/node`],
      { stdio: "inherit" },
    );
  }

  rmSync(archivePath, { force: true });
  console.log(`[prepare-server] Node ${NODE_VERSION} ${arch} ready at ${destBin}`);
}

console.log("[prepare-server] Cleaning up staging directories...");
rmSync(stagingRootDir, { recursive: true, force: true });

console.log("[prepare-server] Done.");
