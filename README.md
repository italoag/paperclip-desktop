<p align="center">
  <strong>Paperclip Desktop</strong><br/>
  A one-click desktop app for <a href="https://github.com/paperclipai/paperclip">Paperclip</a>.
</p>

<p align="center">
  <a href="#quickstart"><strong>Quickstart</strong></a> &middot;
  <a href="https://github.com/paperclipai/paperclip"><strong>Paperclip (upstream)</strong></a> &middot;
  <a href="https://docs.paperclip.ing/"><strong>Docs</strong></a> &middot;
  <a href="https://discord.gg/m4HZY7xNG3"><strong>Discord</strong></a>
</p>

<p align="center">
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License" /></a>
</p>

<br/>

## What is this?

**Paperclip Desktop** is an unofficial [Tauri](https://tauri.app/) wrapper around [Paperclip](https://github.com/paperclipai/paperclip). It bundles Paperclip (via the official `@paperclipai/server` npm package) inside a native desktop app for macOS.

The shell is written in Rust and uses the system WebView instead of shipping a browser engine, which keeps it around 10 MB instead of 275 MB.

The goal is simple: **make running Paperclip as easy as opening an app.**

- 🖱 **One click to launch** — double-click the app and Paperclip starts automatically. No terminal, no `pnpm install`, no Node.js setup.
- 🌐 **Remote connection mode** — connect the desktop shell to verified upstream Paperclip remotes in authenticated mode, with saved connections and quick reconnects.
- 📦 **Paperclip inside** — ships an unmodified build of the upstream Paperclip server and UI. What you get in the app is exactly what you'd get by cloning and running the main repo.
- 🔄 **Auto-updates** — pulls new desktop releases automatically via GitHub Releases.
- 🖥 **Native menus, windowing, and system tray** — the Paperclip UI, but as a real desktop app.

Under the hood, the app can either:

1. Run the embedded local server flow
2. Or verify a remote Paperclip origin via `/api/health` and `/api/auth/get-session` before loading it in a restricted, origin-locked window

When local mode is active, the server is cleanly shut down when you quit the app.

> For everything Paperclip itself can do — orchestrating AI agents, running autonomous companies, governance, budgets, org charts, etc. — see the **[upstream Paperclip repo](https://github.com/paperclipai/paperclip)**. This project does not modify or extend Paperclip's functionality; it only packages it.

<br/>

## Relationship to upstream Paperclip

| This repo (`paperclip-desktop`)              | Upstream ([`paperclipai/paperclip`](https://github.com/paperclipai/paperclip)) |
| -------------------------------------------- | ------------------------------------------------------------------------------ |
| Tauri/Rust shell + installers                | The actual Paperclip server, UI, and features                                  |
| Maintained by Aron Prins (community)         | Maintained by the Paperclip team                                               |
| Bundles `@paperclipai/server` as a dependency| Source of that package                                                         |
| MIT licensed                                 | MIT licensed                                                                   |

This is an **independent community distribution**. It is not an official Paperclip release. All credit for Paperclip itself goes to the upstream project and its authors.

<br/>

## Quickstart

### Install the desktop app

Download the latest installer for your platform from the [Releases page](https://github.com/aronprins/paperclip-desktop/releases):

- **macOS 14 (Sonoma) or newer** — `.dmg` (Apple Silicon and Intel)

> Windows and Linux builds are not currently produced. macOS 13 and older are supported only by releases up to v3.2.11.

Open the app. Paperclip starts automatically and the dashboard opens in the app window. That's it.

### Prefer the original?

If you'd rather run Paperclip yourself from source (no desktop wrapper), follow the upstream instructions:

```bash
npx paperclipai onboard --yes
```

Or:

```bash
git clone https://github.com/paperclipai/paperclip.git
cd paperclip
pnpm install
pnpm dev
```

See [github.com/paperclipai/paperclip](https://github.com/paperclipai/paperclip) for full docs.

<br/>

## Development

Requirements: Rust (stable), Node.js 20+, pnpm 9.15+.

```bash
pnpm install           # Install the Node tooling used by the build scripts
pnpm dev               # Run the shell against the server in node_modules
pnpm test              # cargo test
pnpm check             # clippy -D warnings + rustfmt
pnpm prepare-server    # Stage the Paperclip server bundle into build/
pnpm build-ui          # Stage the Paperclip UI
pnpm bundle:mac:arm64  # Build a signed .app + .dmg for Apple Silicon
pnpm bundle:mac:x64    # ...and for Intel
```

Key files:

- `src-tauri/src/lib.rs` — app setup, startup flow, signal handling
- `src-tauri/src/boot.rs` — local/remote connection flows
- `src-tauri/src/server/` — embedded server supervision
- `src-tauri/src/connection/` — URL validation, remote preflight, saved profiles
- `src-tauri/src/windows/` — window construction and hardening
- `ui/` — the launcher frontend (static files, no bundler)
- `scripts/` — server staging, nested code signing, updater manifest

See [docs/development/architecture.md](docs/development/architecture.md) for the full picture.
