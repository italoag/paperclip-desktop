/**
 * Devin CLI adapter for Paperclip.
 *
 * Runs Devin (https://devin.ai) as a Paperclip agent through its Agent Client
 * Protocol server, `devin acp`.
 */

import { ADAPTER_TYPE } from "./identity.js";
import { DEVIN_MODELS } from "./models.js";

export { ADAPTER_TYPE } from "./identity.js";
export const type = ADAPTER_TYPE;
export const label = "Devin (local)";

export const models = DEVIN_MODELS.map((model) => ({
  id: model.id,
  label: model.label,
}));

export { DEVIN_MODELS } from "./models.js";
export type { DevinModel } from "./models.js";
export * from "./config.js";

export const agentConfigurationDoc = `# devin_local agent configuration

Adapter: devin_local

Runs the Devin CLI as an Agent Client Protocol server (\`devin acp\`) and drives
it through ACPX, the same engine the built-in \`acpx_local\` adapter uses. Live
runs therefore stream structured events — messages, thinking, tool calls,
status — into the run viewer, and sessions persist across heartbeats through
ACP \`loadSession\`.

Use when:
- You want Devin to do the work and you want Devin-specific configuration:
  its model catalogue in a dropdown, its specialised agent types, and
  diagnostics that check the \`devin\` binary and its login state.
- The task benefits from Devin's tooling (multi-file edits, shell, subagents).
- You want live progress in the run viewer rather than a single blob of output
  at the end.

Don't use when:
- The \`devin\` binary is not installed on the host that runs the agent, or the
  account is not authenticated (\`devin auth status\`). Run the environment test
  before hiring.
- You need a plain one-shot command with no conversational state — the
  \`process\` adapter is simpler and cheaper.
- You specifically need Claude Code or Codex CLI semantics; use
  \`claude_local\` / \`codex_local\`, which wrap those CLIs directly.

Core fields:
- cwd (string, required): absolute working directory for the agent.
- model (string, optional): Devin model id, e.g. \`claude-sonnet-5-high\`.
  Applied via \`devin acp --model\`. Note that a *resumed* Devin session keeps
  the model it was created with, so changing this affects new sessions.
  The dropdown is discovered live from \`devin models list\` on the server
  host, so it mirrors the subscription of the logged-in account rather than a
  packaged snapshot; set PAPERCLIP_DEVIN_COMMAND if the binary is not on the
  server PATH. When discovery fails the packaged catalogue is shown instead.
- agentType (string, optional): empty for the default agent, or \`summarizer\`
  / \`review\` for Devin's specialised agents.
- command (string, optional): path to the Devin binary. Defaults to \`devin\`.
- permissionMode (string, optional): how ACPX answers Devin's permission
  requests — \`approve-all\` (default), \`approve-reads\`, or \`deny-all\`.
  This is the ACPX layer; \`devin acp\` has no permission flag of its own.
- nonInteractivePermissions (string, optional): \`deny\` or \`fail\` when a
  permission cannot be requested interactively.
- timeoutSec (number, optional): run timeout. Defaults to 0, meaning no
  adapter-side timeout.
- warmHandleIdleMs (number, optional): how long to keep the ACP process warm
  after a successful run. Defaults to 0 (shut down, keep session state).
- promptTemplate (string, optional): run prompt template.
- bootstrapPromptTemplate (string, optional): first-run prompt template.
- env (object, optional): extra environment variables or secret bindings.

Authentication:
Devin authenticates through its own CLI (\`devin auth login\`), not through a
Paperclip-managed API key. The credentials live in the host user's Devin
config, so the account running the Paperclip server must be logged in.
`;

export { createServerAdapter } from "./server/index.js";
