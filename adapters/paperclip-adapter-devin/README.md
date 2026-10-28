# paperclip-adapter-devin

Run [Devin](https://devin.ai) as a Paperclip agent.

Adapter type: `devin_local`

## How it works

Devin ships an Agent Client Protocol server (`devin acp`), and Paperclip already
bundles `acpx_local`, which drives any ACP server and turns its stream into the
events the run viewer renders. This adapter is the thin layer between them: it
presents a Devin-shaped configuration and translates it into ACPX's.

```
Paperclip ──> devin_local ──> acpx_local (ACPX) ──> devin acp
              (this package)   (bundled)            (Devin CLI)
```

Everything hard — the ACP handshake, session reuse, permission prompts, live
event streaming — comes from ACPX. What this package adds is the part ACPX
cannot know about:

- Devin's models in a grouped dropdown instead of a free-text field —
  discovered live from `devin models list` on the server host, so the list
  always mirrors the logged-in account's subscription rather than a snapshot
  baked at packaging time (the baked catalogue remains as a fallback when the
  CLI cannot be asked; `PAPERCLIP_DEVIN_COMMAND` overrides which binary is
  probed)
- Devin's specialised agent types (`review`, `summarizer`)
- Environment diagnostics: binary present, `devin acp` supported, account
  logged in, working directory valid
- Defaults that make a Devin agent work without hand-writing JSON

You can get the same execution today with the built-in `acpx_local` and
`agent: "custom"`, `agentCommand: "devin acp"`. This package exists for the
configuration surface, not for the execution.

## Requirements

- Devin CLI installed and authenticated (`devin auth login`)
- Node >= 22.12.0 on the host running the Paperclip server (an ACPX requirement)
- Paperclip with external adapter support (`@paperclipai/server` >= 2026.707.0)

## Install

```sh
pnpm install && pnpm build
```

Then register it with your Paperclip instance, either through the API:

```sh
curl -X POST http://localhost:3100/api/adapters/install \
  -H "Content-Type: application/json" \
  -d '{"packageName": "/absolute/path/to/paperclip-adapter-devin", "isLocalPath": true}'
```

or by adding it to `$PAPERCLIP_HOME/adapter-plugins.json` and restarting:

```json
[
  {
    "packageName": "paperclip-adapter-devin",
    "localPath": "/absolute/path/to/paperclip-adapter-devin",
    "type": "devin_local",
    "installedAt": "2026-08-20T23:30:00.000Z"
  }
]
```

Confirm it loaded:

```sh
curl -s http://localhost:3100/api/adapters | jq '.[] | select(.type=="devin_local")'
```

## Configuration

| Field | Default | Notes |
|---|---|---|
| `cwd` | — | Required. Absolute working directory. |
| `model` | account default | Devin model id, e.g. `claude-sonnet-5-high`. |
| `agentType` | default agent | `review` or `summarizer` for Devin's specialised agents. |
| `permissionMode` | `approve-all` | How **ACPX** answers permission requests. |
| `command` | `devin` | Path to the binary if it is not on PATH. |
| `timeoutSec` | `0` | 0 means no adapter-side timeout. |
| `warmHandleIdleMs` | `0` | Keep the ACP process warm between runs. |
| `promptTemplate` | — | Supports `{{variable}}` substitution. |

Two behaviours worth knowing:

- **Model changes only affect new sessions.** Devin pins a session's model when
  it is created, so switching `model` will not move an existing conversation.
- **`permissionMode` is the ACPX layer.** In ACP mode Devin delegates approvals
  to the client over the protocol; its own `--permission-mode` flag does not
  apply.

## Known limitation: no token accounting

Runs report **0 tokens and 0 cost**, so Paperclip's monthly budget never
advances and the budget hard-stop cannot trigger for this agent.

This is not specific to Devin. ACPX exposes usage through
`AcpRuntimeStatus.usage`, but `@paperclipai/adapter-acpx-local` never reads it —
every ACPX-backed adapter, Claude and Codex included, has the same gap.

It is deliberately not worked around here: recovering the numbers from outside
would mean proxying ACPX's internal runtime object, and a wrapper that
mis-forwards a single method would break every run. The fix is a few lines in
acpx_local's `execute`, upstream. The environment test reports this as a warning
so it is visible before you hire an agent.

Until then, bound work with `timeoutSec` and Paperclip's run limits rather than
a token budget.

## Development

```sh
pnpm typecheck
pnpm test
pnpm build
pnpm refresh-models   # regenerate src/models.ts from `devin models list`
```

Local-path installs are watched by the Paperclip server: rebuilding `dist/`
reloads the adapter without reinstalling.

## Credits

`src/ui-parser.ts` is a port of the transcript parser from
`@paperclipai/adapter-acpx-local` (MIT, Copyright (c) 2025 Paperclip AI),
because the event stream is ACPX's. A built-in adapter gets its parser compiled
into the UI; an external one has to ship its own copy.

## License

MIT
