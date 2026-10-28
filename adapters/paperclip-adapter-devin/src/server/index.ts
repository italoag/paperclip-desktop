import type { ServerAdapterModule } from "@paperclipai/adapter-utils";
import { sessionCodec } from "@paperclipai/adapter-utils/acpx-engine/session-codec";

import { agentConfigurationDoc, models, type } from "../index.js";
import { execute } from "./execute.js";
import { listDevinModels, refreshDevinModels } from "./models.js";
import { createSkillHandlers } from "./skills.js";
import { effectiveStateDir } from "./state-dir.js";
import { getConfigSchema } from "./schema.js";
import { testEnvironment } from "./test.js";

/**
 * Entry point the Paperclip plugin loader looks for.
 *
 * The session codec is ACPX's: session state is produced and consumed entirely
 * by ACPX, so re-encoding it here could only corrupt it.
 */
export function createServerAdapter(): ServerAdapterModule {
  const skills = createSkillHandlers((ctx) =>
    effectiveStateDir({
      configured: (ctx.config as { stateDir?: string }).stateDir,
      companyId: ctx.companyId,
      agentId: ctx.agentId,
      paperclipHome: process.env.PAPERCLIP_HOME,
      instanceId: process.env.PAPERCLIP_INSTANCE_ID,
      home: process.env.HOME ?? "",
    }),
  );

  return {
    type,
    execute,
    testEnvironment,
    sessionCodec,
    // Static catalogue is the registry's fallback; `listModels`/`refreshModels`
    // answer with the subscription the host's `devin` is logged into.
    models,
    listModels: listDevinModels,
    refreshModels: refreshDevinModels,
    agentConfigurationDoc,
    getConfigSchema,

    // Drives the board's "Instructions" tab. `acpx_local` already threads
    // `instructionsFilePath` into the prompt it builds, so the bundle Paperclip
    // writes is picked up without any further work here.
    supportsInstructionsBundle: true,
    instructionsPathKey: "instructionsFilePath",

    // Drives the board's "Skills" tab. Paperclip materialises the selected
    // skills and passes them through `paperclipRuntimeSkills`; `execute` copies
    // them into the agent's Devin skills directory before each run.
    listSkills: skills.listSkills,
    syncSkills: skills.syncSkills,
    requiresMaterializedRuntimeSkills: true,

    // Mints a per-run token and injects it as `PAPERCLIP_API_KEY`, which is what
    // lets the agent write back to the control plane — hiring, issues, comments.
    // The gate is literally `adapter.supportsLocalAgentJwt ? createLocalAgentJwt(...) : null`,
    // so leaving it out means every such write returns
    // `401 Agent authentication required`, with the run itself still succeeding.
    //
    // True because Devin runs locally, under the same user as the control plane —
    // the same reason `opencode_local` and `pi_local` set it, and the reason the
    // cloud and gateway adapters do not.
    supportsLocalAgentJwt: true,

    // Declares Devin as its own ACP harness. `acpx_local` was retired precisely
    // because ACP is a capability of a harness, not an adapter of its own — so
    // this adapter drives the shared engine directly instead of borrowing
    // another harness's executor. The API surfaces this as `supportsAcp`.
    acp: {
      agentId: "custom",
      skillsMode: "ephemeral",
      prerequisites: { nodeRange: ">=22.15.0" },
    },
  };
}
