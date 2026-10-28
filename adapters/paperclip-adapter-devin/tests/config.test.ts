import { describe, expect, it } from "vitest";

import {
  buildAgentCommand,
  shellQuote,
  toAcpxConfig,
  type DevinAdapterConfig,
} from "../src/config.js";
import { parseStdoutLine } from "../src/ui-parser.js";

describe("buildAgentCommand", () => {
  it("launches devin's ACP server by default", () => {
    expect(buildAgentCommand({})).toBe("devin acp");
  });

  it("pins the model and agent type when configured", () => {
    expect(
      buildAgentCommand({ model: "claude-sonnet-5-high", agentType: "review" }),
    ).toBe("devin acp --model claude-sonnet-5-high --agent-type review");
  });

  it("omits an empty agent type rather than passing a blank flag", () => {
    expect(buildAgentCommand({ agentType: "" })).toBe("devin acp");
  });

  /**
   * ACPX interpolates this string into `exec <command> "$@"`, so an unquoted
   * space would split the binary path into two arguments.
   */
  it("quotes a binary path containing spaces", () => {
    expect(buildAgentCommand({ command: "/opt/My Tools/devin" })).toBe(
      "'/opt/My Tools/devin' acp",
    );
  });

  it("neutralises a quote in a config value", () => {
    expect(shellQuote("it's")).toBe(`'it'\\''s'`);
  });
});

describe("toAcpxConfig", () => {
  it("always targets a custom ACP command", () => {
    const config = toAcpxConfig({ cwd: "/w" });
    expect(config.agent).toBe("custom");
    expect(config.agentCommand).toBe("devin acp");
  });

  /**
   * Without this, an operator who pasted an acpx_local config into a Devin
   * agent would get a Devin-labelled agent quietly running Claude.
   */
  it("refuses to let a config override the agent away from Devin", () => {
    const hostile = {
      cwd: "/w",
      agent: "claude",
      agentCommand: "claude-agent-acp",
    } as unknown as DevinAdapterConfig;

    const config = toAcpxConfig(hostile);
    expect(config.agent).toBe("custom");
    expect(config.agentCommand).toBe("devin acp");
  });

  /**
   * `timeoutSec` defaults to 7200, not 0. The engine's own default is unbounded,
   * which on an unattended agent lets a wedged run hold its lease forever; the
   * bound is a safety net well above real work, not a work limit.
   */
  it("applies the documented defaults", () => {
    const config = toAcpxConfig({});
    expect(config.permissionMode).toBe("approve-all");
    expect(config.timeoutSec).toBe(7200);
    expect(config.mode).toBe("persistent");
  });

  it("keeps explicit values, including a zero timeout", () => {
    const config = toAcpxConfig({ permissionMode: "deny-all", timeoutSec: 0 });
    expect(config.permissionMode).toBe("deny-all");
    expect(config.timeoutSec).toBe(0);
  });

  it("passes unmodelled acpx options straight through", () => {
    const config = toAcpxConfig({
      cwd: "/w",
      stateDir: "/state",
      env: { FOO: "bar" },
    });
    expect(config.cwd).toBe("/w");
    expect(config.stateDir).toBe("/state");
    expect(config.env).toEqual({ FOO: "bar" });
  });

  it("does not leak devin-only keys into the acpx config", () => {
    const config = toAcpxConfig({ command: "devin", model: "adaptive", agentType: "review" });
    expect(config).not.toHaveProperty("command");
    expect(config).not.toHaveProperty("model");
    expect(config).not.toHaveProperty("agentType");
  });
});

describe("parseStdoutLine", () => {
  const ts = "2026-01-01T00:00:00.000Z";

  it("passes non-JSON through as raw stdout", () => {
    expect(parseStdoutLine("plain text", ts)).toEqual([
      { kind: "stdout", ts, text: "plain text" },
    ]);
  });

  it("labels the session as Devin, not as the literal custom command", () => {
    const [entry] = parseStdoutLine(
      JSON.stringify({ type: "acpx.session", agent: "custom", mode: "persistent" }),
      ts,
    );
    expect(entry).toMatchObject({ kind: "init", model: "Devin (persistent)" });
  });

  it("routes thinking deltas away from assistant text", () => {
    const [entry] = parseStdoutLine(
      JSON.stringify({ type: "acpx.text_delta", text: "hmm", channel: "thought" }),
      ts,
    );
    expect(entry).toMatchObject({ kind: "thinking", text: "hmm" });
  });

  it("emits a result entry for a completed tool call", () => {
    const entries = parseStdoutLine(
      JSON.stringify({
        type: "acpx.tool_call",
        name: "shell",
        status: "completed",
        toolCallId: "t1",
        text: "ok",
      }),
      ts,
    );
    expect(entries.map((entry) => entry.kind)).toEqual(["tool_call", "tool_result"]);
    expect(entries[1]).toMatchObject({ isError: false, toolUseId: "t1" });
  });

  it("marks a failed tool call as an error", () => {
    const entries = parseStdoutLine(
      JSON.stringify({ type: "acpx.tool_call", name: "shell", status: "failed" }),
      ts,
    );
    expect(entries[1]).toMatchObject({ isError: true });
  });

  it("carries usage through on the final result", () => {
    const [entry] = parseStdoutLine(
      JSON.stringify({
        type: "acpx.result",
        stopReason: "end_turn",
        inputTokens: 10,
        outputTokens: 5,
      }),
      ts,
    );
    expect(entry).toMatchObject({ kind: "result", inputTokens: 10, outputTokens: 5 });
  });
});
