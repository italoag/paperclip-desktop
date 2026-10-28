import { describe, expect, it } from "vitest";

import { isTransientModelFailure, modelAttemptOrder } from "../src/server/retry.js";

const failure = (errorMessage: string) => ({ exitCode: 1, errorMessage, summary: null });

describe("isTransientModelFailure", () => {
  /** The exact text seen in the run logs when the provider was down. */
  it("recognises the provider outage from the logs", () => {
    expect(
      isTransientModelFailure(
        failure("Protocol error (unavailable): The third-party model provider is experiencing issues"),
      ),
    ).toBe(true);
  });

  it("recognises the ACP connection error the turn dies with", () => {
    expect(
      isTransientModelFailure(
        failure("Connection error, send a message to continue retrying (error id: cf2d4aa1)"),
      ),
    ).toBe(true);
  });

  it("never retries a successful run", () => {
    expect(isTransientModelFailure({ exitCode: 0, errorMessage: null, summary: null })).toBe(false);
  });

  /**
   * The expensive mistake: retrying a task the agent genuinely failed bills the
   * same failure twice on a second model.
   */
  it("does not retry a genuine task failure", () => {
    expect(isTransientModelFailure(failure("patch did not apply cleanly"))).toBe(false);
    expect(isTransientModelFailure(failure("agent refused the request"))).toBe(false);
  });

  it("does not retry a timeout the operator asked for", () => {
    expect(isTransientModelFailure(failure("run timed out after 7200s"))).toBe(false);
  });

  it("does not retry auth or policy failures, which no model fixes", () => {
    expect(
      isTransientModelFailure(failure("Permission denied: Your IP address is not allowed by your team")),
    ).toBe(false);
    expect(isTransientModelFailure(failure("unauthorized"))).toBe(false);
  });

  it("ignores a failure with no message rather than guessing", () => {
    expect(isTransientModelFailure({ exitCode: 1, errorMessage: null, summary: null })).toBe(false);
  });

  it("reads the summary when the message is empty", () => {
    expect(
      isTransientModelFailure({ exitCode: 1, errorMessage: null, summary: "protocol error (unavailable)" }),
    ).toBe(true);
  });
});

describe("modelAttemptOrder", () => {
  it("tries the primary first, then each fallback", () => {
    expect(modelAttemptOrder("primary", ["a", "b"])).toEqual(["primary", "a", "b"]);
  });

  /** A fallback list repeating the primary must not change what runs first. */
  it("never repeats the primary", () => {
    expect(modelAttemptOrder("primary", ["primary", "a"])).toEqual(["primary", "a"]);
  });

  it("drops duplicates and blanks from the list", () => {
    expect(modelAttemptOrder("p", ["a", "a", "  ", "b"])).toEqual(["p", "a", "b"]);
  });

  it("is a single attempt when nothing is configured", () => {
    expect(modelAttemptOrder("p", undefined)).toEqual(["p"]);
    expect(modelAttemptOrder("p", "not-an-array")).toEqual(["p"]);
  });

  it("ignores non-string entries", () => {
    expect(modelAttemptOrder("p", ["a", 42, null])).toEqual(["p", "a"]);
  });
});
