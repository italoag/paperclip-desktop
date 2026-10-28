/**
 * Deciding when a failed run is worth retrying on a different model.
 *
 * # Why this exists
 *
 * Devin's own control loop already retries a failing turn three times
 * (`attempts=3`). When the third-party provider behind a model is down, all
 * three fail and the turn dies with an ACP error:
 *
 * ```
 * error=Error { code: -32010, message: "Connection error, send a message to continue retrying (error id: ...)" }
 * ```
 *
 * Retrying the same model again would fail a fourth time. A different model
 * usually sits behind a different provider, so it is the only retry with a
 * reason to succeed.
 *
 * # Why the matching is narrow
 *
 * Every retry costs a fresh session and more ACUs, and the agent loses the
 * context of the failed attempt. So this only fires on failures that are
 * provably about model availability. An agent that genuinely failed the task —
 * bad patch, refused request, hit its timeout — must not be run again on
 * another model, because the second attempt would fail the same way and bill
 * twice for it.
 */

import type { AdapterExecutionResult } from "@paperclipai/adapter-utils";

/**
 * Signatures of "the model could not be reached", observed in the run logs.
 *
 * Deliberately specific strings rather than a loose `/error/i`: the cost of a
 * false positive is a duplicate paid run.
 */
const TRANSIENT_PATTERNS = [
  /third-party model provider is experiencing issues/i,
  /connection error, send a message to continue retrying/i,
  /protocol error \(unavailable\)/i,
  /model .* is (currently )?not available/i,
  /\b(503|502|overloaded|capacity)\b/i,
];

/** Failures that look transient but must never trigger a paid retry. */
const NEVER_RETRY = [
  // The operator asked for a bound and it was reached; running again on another
  // model just spends the same budget twice.
  /timed? ?out/i,
  /cancell?ed/i,
  // Auth and policy do not improve by changing model.
  /not allowed by your team/i,
  /unauthorized|forbidden/i,
];

export function isTransientModelFailure(
  result: Pick<AdapterExecutionResult, "exitCode" | "errorMessage" | "summary">,
): boolean {
  if (result.exitCode === 0) return false;

  const text = `${result.errorMessage ?? ""} ${result.summary ?? ""}`.trim();
  if (!text) return false;

  if (NEVER_RETRY.some((pattern) => pattern.test(text))) return false;
  return TRANSIENT_PATTERNS.some((pattern) => pattern.test(text));
}

/**
 * The models to try, in order, without repeats.
 *
 * The primary comes first even when it also appears in the fallback list, so a
 * misconfigured list cannot silently change which model runs first.
 */
export function modelAttemptOrder(
  primary: string | undefined,
  fallbacks: unknown,
): Array<string | undefined> {
  const order: Array<string | undefined> = [primary];
  const seen = new Set([primary?.trim()].filter(Boolean));

  if (Array.isArray(fallbacks)) {
    for (const candidate of fallbacks) {
      if (typeof candidate !== "string") continue;
      const model = candidate.trim();
      if (!model || seen.has(model)) continue;
      seen.add(model);
      order.push(model);
    }
  }

  return order;
}
