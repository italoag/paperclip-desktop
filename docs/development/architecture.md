# Paperclip Desktop — Architecture & Operations Guide

## Overview

Paperclip Desktop is a **Tauri 2 / Rust** shell around the [Paperclip](https://github.com/paperclipai/paperclip) server. It consumes `@paperclipai/server` from npm — it does **not** contain the engine source.

The shell spawns the Paperclip server as a child process and loads its UI over HTTP on localhost, or connects to a verified remote Paperclip instance. A Node.js binary ships inside the app so end users need nothing installed.

It was an Electron app until v3.2.11; see [Migration from Electron](#migration-from-electron).

## Repository structure

```
paperclip-desktop/
  src-tauri/                       The shell (Rust)
    src/
      lib.rs                       App setup, startup flow, signal handling
      state.rs                     AppState — replaces main.ts's module globals
      paths.rs                     Data directories (see "Data locations")
      runtime_safety.rs            Isolation guard for smoke runs
      boot.rs                      boot_local / boot_remote / boot_saved_profile
      commands.rs                  IPC surface exposed to the launcher
      menu.rs                      Native application menu
      updater.rs                   Auto-update
      connection/                  URL validation, remote preflight, saved profiles
      server/                      Embedded-server supervision
      windows/                     Window construction, hardening, gestures
    tauri.conf.json                Base config
    tauri.macos-{arm64,x64}.conf.json  Per-arch payload + nested signing
    capabilities/launcher.json     Plugin permissions, launcher window only
  ui/                              Launcher frontend (static, no bundler)
    launcher.html / .css / .js
    bridge.js                      window.paperclipLauncher → Tauri IPC
  scripts/
    prepare-server.mjs             Installs the server bundle + Node binary
    build-ui.mjs                   Builds the Paperclip UI from upstream
    sign-nested-binaries.mjs       Signs every Mach-O under app-server
    prepare-updater-manifest.mjs   Builds latest.json for the updater
  build/                           Icons, entitlements, staged server bundle
  .cargo/config.toml               Crates resolve through the JFrog mirror
```

## How it works

### Packaged app

```
Paperclip Desktop.app/Contents/
  MacOS/paperclip-desktop          The Rust shell (~10 MB)
  Resources/app-server/
    server/dist/index.js           @paperclipai/server entry point
    server/ui-dist/                Built Paperclip UI
    server/node_modules/           Flat production tree
    node-bin/node                  Bundled Node.js v22.15.0
```

1. Resolve data directories and refuse to run against production data if isolation was requested
2. Reap any server left behind by a crashed run (identity-checked, see below)
3. Show the launcher, or auto-connect the remembered profile
4. **Local:** find a free port from 3100, spawn the server, stream its output into `server.log` and the progress bar, wait for the port
5. **Remote:** preflight `/api/health` and `/api/auth/get-session`, then open the verified origin
6. Open the main window under a same-origin lock, with media capture disabled and storage isolated per profile

### Development

`pnpm dev` runs against `node_modules/@paperclipai/server`. Unlike the Electron build, this works with pnpm's symlinked layout — see `supervisor::resolve_entry_point` for why that took explicit handling.

## Data locations

| What | Path (macOS) |
|---|---|
| App data | `~/Library/Application Support/Paperclip` |
| Saved connections | `<app data>/connections.json` |
| Server log | `<app data>/server.log` (rotated at 20 MB, 5 archives kept) |
| PID file | `<app data>/paperclip-electron.pid` |
| `PAPERCLIP_HOME` | `~/.paperclip` if it already holds a database, else `<app data>` |

**The app data directory is deliberately not Tauri's default.** Tauri derives it from the bundle identifier (`com.paperclipai.app`); Electron derived it from the app *name* (`Paperclip`). Using Tauri's default would orphan every existing install — saved profiles would vanish and the server would be handed an empty database. `paths::legacy_user_data_dir` reproduces the Electron layout on purpose; do not "simplify" it.

Overrides for smoke tests: `PAPERCLIP_DESKTOP_USER_DATA_DIR`, `PAPERCLIP_HOME`, and `PAPERCLIP_DESKTOP_REQUIRE_ISOLATED_DATA=1` to assert they took effect.

## Security model

The main window renders a web app we do not control — for remote profiles, one on someone else's server. It is treated as untrusted:

| Control | Implementation |
|---|---|
| Same-origin navigation lock | `on_navigation`; off-origin web links go to the browser, everything else is dropped |
| No pop-ups | `on_new_window` → `Deny` |
| No media capture | `initialization_script_for_all_frames` removes `getUserMedia`/`getDisplayMedia` non-configurably |
| Storage isolation per profile | `data_store_identifier`, a UUIDv5 of the profile id (needs macOS 14+) |
| No IPC from remote content | Every command checks the calling window (`only_launcher`) |

Two of these deserve emphasis, because Tauri does not provide them by default:

- **Media capture.** The WKWebView grants `getUserMedia` with no prompt. Electron denied all permission requests via `setPermissionRequestHandler`; Tauri has no equivalent hook, so the guard is an injected script. Do not remove it, and do not add `NSMicrophoneUsageDescription`/`NSCameraUsageDescription` to the Info.plist.
- **IPC reach.** Tauri's capability system governs *plugin* commands only. Application commands are callable from any webview that has the IPC bridge, and the bridge is injected into every webview — including the remote one. `only_launcher` is what keeps a remote origin from enumerating or deleting saved profiles.

## Server supervision

- **Process group.** The child is spawned as a group leader, so one `killpg` reaches the server, its embedded PostgreSQL, and anything else it forked. This replaces the `tree-kill` package, which discovered descendants by shelling out to `ps` and raced a tree that was still spawning.
- **Escalation.** Shutdown sends `SIGTERM`, waits, then `SIGKILL`. The Electron version signalled and resolved immediately, which raced PostgreSQL's shutdown flush.
- **Signals.** `SIGTERM`/`SIGINT`/`SIGHUP` are handled explicitly. Tauri's exit events only fire for an orderly quit; without these handlers a `pkill` or a logout would leave the server and PostgreSQL running.
- **Orphan reaping.** A pid recovered from the pid file is only signalled after its command line confirms it is a Node process running the server. Pids get recycled, and the Electron version would happily kill whatever inherited one.
- **Crash detection.** Boot races the port against the child process, so a server that dies immediately reports its exit code and log tail at once instead of after the 60 s port timeout.

## Build and release

```
pnpm test                # cargo test
pnpm check               # clippy -D warnings + rustfmt
pnpm prepare-server      # installs @paperclipai/server + Node binary per arch
pnpm build-ui            # builds the Paperclip UI from upstream
pnpm bundle:mac:arm64    # signs nested binaries, then bundles .app + .dmg
pnpm bundle:mac:x64
pnpm release:manifest    # builds latest.json from the bundle outputs
```

### Nested code signing

`app-server/node_modules` holds thousands of Mach-O files (embedded PostgreSQL, `.node` addons, esbuild) and notarization requires every one of them to be signed. Tauri's bundler signs only frameworks, sidecars and the `.app`; it never walks `Contents/Resources`.

`sign-nested-binaries.mjs` runs as `beforeBundleCommand` and signs them **in the staging directory**. The signature lives inside the Mach-O file, so it survives the copy into the bundle; Tauri then signs the `.app` around it, giving the inside-out order Apple requires.

### Bundled adapters

Adapters under `adapters/` ship inside the app, so a fresh install has them
without the operator installing anything.

`scripts/bundle-adapters.mjs` builds each one and copies its `dist/` into
`server/node_modules/<name>/` — the server's own flat tree, on purpose. An
adapter depends on `@paperclipai/adapter-utils` and friends, which the server
already ships, so from there Node resolves them from the sibling entries.
Anywhere else would mean a second copy of adapter-utils, handing the plugin
loader types from a different version than the server runs.

Being on disk is not enough: the server only loads adapters listed in
`$PAPERCLIP_HOME/adapter-plugins.json`. The script writes a manifest, and
`server::adapter_plugins` reconciles that file on **every** launch — not just
the first, because the record holds an absolute path and dragging the app from
`~/Downloads` to `/Applications` would otherwise leave it pointing at nothing.
The reconciliation only touches records it owns and preserves the rest of each
one, so an adapter disabled in the UI stays disabled.

Currently bundled: `devin_local` (see `adapters/paperclip-adapter-devin`).

### Pruning the payload

`prepare-server.mjs` strips source maps and `.d.ts` declarations from the staged
bundle — about 63 MB that only ever serves editors and CI.

It removes **nothing else**, and markdown in particular must never be added to
that list. Paperclip ships markdown as runtime *data*:
`dist/onboarding-assets/**/*.md` are the default agent instructions read when
hiring an agent, and `skills/**/*.md` are the skill definitions. Pruning them
saves 5 MB and breaks onboarding with a bare `ENOENT` 500 from
`POST /api/companies/:id/agent-hires`.

`validateRuntimeAssets` compares the bundle against the pristine npm install and
fails the build if either directory lost files, so this cannot regress quietly.

### Why `bundle.macOS.files` and not `bundle.resources`

`bundle.resources` flattens its input and copies file by file, which **dereferences symlinks**. `@embedded-postgres` relies on `.dylib` soname links, and dereferencing them shipped three full copies of `libicudata.dylib` — 144 MB of pure duplication in the Electron builds. `bundle.macOS.files` uses a directory copy that preserves symlinks.

The trade-off: it is macOS-only. Windows and Linux need `bundle.resources` plus a symlink-restoration step, which is why the release workflow is macOS-only today.

## Cost is displayed in ACUs

Cognition bills in ACUs, and `devin_local` reports them into the ledger
one-for-one (`adapters/paperclip-adapter-devin/src/server/acu.ts`). Paperclip
stores a single integer of cents and has no per-adapter unit, so
`scripts/patch-ui-cost-unit.mjs` relabels the staged UI: the cents formatter and
the monthly budget input read `ACU` instead of `$`.

A budget of `100000` cents therefore means **1000 ACUs per month**.

### Why the label is global, and when it stops being true

The formatter receives a number and nothing else, so it cannot tell which
adapter produced a figure. Labelling per adapter was considered and rejected:
of its 59 call sites only 9 have provider context, and the rest — including the
agent list — are sums that can legitimately mix adapters. Patching only the 9
would leave the provider panel reading `0.32 ACU` while the list reads `$0.32`
for the same money, at nine times the upgrade fragility.

The global label is therefore correct **only while every agent runs
`devin_local`**. Add a Claude or Codex agent and its real dollars will be
labelled `ACU` and be wrong. If that happens, either revert this patch and set
`acuRateUsd` on the Devin agents so every figure is genuine money, or accept the
mislabelling knowingly.

## Which Paperclip version ships

The Paperclip server is **pinned**, not floating: `prepare-server.mjs` reads the
exact version from `devDependencies["@paperclipai/server"]` and installs that.
A desktop build therefore ships the one server it was built and tested against,
and the same commit always produces the same bundle.

The consequence is that nothing bumps it for you. `tauri-plugin-updater` replaces
the whole `.app`, and the server inside it is whatever was pinned when that `.app`
was built — so shipping a newer Paperclip means bumping the pin and cutting a
release.

```sh
make outdated   # pinned vs. staged vs. installed vs. latest on npm
```

To upgrade:

```sh
pnpm add -D @paperclipai/server@<version>
make clean-bundle && make verify && make dist
```

`clean-bundle` matters: the staged bundle is keyed on the file, not the version,
so a stale `build/server-bundle/` would otherwise be reused.

`/api/health` reports the server version directly from 2026.817.0 onward
(`version` and `serverVersion`). Older servers returned a git-describe string
there instead, which is why `make server-version` reads the bundled
`package.json` rather than trusting the endpoint.

## Auto-update

`tauri-plugin-updater`, checking on launch and every four hours.

| | Electron (≤ 3.2.11) | Tauri |
|---|---|---|
| Manifest | `latest-mac.yml` | `latest.json` |
| Artifact | `.zip` | `.app.tar.gz` |
| Integrity | sha512 in the manifest | minisign signature, verified before install |

The formats are incompatible and there is **no bridge**: clients on the Electron build will not see Tauri releases and must reinstall once. The signing keypair is generated with `cargo tauri signer generate`; the public key lives in `tauri.conf.json` and the private key belongs in the `TAURI_SIGNING_PRIVATE_KEY` secret.

## Dependencies

Crates resolve through the JFrog mirror configured in `.cargo/config.toml`. That file holds **no credentials**; authentication comes from `CARGO_REGISTRIES_ARTIFACTORY_TOKEN`, injected from a repository secret in CI. Use `cargo add` rather than editing `Cargo.toml` by hand.

TLS uses `native-tls` (Security.framework), not rustls, on purpose: it is the same trust store Chromium used, so corporate and self-signed CAs installed in the keychain keep working, and it avoids linking `aws-lc-rs`.

## Migration from Electron

The Electron shell is gone. Removed: `src/*.ts`, `electron-builder.yml`,
`tsconfig.json`, the `after-pack` / `after-sign` / `stage-after-pack` /
`release-macos-local` / `prepare-macos-release-assets` scripts, the
`test/*.test.mjs` suite (ported to `cargo test`), the manual
`notarize-submit` / `notarize-status` workflows, and the runbooks that drove
them. Nothing in the build depends on Electron: it appears in neither
`package.json`, the lockfile, nor `node_modules`.

Notarization moved into `release.yml`. Tauri does it inline when `APPLE_ID`,
`APPLE_PASSWORD` and `APPLE_TEAM_ID` are set, which replaces the two-step
submit-then-poll workflows the Electron pipeline needed.

### Legacy that is kept on purpose

Two things still carry the Electron name, and both are load-bearing:

- **`paperclip-electron.pid`** (`server::pid_file::PID_FILE_NAME`). The pid file
  is how a crashed run's orphaned server is found and reaped on the next launch.
  Renaming it would make one upgrade blind to a server the previous version left
  running — it would hold the port and the database with nothing to reap it.
- **`~/Library/Application Support/Paperclip`** as the data directory
  (`paths::legacy_user_data_dir`). Electron derived it from the app *name*;
  Tauri would derive it from the bundle identifier. Adopting Tauri's default
  would orphan every existing install's saved profiles and database.

Behavioural differences worth knowing:

- **macOS 14+ is required** (`data_store_identifier`). Older systems stay on the
  last Electron release.
- **Windows and Linux builds are gone** for now; see the packaging note above.
- **No update path from Electron builds** — the manifest and artifact formats
  are incompatible, so the first Tauri release is a fresh install.

## Troubleshooting

**Server does not start.** Check `<app data>/server.log`. The boot dialog now includes the exit code and the last 30 lines, so the cause is usually there.

**"Server exited during startup (code 0)" with no output in development.** The server only runs when `process.argv[1]` matches its own `import.meta.url`. Under pnpm the package is a symlink, so the paths disagree and it exits silently. `supervisor::resolve_entry_point` canonicalises the path to prevent this; if you bypass the supervisor, pass the realpath.

**Editing `ui/` changes nothing.** The frontend is embedded at compile time. `build.rs` emits `rerun-if-changed` for `ui/`, so a plain `cargo build` picks it up — but a stale `target/` from before that was added needs one clean rebuild.

**Orphaned server.** Detected and reaped on the next launch. Manually: `kill $(cat "~/Library/Application Support/Paperclip/paperclip-electron.pid")`.
