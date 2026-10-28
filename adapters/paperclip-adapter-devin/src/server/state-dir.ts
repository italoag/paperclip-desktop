/**
 * Keeps ACPX's state directory out of paths containing spaces.
 *
 * # The bug this exists for
 *
 * ACPX builds the agent's launch plan by running the command through
 * `splitCommandLine`, which tokenises on whitespace. The command it launches is
 * a wrapper script it generates under its state directory, and `acpx_local`
 * defaults that to:
 *
 * ```
 * $PAPERCLIP_HOME/instances/<id>/companies/<c>/acpx-local/agents/<a>
 * ```
 *
 * On macOS `PAPERCLIP_HOME` defaults to `~/Library/Application Support/Paperclip`.
 * The space in "Application Support" splits the wrapper path in two, so ACPX
 * spawns `/Users/<you>/Library/Application` — which happens to exist as an empty
 * directory on some machines, turning a clear `ENOENT` into a baffling:
 *
 * ```
 * spawn /Users/<you>/Library/Application EACCES
 * ```
 *
 * The real fix belongs in ACPX (quote the command, or spawn the wrapper without
 * re-parsing it). Until then this relocates the state directory somewhere
 * space-free, which sidesteps the tokenisation entirely.
 *
 * It deviates only when it has to: if the default path is already space-free,
 * this returns `undefined` and `acpx_local` keeps its own default, so state
 * stays inside Paperclip's data directory wherever that is possible.
 */

import path from "node:path";

export interface StateDirOptions {
  /** `stateDir` from the adapter config, if the operator set one. */
  configured?: string | undefined;
  companyId: string;
  agentId: string;
  /** `PAPERCLIP_HOME`; defaults to `<home>/.paperclip` exactly as acpx_local does. */
  paperclipHome?: string | undefined;
  instanceId?: string | undefined;
  home: string;
}

/** Mirrors the engine's default state directory layout. */
export function acpxDefaultStateDir(options: StateDirOptions): string {
  const home = options.paperclipHome?.trim() || path.join(options.home, ".paperclip");
  const instanceId = options.instanceId?.trim() || "default";
  return path.join(
    home,
    "instances",
    instanceId,
    "companies",
    options.companyId,
    "acpx-local",
    "agents",
    options.agentId,
  );
}

/**
 * Returns a state directory to force, or `undefined` to accept acpx_local's.
 *
 * An explicitly configured `stateDir` is always honoured, even with a space in
 * it: the operator asked for it, and silently moving their state would be worse
 * than the failure — which the environment test flags anyway.
 */
export function resolveStateDir(options: StateDirOptions): string | undefined {
  if (options.configured?.trim()) return undefined;

  if (!acpxDefaultStateDir(options).includes(" ")) return undefined;

  // Nothing to gain if the home directory itself has a space; leave the default
  // so the failure stays in one recognisable place.
  if (options.home.includes(" ")) return undefined;

  return path.join(
    options.home,
    ".paperclip-adapter-devin",
    "instances",
    options.instanceId?.trim() || "default",
    "companies",
    options.companyId,
    "agents",
    options.agentId,
  );
}

/** Whether a run will hit the ACPX whitespace bug. */
export function stateDirHasSpace(options: StateDirOptions): boolean {
  const effective =
    options.configured?.trim() || resolveStateDir(options) || acpxDefaultStateDir(options);
  return effective.includes(" ");
}

/** The directory ACPX will actually use, whether relocated or not. */
export function effectiveStateDir(options: StateDirOptions): string {
  return (
    options.configured?.trim() || resolveStateDir(options) || acpxDefaultStateDir(options)
  );
}
