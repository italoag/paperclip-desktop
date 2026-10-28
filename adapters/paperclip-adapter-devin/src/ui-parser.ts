/**
 * Run-viewer transcript parser.
 *
 * Loaded into the browser by Paperclip and evaluated in a sandbox, so this
 * module must have **zero runtime imports and no side effects**. The types are
 * declared inline for that reason rather than imported from adapter-utils.
 *
 * The event stream is ACPX's (`acpx.session`, `acpx.text_delta`,
 * `acpx.tool_call`, ...) because that is what actually runs the agent, so this
 * is a port of the parser from the shared ACP engine
 * (MIT, Copyright (c) 2025 Paperclip AI). A built-in adapter gets its parser
 * compiled into the UI; an external one has to ship its own copy.
 */

export type TranscriptEntry =
  | { kind: "stdout"; ts: string; text: string }
  | { kind: "stderr"; ts: string; text: string }
  | { kind: "system"; ts: string; text: string }
  | { kind: "init"; ts: string; model?: string; sessionId?: string }
  | { kind: "assistant"; ts: string; text: string; delta?: boolean }
  | { kind: "thinking"; ts: string; text: string; delta?: boolean }
  | {
      kind: "tool_call";
      ts: string;
      name: string;
      toolUseId?: string;
      input: unknown;
    }
  | {
      kind: "tool_result";
      ts: string;
      toolUseId: string;
      toolName?: string;
      content: string;
      isError?: boolean;
    }
  | {
      kind: "result";
      ts: string;
      text: string;
      inputTokens: number;
      outputTokens: number;
      cachedTokens: number;
      costUsd: number;
      subtype: string;
      isError: boolean;
      errors: string[];
    };

function parseJson(line: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(line);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return null;
    return parsed as Record<string, unknown>;
  } catch {
    return null;
  }
}

function asString(value: unknown, fallback = ""): string {
  return typeof value === "string" ? value : fallback;
}

function asNumber(value: unknown, fallback = 0): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}

function stringify(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === null || value === undefined) return "";
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function pickToolUseId(parsed: Record<string, unknown>): string {
  return (
    asString(parsed.toolCallId) || asString(parsed.toolUseId) || asString(parsed.id)
  );
}

function statusText(parsed: Record<string, unknown>): string {
  const text = asString(parsed.text).trim();
  const tag = asString(parsed.tag).trim();
  const used = asNumber(parsed.used, -1);
  const size = asNumber(parsed.size, -1);

  const parts: string[] = [];
  if (text) parts.push(text);
  if (tag && !text) parts.push(tag);
  if (used >= 0 && size > 0) parts.push(`(${used}/${size} ctx)`);
  return parts.join(" ") || tag || "status";
}

export function parseStdoutLine(line: string, ts: string): TranscriptEntry[] {
  const parsed = parseJson(line);
  if (!parsed) return [{ kind: "stdout", ts, text: line }];

  const type = asString(parsed.type);

  if (type === "acpx.session") {
    const mode = asString(parsed.mode);
    const permissionMode = asString(parsed.permissionMode);
    const tail = [mode, permissionMode].filter(Boolean).join(" / ");
    // Labelled "Devin" rather than ACPX's own agent name, which for a custom
    // command is the uninformative literal "custom".
    return [
      {
        kind: "init",
        ts,
        model: tail ? `Devin (${tail})` : "Devin",
        sessionId:
          asString(parsed.acpSessionId) ||
          asString(parsed.sessionId) ||
          asString(parsed.runtimeSessionName),
      },
    ];
  }

  if (type === "acpx.text_delta") {
    const text = asString(parsed.text);
    if (!text) return [];
    const channel = asString(parsed.channel) || asString(parsed.stream);
    return [
      {
        kind: channel === "thought" || channel === "thinking" ? "thinking" : "assistant",
        ts,
        text,
        delta: true,
      },
    ];
  }

  if (type === "acpx.tool_call") {
    const status = asString(parsed.status);
    const text = asString(parsed.text);
    const name = asString(parsed.name, "acp_tool");
    const toolUseId = pickToolUseId(parsed);

    const rawInput = parsed.input;
    const input = (() => {
      if (rawInput === undefined) {
        return text || status
          ? { ...(text ? { text } : {}), ...(status ? { status } : {}) }
          : {};
      }
      const record = asRecord(rawInput);
      if (!record) return rawInput;
      return {
        ...record,
        ...(status && record.status === undefined ? { status } : {}),
        ...(text && record.text === undefined ? { text } : {}),
      };
    })();

    const entries: TranscriptEntry[] = [
      { kind: "tool_call", ts, name, toolUseId: toolUseId || undefined, input },
    ];

    if (status === "completed" || status === "failed" || status === "cancelled") {
      entries.push({
        kind: "tool_result",
        ts,
        toolUseId: toolUseId || name,
        toolName: name,
        content: text || status,
        isError: status !== "completed",
      });
    }

    return entries;
  }

  if (type === "acpx.tool_result") {
    return [
      {
        kind: "tool_result",
        ts,
        toolUseId: pickToolUseId(parsed) || asString(parsed.name, "acp_tool"),
        toolName: asString(parsed.name) || undefined,
        content: stringify(parsed.content ?? parsed.output ?? parsed.error),
        isError: parsed.isError === true || parsed.error !== undefined,
      },
    ];
  }

  if (type === "acpx.status") {
    return [{ kind: "system", ts, text: statusText(parsed) }];
  }

  if (type === "acpx.result") {
    return [
      {
        kind: "result",
        ts,
        text: asString(parsed.summary, asString(parsed.stopReason, asString(parsed.text))),
        inputTokens: asNumber(parsed.inputTokens),
        outputTokens: asNumber(parsed.outputTokens),
        cachedTokens: asNumber(parsed.cachedTokens),
        costUsd: asNumber(parsed.costUsd),
        subtype: asString(parsed.subtype, asString(parsed.stopReason, "acpx.result")),
        isError: parsed.isError === true,
        errors: Array.isArray(parsed.errors)
          ? parsed.errors.map((error) => stringify(error)).filter(Boolean)
          : [],
      },
    ];
  }

  if (type === "acpx.error") {
    return [{ kind: "stderr", ts, text: asString(parsed.message, line) }];
  }

  if (type.startsWith("acpx.")) {
    return [{ kind: "system", ts, text: asString(parsed.message, type) }];
  }

  return [{ kind: "stdout", ts, text: line }];
}

export default parseStdoutLine;
