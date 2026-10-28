# Local Testing Guide: Remote Connection Mode

This guide captures the exact local test paths for the remote connection mode implementation on `feature/remote-connection-mode`.

## Quick remote-mode test

Use this when you want to verify the new launcher, chooser, remote connect flow, saved connections, and menu-based quick connect.

```bash
git checkout feature/remote-connection-mode
pnpm install
pnpm dev
```

Notes:

- `pnpm dev` builds the Rust shell and launches it through the Tauri CLI
- this is the fastest path for testing remote mode
- use it to verify:
  - chooser flow
  - remote verification states
  - sign-in-required remotes
  - saved connections CRUD
  - Connection menu quick connect entries

## Full local package test

Use this when you want the packaged desktop app shape rather than the direct dev shell.

```bash
git checkout feature/remote-connection-mode
pnpm install
pnpm test
pnpm dist:mac
```

What `pnpm dist:mac` does:

1. stages the bundled `@paperclipai/server` and the Node runtime per architecture
2. stages the Paperclip UI
3. builds and stages the in-repo adapters (`adapters/`)
4. builds the Rust shell and bundles the `.app` and `.dmg` for each architecture

## Signing

`pnpm dist:mac` refuses to package unsigned by default:

```text
[sign-nested] APPLE_CODESIGN_IDENTITY or CSC_NAME is required.
```

Set `APPLE_CODESIGN_IDENTITY`, or pass `ALLOW_UNSIGNED_MACOS_BUILD=true` for a
deliberate local build. An unsigned `.app` is blocked by Gatekeeper until you run
`xattr -dr com.apple.quarantine` on it.

## Artifact paths

```text
src-tauri/target/<triple>/release/bundle/macos/Paperclip Desktop.app
src-tauri/target/<triple>/release/bundle/dmg/Paperclip Desktop_<version>_<arch>.dmg
```

`pnpm release:manifest` collects those into `release/updater/` together with
`latest.json` for the auto-updater.

## Recommended local test sequence

1. `pnpm dev` — exercise remote mode end to end.
2. `pnpm test` and `pnpm test:adapters` — connection logic, security checks and adapters.
3. `pnpm dist:mac` — only when you need the packaged shape.

## Testing when you do not have a remote Paperclip

You can still test a large part of the feature without a real remote.

### Expected negative-path tests

These are useful and should behave as follows:

- `https://localhost:3100`
  - expected to fail
  - the embedded local Paperclip server is plain HTTP, not HTTPS
  - if nothing is listening there yet, `host unreachable` is expected
- `http://localhost:3100`
  - once the embedded local mode is already running, this should identify Paperclip but reject it for remote mode
  - expected reason: `deploymentMode=local_trusted`
  - this is correct behavior, not a bug
- `https://example.com`
  - expected result: reachable but not Paperclip
- malformed URLs
  - expected result: inline validation failure

Important:

- the desktop app intentionally blocks `local_trusted` remotes in remote mode
- the normal embedded local startup uses `local_trusted` by default
- so `localhost` is not a valid positive remote-mode target for this feature

### Positive-path remote test without owning a real remote machine

If you want to test the success path on your own machine, start a second Paperclip instance manually in upstream `authenticated` mode on a different port.

At minimum, that instance needs:

- `PAPERCLIP_DEPLOYMENT_MODE=authenticated`
- `BETTER_AUTH_SECRET=...`

Useful additional settings for a local authenticated test instance:

- `PORT=3200`
- `PAPERCLIP_AUTH_PUBLIC_BASE_URL=http://127.0.0.1:3200`
- `PAPERCLIP_ALLOWED_HOSTNAMES=127.0.0.1,localhost`

That gives you a remote-mode target like:

```text
http://127.0.0.1:3200
```

This is the easiest way to test:

- verified authenticated remote
- sign-in-required state
- bootstrap/setup-required state
- saved remote profiles
- reconnect behavior
- remote window isolation

If that authenticated test instance has not been bootstrapped yet, or there is no active session yet, Desktop should still open the verified remote and let the upstream Paperclip UI handle setup or sign-in in-app.

## What to verify manually

- Local vs Remote chooser behavior
- `Remember my choice and don't ask again`
- remote verification success for authenticated Paperclip remotes
- sign-in-required remote behavior
- setup-required remote behavior in the remote Paperclip UI
- blocking of:
  - non-Paperclip endpoints
  - `local_trusted` remotes
  - malformed URLs
  - embedded credential URLs
- saved connections:
  - add
  - edit
  - duplicate
  - delete
  - reconnect from the Connection menu
- local boot path still opens the embedded server flow
