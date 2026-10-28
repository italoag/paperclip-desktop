/**
 * Declarative config form for the board UI.
 *
 * An external adapter has no React bundle in the UI, so this schema is the only
 * thing standing between an operator and hand-writing the adapterConfig JSON.
 * The model list in particular is the reason this adapter exists rather than a
 * bare `acpx_local` with a custom command.
 */

import type { AdapterConfigSchema } from "@paperclipai/adapter-utils";

import { DEFAULT_COMMAND, DEFAULT_PERMISSION_MODE, DEFAULT_TIMEOUT_SEC } from "../config.js";
import { loadDevinModelCatalogue, type ModelsListRunner } from "./models.js";

/**
 * The model dropdown must mirror the subscription of the account the host's
 * `devin` is logged into, so its options come from live discovery rather than
 * the packaged snapshot. `loadDevinModelCatalogue` falls back to that
 * snapshot when the CLI cannot be asked, so the form always renders.
 */
export async function getConfigSchema(options?: {
  command?: string;
  run?: ModelsListRunner;
}): Promise<AdapterConfigSchema> {
  const models = await loadDevinModelCatalogue(options);
  return {
    fields: [
      {
        key: "cwd",
        label: "Working directory",
        type: "text",
        // Not required: Paperclip resolves an execution workspace per run and
        // overrides this, so an agent that relies on the workspace leaves it
        // empty. `acpx_local` declares it the same way.
        hint: "Absolute fallback directory. Paperclip execution workspaces override this at runtime.",
      },
      {
        key: "model",
        label: "Model",
        type: "combobox",
        // Grouped by family: hundreds of flat entries would be unusable.
        options: models.map((model) => ({
          value: model.id,
          label: model.label,
          group: model.family,
        })),
        hint: "Leave empty to use the account default. A resumed session keeps the model it was created with.",
      },
      {
        key: "agentType",
        label: "Agent type",
        type: "select",
        default: "",
        options: [
          { value: "", label: "Default agent" },
          { value: "review", label: "Code review (read-only + shell)" },
          { value: "summarizer", label: "Summarizer (no tools)" },
        ],
        hint: "Devin's specialised ACP agents.",
      },
      {
        key: "permissionMode",
        label: "Permission handling",
        type: "select",
        default: DEFAULT_PERMISSION_MODE,
        options: [
          { value: "approve-all", label: "Approve all requests" },
          { value: "approve-reads", label: "Approve reads, deny writes" },
          { value: "deny-all", label: "Deny all requests" },
        ],
        hint: "How ACPX answers Devin's permission requests. Devin's own --permission-mode does not apply in ACP mode.",
      },
      {
        key: "command",
        label: "Devin binary",
        type: "text",
        default: DEFAULT_COMMAND,
        hint: "Path to the Devin CLI if it is not on the server's PATH.",
      },
      {
        key: "timeoutSec",
        label: "Timeout (seconds)",
        type: "number",
        default: DEFAULT_TIMEOUT_SEC,
        hint: "0 means no adapter-side timeout.",
      },
      {
        key: "nonInteractivePermissions",
        label: "Unattended permission requests",
        type: "select",
        default: "deny",
        options: [
          { value: "deny", label: "Deny and continue" },
          { value: "fail", label: "Fail the run" },
        ],
        // Agents run unattended here, so a permission prompt has nobody to answer
        // it. "deny" lets the agent route around it; "fail" surfaces it instead of
        // letting the run quietly do less than asked.
        hint: "What to do when the agent asks for a permission no one can grant.",
      },
      {
        key: "fallbackModels",
        label: "Fallback models",
        type: "text",
        // Only used when the primary model's provider is unreachable — never for
        // a task the agent genuinely failed, which would bill the same failure
        // twice. Comma-separated; pick models on different providers.
        hint: "Comma-separated. Tried in order when the primary model's provider is down.",
      },
      {
        key: "acuRateUsd",
        label: "USD per ACU",
        type: "number",
        // Left empty on purpose: with no rate the run cost is reported in ACUs,
        // so a budget of 500 means 500 ACUs. Enter your contracted rate only if
        // you would rather budget in money.
        hint: "Leave empty to budget in ACUs. Set your contracted rate to budget in dollars instead.",
      },
      {
        key: "warmHandleIdleMs",
        label: "Keep process warm (ms)",
        type: "number",
        default: 0,
        hint: "Keeps the ACP process alive between runs. 0 shuts it down and keeps only the session state.",
      },
      {
        key: "promptTemplate",
        label: "Prompt template",
        type: "textarea",
        hint: "Supports {{variable}} substitution.",
      },
    ],
  };
}
