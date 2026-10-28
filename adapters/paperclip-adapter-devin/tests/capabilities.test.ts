import { describe, expect, it } from "vitest";

import { createServerAdapter } from "../src/server/index.js";

/**
 * These are declarations, not logic — but each one is a capability Paperclip
 * gates on, and every one of them failed silently when it was missing: the run
 * succeeded and only the side effect disappeared.
 */
describe("declared capabilities", () => {
  const adapter = createServerAdapter();

  /**
   * `heartbeat.js` mints the token with
   * `adapter.supportsLocalAgentJwt ? createLocalAgentJwt(...) : null`, so
   * without this every control-plane write from the agent gets a 401 while the
   * run itself reports success.
   */
  it("accepts a per-run agent token, so PAPERCLIP_API_KEY is injected", () => {
    expect(adapter.supportsLocalAgentJwt).toBe(true);
  });

  it("offers the Instructions tab", () => {
    expect(adapter.supportsInstructionsBundle).toBe(true);
    expect(adapter.instructionsPathKey).toBe("instructionsFilePath");
  });

  it("offers the Skills tab", () => {
    expect(Boolean(adapter.listSkills || adapter.syncSkills)).toBe(true);
  });

  it("declares itself an ACP harness", () => {
    expect(adapter.acp?.skillsMode).toBe("ephemeral");
  });
});
