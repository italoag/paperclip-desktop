/**
 * Resolves the Devin binary to an absolute path.
 *
 * # Why a bare command name is not enough
 *
 * ACPX launches the agent through a generated wrapper script, and it does not
 * give that script the server's `PATH`. A wrapper ending in `exec devin acp`
 * therefore fails with:
 *
 * ```
 * custom-<hash>.sh: line 14: exec: devin: not found
 * ```
 *
 * even though the server process itself can find `devin` perfectly well — which
 * is exactly why the environment test passes while every run fails. Devin
 * installs to `~/.local/bin`, which is on a login shell's `PATH` but not on the
 * minimal one a GUI app inherits.
 *
 * Resolving here, inside the server process, and baking the absolute path into
 * the wrapper removes the dependency on whatever `PATH` the child ends up with.
 */

import { accessSync, constants, statSync } from "node:fs";
import path from "node:path";

/** Searched when the process `PATH` does not already cover them. */
const EXTRA_LOOKUP_DIRS = [
  "~/.local/bin",
  "/opt/homebrew/bin",
  "/usr/local/bin",
];

function expandHome(dir: string, home: string | undefined): string {
  return dir.startsWith("~/") && home ? path.join(home, dir.slice(2)) : dir;
}

function isExecutableFile(candidate: string): boolean {
  try {
    if (!statSync(candidate).isFile()) return false;
    accessSync(candidate, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export interface ResolveOptions {
  pathEnv?: string | undefined;
  home?: string | undefined;
}

/**
 * Returns an absolute path to `command`, or `command` unchanged when it cannot
 * be found.
 *
 * Returning the input on failure is deliberate: the run then fails with the
 * shell's own "not found" message naming the command, which is clearer than any
 * error this function could invent, and the environment test already reports a
 * missing binary properly.
 */
export function resolveCommandPath(command: string, options: ResolveOptions = {}): string {
  const trimmed = command.trim();
  if (!trimmed) return command;

  // Anything with a separator is already a path; only make it absolute.
  if (trimmed.includes(path.sep)) {
    return path.isAbsolute(trimmed) ? trimmed : path.resolve(trimmed);
  }

  const pathEnv = options.pathEnv ?? process.env.PATH ?? "";
  const home = options.home ?? process.env.HOME;

  const searched = [
    ...pathEnv.split(path.delimiter).filter(Boolean),
    ...EXTRA_LOOKUP_DIRS.map((dir) => expandHome(dir, home)),
  ];

  for (const dir of searched) {
    const candidate = path.join(dir, trimmed);
    if (isExecutableFile(candidate)) return candidate;
  }

  return command;
}
