/**
 * Devin config -> ACPX config translation.
 *
 * This adapter does not talk to Devin itself. Devin ships an Agent Client
 * Protocol server (`devin acp`), and Paperclip already bundles `acpx_local`,
 * which drives any ACP server and turns its stream into the structured events
 * the run viewer renders. So the whole job here is to present a Devin-shaped
 * configuration to the operator and translate it into what `acpx_local` expects.
 *
 * Keeping the translation in one pure function makes it testable without
 * spawning anything, which is the only reason the rest of the adapter can stay
 * as thin as it is.
 */

/** Devin's ACP server exposes these specialised agents; empty means the default. */
export type DevinAgentType = "" | "summarizer" | "review";

/**
 * How ACPX answers permission requests coming from the agent.
 *
 * Note this is the *ACPX* layer, not Devin's own `--permission-mode`: in ACP
 * mode Devin delegates approvals to the client over the protocol, and
 * `devin acp` accepts no permission flag of its own.
 */
export type AcpxPermissionMode = "approve-all" | "approve-reads" | "deny-all";

export interface DevinAdapterConfig {
  cwd?: string;
  command?: string;
  model?: string;
  agentType?: DevinAgentType;
  permissionMode?: AcpxPermissionMode;
  nonInteractivePermissions?: "deny" | "fail";
  timeoutSec?: number;
  warmHandleIdleMs?: number;
  /**
   * Models to try, in order, when the primary fails because its provider is
   * unavailable. See `server/retry.ts`.
   */
  fallbackModels?: string[];
  /**
   * USD per ACU. Unset means run cost is reported in ACUs directly, which
   * denominates the Paperclip budget in ACUs rather than money.
   */
  acuRateUsd?: number;
  promptTemplate?: string;
  bootstrapPromptTemplate?: string;
  instructionsFilePath?: string;
  stateDir?: string;
  env?: Record<string, unknown>;
}

export const DEFAULT_COMMAND = "devin";
export const DEFAULT_PERMISSION_MODE: AcpxPermissionMode = "approve-all";
/** No adapter-side timeout by default, matching `acpx_local`. */
/**
 * Two hours, as a safety net rather than a work limit.
 *
 * The engine's own default is `0` — no bound at all — and this adapter shipped
 * with the same value. On an unattended agent that means a wedged run holds its
 * lease indefinitely; the run logs show prompts sitting for 90 minutes with no
 * way to tell a working agent from a stuck one.
 *
 * Two hours is deliberately far above observed real work (the longest genuine
 * prompt seen was 90 minutes) so it never cuts a productive run short. It only
 * bounds the pathological case. Set `timeoutSec` explicitly to tighten it, or
 * `0` to restore unbounded runs.
 */
export const DEFAULT_TIMEOUT_SEC = 7200;

/**
 * Quotes a token for `sh -c`.
 *
 * `acpx_local` interpolates the command into `exec <command> "$@"`, so anything
 * unquoted is word-split by the shell. A cwd or binary path containing a space
 * would otherwise silently become two arguments.
 */
export function shellQuote(value: string): string {
  if (value.length > 0 && /^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) return value;
  return `'${value.replaceAll("'", `'\\''`)}'`;
}

function trimmedString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/**
 * Builds the shell command ACPX launches to obtain an ACP server.
 *
 * The model is pinned here rather than through ACPX's ACP session config on
 * purpose: `devin acp --model` is a documented flag, whereas passing a model
 * through session config depends on agent-side support we cannot verify for
 * every Devin version. A CLI flag either works or fails loudly at launch.
 */
export function buildAgentCommand(
  config: DevinAdapterConfig,
  resolvedCommand?: string,
): string {
  const command =
    trimmedString(resolvedCommand) || trimmedString(config.command) || DEFAULT_COMMAND;
  const parts = [shellQuote(command), "acp"];

  const model = trimmedString(config.model);
  if (model) parts.push("--model", shellQuote(model));

  const agentType = trimmedString(config.agentType);
  if (agentType) parts.push("--agent-type", shellQuote(agentType));

  return parts.join(" ");
}

/**
 * Translates the Devin-facing config into the `acpx_local` config blob.
 *
 * Unknown keys are passed through untouched so operators can still reach any
 * `acpx_local` option this adapter does not model explicitly — with the
 * exception of `agent`/`agentCommand`, which are always ours: letting those
 * through would mean a Devin agent silently running Claude.
 */
export interface AcpxOverrides {
  /** Absolute path to the Devin binary; see `server/resolve-command.ts`. */
  resolvedCommand?: string | undefined;
  /** Space-free state directory; see `server/state-dir.ts`. */
  stateDir?: string | undefined;
  /** Environment for the agent process, including the managed XDG config home. */
  env?: Record<string, unknown> | undefined;
}

export function toAcpxConfig(
  config: DevinAdapterConfig,
  overrides: AcpxOverrides = {},
): Record<string, unknown> {
  const {
    command: _command,
    model: _model,
    agentType: _agentType,
    ...passthrough
  } = config as DevinAdapterConfig & Record<string, unknown>;

  delete (passthrough as Record<string, unknown>).agent;
  delete (passthrough as Record<string, unknown>).agentCommand;

  return {
    ...passthrough,
    agent: "custom",
    agentCommand: buildAgentCommand(config, overrides.resolvedCommand),
    mode: "persistent",
    permissionMode: trimmedString(config.permissionMode) || DEFAULT_PERMISSION_MODE,
    timeoutSec:
      typeof config.timeoutSec === "number" && Number.isFinite(config.timeoutSec)
        ? config.timeoutSec
        : DEFAULT_TIMEOUT_SEC,
    ...(overrides.stateDir ? { stateDir: overrides.stateDir } : {}),
    ...(overrides.env ? { env: overrides.env } : {}),
  };
}
