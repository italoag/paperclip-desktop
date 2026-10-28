/**
 * Environment diagnostics for the board's "Test environment" button.
 *
 * The point of these checks is to turn the failures an operator would otherwise
 * only discover as a red run — a missing binary, a logged-out account, a cwd
 * that does not exist — into something readable before the agent is hired.
 *
 * Findings are returned, never thrown. Only conditions that genuinely prevent a
 * run are `error`; everything an operator should know but that still runs is
 * `warn`.
 */

import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import type {
  AdapterEnvironmentCheck,
  AdapterEnvironmentTestContext,
  AdapterEnvironmentTestResult,
} from "@paperclipai/adapter-utils";

import { DEFAULT_COMMAND, type DevinAdapterConfig } from "../config.js";
import { resolveCommandPath } from "./resolve-command.js";
import { readSessionAcu, sessionDatabasePaths } from "./acu.js";
import { stateDirHasSpace } from "./state-dir.js";

const execFileAsync = promisify(execFile);

/** ACPX requires this; Paperclip itself only requires Node 20. */
const MINIMUM_NODE_MAJOR = 22;
const MINIMUM_NODE_MINOR = 12;

const PROBE_TIMEOUT_MS = 15_000;

function firstLine(value: string): string {
  return value.split("\n")[0]?.trim() ?? "";
}

async function run(
  command: string,
  args: string[],
): Promise<{ ok: boolean; stdout: string; stderr: string }> {
  try {
    const { stdout, stderr } = await execFileAsync(command, args, {
      timeout: PROBE_TIMEOUT_MS,
      encoding: "utf8",
    });
    return { ok: true, stdout, stderr };
  } catch (error) {
    const err = error as { stdout?: string; stderr?: string; message?: string };
    return {
      ok: false,
      stdout: err.stdout ?? "",
      stderr: err.stderr ?? err.message ?? String(error),
    };
  }
}

function checkNodeVersion(): AdapterEnvironmentCheck {
  const [major = 0, minor = 0] = process.versions.node.split(".").map(Number);
  const satisfies =
    major > MINIMUM_NODE_MAJOR ||
    (major === MINIMUM_NODE_MAJOR && minor >= MINIMUM_NODE_MINOR);

  return satisfies
    ? {
        code: "node_version",
        level: "info",
        message: `Node ${process.versions.node} satisfies the ACPX requirement`,
      }
    : {
        code: "node_version",
        level: "error",
        message: `Node ${process.versions.node} is too old for ACPX`,
        hint: `ACPX requires Node >= ${MINIMUM_NODE_MAJOR}.${MINIMUM_NODE_MINOR}.0. Paperclip must run on a newer Node for this adapter to work.`,
      };
}

async function checkBinary(command: string): Promise<AdapterEnvironmentCheck[]> {
  // Probing the *resolved* path, not the bare name, so the test exercises what a
  // run will actually execute. Probing by name passes off the server's PATH,
  // which the ACPX wrapper does not inherit — the test would go green while
  // every run died with "devin: not found".
  const resolved = resolveCommandPath(command);
  const version = await run(resolved, ["version"]);
  if (!version.ok) {
    return [
      {
        code: "devin_missing",
        level: "error",
        message: `Could not run "${resolved} version"`,
        detail: version.stderr.trim().slice(0, 400) || null,
        hint: `Install the Devin CLI and make sure "${command}" is on the PATH of the user running the Paperclip server, or set an absolute path in the "command" field.`,
      },
    ];
  }

  const checks: AdapterEnvironmentCheck[] = [
    {
      code: "devin_detected",
      level: "info",
      message: `Devin CLI detected: ${firstLine(version.stdout)}`,
    },
    {
      code: "devin_path",
      // Shown because the run uses this exact path; if it ever reads as a bare
      // name, resolution failed and the run will not find the binary.
      level: path.isAbsolute(resolved) ? "info" : "warn",
      message: path.isAbsolute(resolved)
        ? `Resolved binary: ${resolved}`
        : `Could not resolve "${command}" to an absolute path`,
      hint: path.isAbsolute(resolved)
        ? undefined
        : "ACPX runs the agent from a wrapper script that does not inherit the server's PATH. Set an absolute path in the \"command\" field.",
    },
  ];

  // A binary that cannot run as an ACP server is unusable here no matter how
  // healthy it looks otherwise.
  const acp = await run(resolved, ["acp", "--help"]);
  checks.push(
    acp.ok
      ? { code: "devin_acp", level: "info", message: "`devin acp` is available" }
      : {
          code: "devin_acp",
          level: "error",
          message: "This Devin build does not support `devin acp`",
          detail: acp.stderr.trim().slice(0, 400) || null,
          hint: "Update the Devin CLI: the adapter drives Devin through its Agent Client Protocol server.",
        },
  );

  return checks;
}

async function checkAuth(command: string): Promise<AdapterEnvironmentCheck> {
  const status = await run(command, ["auth", "status"]);
  const output = `${status.stdout}${status.stderr}`.trim();

  if (status.ok && /logged in/i.test(output)) {
    return {
      code: "devin_auth",
      level: "info",
      message: firstLine(output) || "Devin CLI is authenticated",
    };
  }

  return {
    code: "devin_auth",
    level: "error",
    message: "Devin CLI is not authenticated",
    detail: output.slice(0, 400) || null,
    hint: `Run "${command} auth login" as the user that runs the Paperclip server. Devin uses its own credentials file, not a Paperclip-managed key.`,
  };
}

/**
 * Validates the working directory *only when one is configured*.
 *
 * `cwd` is a fallback, not a requirement: Paperclip resolves an execution
 * workspace for each run and overrides it, which is why `acpx_local` leaves the
 * field optional and does not check it at all. Treating an empty value as an
 * error — as an earlier version of this did — failed the environment test for a
 * perfectly valid agent that simply relies on the workspace.
 */
export async function checkCwd(cwd: string): Promise<AdapterEnvironmentCheck> {
  if (!cwd) {
    return {
      code: "cwd_from_workspace",
      level: "info",
      message: "No fallback working directory set; Paperclip's execution workspace will supply one",
    };
  }

  if (!path.isAbsolute(cwd)) {
    return {
      code: "cwd_relative",
      level: "error",
      message: `Working directory must be absolute: "${cwd}"`,
      hint: "A relative path resolves against the Paperclip server's own cwd, which is not what you want.",
    };
  }

  try {
    const stat = await fs.stat(cwd);
    if (!stat.isDirectory()) {
      return {
        code: "cwd_not_a_directory",
        level: "error",
        message: `Working directory is not a directory: "${cwd}"`,
      };
    }
  } catch {
    return {
      code: "cwd_absent",
      level: "error",
      message: `Working directory does not exist: "${cwd}"`,
      hint: "Create it, or point the agent at an existing execution workspace.",
    };
  }

  return { code: "cwd_ok", level: "info", message: `Working directory: ${cwd}` };
}

/**
 * Flags a state directory that ACPX cannot spawn from.
 *
 * ACPX splits the wrapper command on whitespace, so a state directory
 * containing a space makes every run die with a misleading
 * `spawn <truncated-path> EACCES`. `resolveStateDir` avoids this automatically;
 * this reports the cases it cannot (an operator-chosen path with a space, or a
 * home directory that has one).
 */
function stateDirCheck(
  config: DevinAdapterConfig,
  companyId: string,
): AdapterEnvironmentCheck {
  const options = {
    configured: config.stateDir,
    companyId,
    // Only the directory shape matters here, not the specific agent.
    agentId: "probe",
    paperclipHome: process.env.PAPERCLIP_HOME,
    instanceId: process.env.PAPERCLIP_INSTANCE_ID,
    home: process.env.HOME ?? "",
  };

  return stateDirHasSpace(options)
    ? {
        code: "state_dir_space",
        level: "error",
        message: "ACPX state directory contains a space",
        detail:
          "ACPX tokenises the wrapper command on whitespace, so runs fail with a truncated \"spawn ... EACCES\".",
        hint: 'Point the "stateDir" field at a path with no spaces.',
      }
    : {
        code: "state_dir_ok",
        level: "info",
        message: "ACPX state directory is free of spaces",
      };
}

/**
 * Reports whether ACU spend can be read.
 *
 * Cost reporting depends on Devin's own session database, which is not a
 * documented interface. If a Devin release moves or reshapes it, runs keep
 * working and only cost goes quiet — so this check is the place that says so
 * out loud, rather than leaving someone to wonder why every run shows $0.00.
 */
function acuAccountingCheck(config: DevinAdapterConfig): AdapterEnvironmentCheck {
  const databases = sessionDatabasePaths().filter((file) => existsSync(file));

  if (databases.length === 0) {
    return {
      code: "acu_source_missing",
      level: "warn",
      message: "Devin's session database was not found, so ACU spend cannot be reported",
      detail: `Looked in: ${sessionDatabasePaths().join(", ")}`,
      hint: "Run the Devin CLI once to create it. Runs still work; only cost reporting is affected.",
    };
  }

  // Probing a session that cannot exist verifies the file opens and the schema
  // still matches, without depending on any particular run being present.
  if (readSessionAcu("paperclip-probe-no-such-session") !== null) {
    return {
      code: "acu_source_unexpected",
      level: "warn",
      message: "Devin's session database returned a value for a session that does not exist",
      hint: "Cost figures may be wrong. Report this — the schema assumption no longer holds.",
    };
  }

  const rate = config.acuRateUsd;
  const priced = typeof rate === "number" && Number.isFinite(rate) && rate > 0;

  return {
    code: "acu_accounting",
    level: "info",
    message: priced
      ? `ACU spend is reported, converted at ${rate} USD per ACU`
      : "ACU spend is reported; budgets are denominated in ACUs",
    detail: priced
      ? undefined
      : 'Cost is reported as ACUs rather than dollars, so a budget of 500 means 500 ACUs. Set "acuRateUsd" to budget in money instead.',
  };
}

export async function testEnvironment(
  ctx: AdapterEnvironmentTestContext,
): Promise<AdapterEnvironmentTestResult> {
  const config = (ctx.config ?? {}) as DevinAdapterConfig;
  const command = (config.command ?? "").trim() || DEFAULT_COMMAND;

  const checks: AdapterEnvironmentCheck[] = [checkNodeVersion()];

  const binaryChecks = await checkBinary(command);
  checks.push(...binaryChecks);

  // Only worth asking about credentials once we know the binary runs at all.
  const binaryUsable = !binaryChecks.some((check) => check.level === "error");
  if (binaryUsable) checks.push(await checkAuth(resolveCommandPath(command)));

  checks.push(await checkCwd((config.cwd ?? "").trim()));
  checks.push(stateDirCheck(config, ctx.companyId));
  checks.push(acuAccountingCheck(config));

  const status = checks.some((check) => check.level === "error")
    ? "fail"
    : checks.some((check) => check.level === "warn")
      ? "warn"
      : "pass";

  return {
    adapterType: ctx.adapterType,
    status,
    checks,
    testedAt: new Date().toISOString(),
  };
}
