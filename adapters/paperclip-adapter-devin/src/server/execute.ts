/**
 * Execution: translate the config, then hand the run to ACPX.
 *
 * Everything hard — the ACP handshake, session reuse, permission prompts,
 * streaming events, usage accounting — is already solved by `acpx_local`, which
 * ships with Paperclip. Reimplementing it here would mean maintaining a second
 * ACP client that drifts from the one the built-in adapters use.
 */

import type {
  AdapterExecutionContext,
  AdapterExecutionResult,
} from "@paperclipai/adapter-utils";
import { createAcpxEngineExecutor } from "@paperclipai/adapter-utils/acpx-engine/execute";

import { DEFAULT_COMMAND, toAcpxConfig, type DevinAdapterConfig } from "../config.js";
import { acuSpentThisRun, acuToLedgerCost, readSessionAcu } from "./acu.js";
import { resolveCommandPath } from "./resolve-command.js";
import { isTransientModelFailure, modelAttemptOrder } from "./retry.js";
import { materializeSkills, devinConfigHome } from "./skills.js";
import { effectiveStateDir, resolveStateDir } from "./state-dir.js";

/**
 * The shared ACP engine, created once.
 *
 * `acpx_local` was retired: ACP is a capability of each harness adapter now, not
 * an adapter of its own. Devin is its own harness, so it drives the engine
 * directly rather than borrowing another harness's executor.
 */
const runAcpxEngine = createAcpxEngineExecutor();

export async function execute(
  ctx: AdapterExecutionContext,
): Promise<AdapterExecutionResult> {
  const devinConfig = (ctx.config ?? {}) as DevinAdapterConfig;

  // Resolved here, in the server process, because ACPX runs the agent from a
  // generated wrapper script that does not inherit the server's PATH. See
  // `resolve-command.ts`.
  const resolvedCommand = resolveCommandPath(
    (devinConfig.command ?? "").trim() || DEFAULT_COMMAND,
  );

  // ACPX tokenises the wrapper path on whitespace, and Paperclip's default home
  // on macOS contains one. See `state-dir.ts`.
  const stateDirOptions = {
    configured: devinConfig.stateDir,
    companyId: ctx.agent.companyId,
    agentId: ctx.agent.id,
    paperclipHome: process.env.PAPERCLIP_HOME,
    instanceId: process.env.PAPERCLIP_INSTANCE_ID,
    home: process.env.HOME ?? "",
  };

  // Skills are placed on disk before the agent starts: Devin reads its skills
  // directory at session creation, so anything written afterwards is invisible
  // until the next run.
  const stateDir = resolveStateDir(stateDirOptions);
  const effective = effectiveStateDir(stateDirOptions);
  const skills = await materializeSkills(ctx.config ?? {}, effective);
  for (const warning of skills.warnings) {
    await ctx.onLog("stderr", `[devin_local] ${warning}\n`);
  }

  // Read before the run: the stored ACU total is cumulative per session, so
  // this run's share is the delta. Only meaningful when resuming a session —
  // a new one starts from zero.
  const previousSessionId = ctx.runtime.sessionDisplayId;
  const acuBefore = readSessionAcu(previousSessionId);

  // Each attempt is a different model. Devin already retries a failing turn
  // three times internally, so reaching here means the provider behind the model
  // is down and repeating it would fail again — only a different provider has a
  // reason to succeed. See `retry.ts` for why the match is narrow.
  const attempts = modelAttemptOrder(devinConfig.model, devinConfig.fallbackModels);
  let result: AdapterExecutionResult | undefined;
  let servedBy = devinConfig.model;

  for (const [index, model] of attempts.entries()) {
    try {
      result = await runAcpxEngine({
        ...ctx,
        config: toAcpxConfig(
          { ...devinConfig, model },
          {
            resolvedCommand,
            stateDir,
            // Points Devin at the agent's own skills directory. `devin skills paths`
            // shows XDG_CONFIG_HOME driving the user-level skill location.
            env: { ...(devinConfig.env ?? {}), XDG_CONFIG_HOME: devinConfigHome(effective) },
          },
        ),
      });
    } catch (error) {
      // A throw leaves no result to carry `costUsd`, so ACUs already burned cannot
      // reach the ledger. They are at least written to the run log rather than
      // vanishing — silent under-reporting of real spend is the worse failure.
      //
      // Deliberately not converted into a failed result: that would hand Paperclip
      // an "adapter ran and failed" where an infrastructure error actually
      // occurred, and cost accuracy is not worth blurring that distinction.
      const orphaned = readSessionAcu(previousSessionId);
      if (orphaned !== null && acuBefore !== null && orphaned > acuBefore) {
        await ctx.onLog(
          "stderr",
          `[devin_local] run failed after spending ${(orphaned - acuBefore).toFixed(6)} ACU; ` +
            "this is not recorded in the budget ledger\n",
        );
      }
      throw error;
    }

    servedBy = model;
    const nextModel = attempts[index + 1];
    if (!nextModel || !isTransientModelFailure(result)) break;

    await ctx.onLog(
      "stderr",
      `[devin_local] model "${model ?? "default"}" is unavailable (${result.errorMessage ?? "no detail"}); ` +
        `retrying with "${nextModel}"\n`,
    );
  }

  if (!result) throw new Error("[devin_local] no execution attempt produced a result");

  // Cognition bills in ACUs, and the ACP stream carries none of that, so the
  // figure comes from Devin's own session store. See `acu.ts`.
  const acus = acuSpentThisRun({
    previousSessionId,
    sessionId: result.sessionDisplayId,
    before: acuBefore,
  });

  // Report the provider as Devin. ACPX reports the ACP agent it launched, which
  // for a custom command is just "custom" — unhelpful in run history, and it
  // would make cost attribution across agents ambiguous.
  return {
    ...result,
    provider: "devin",
    // The model that actually served, which is not the configured primary
    // when a fallback took over.
    model: result.model ?? servedBy ?? null,
    // Left untouched when the lookup failed: reporting 0 would tell the ledger
    // the run was free, which is worse than reporting nothing.
    ...(acus === null
      ? {}
      : {
          costUsd: acuToLedgerCost(acus, devinConfig.acuRateUsd),
          // Describes how Cognition bills, whichever unit we denominate in.
          billingType: "credits" as const,
        }),
  };
}
