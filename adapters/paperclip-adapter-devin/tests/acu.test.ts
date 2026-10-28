import { DatabaseSync } from "node:sqlite";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { acuSpentThisRun, acuToLedgerCost, readSessionAcu } from "../src/server/acu.js";

let scratch: string | undefined;
afterEach(() => {
  if (scratch) rmSync(scratch, { recursive: true, force: true });
  scratch = undefined;
});

/** Mirrors the shape the Devin CLI writes. */
function sessionDb(rows: Array<{ id: string; metadata: string | null }>): string {
  scratch = mkdtempSync(path.join(tmpdir(), "devin-acu-"));
  const file = path.join(scratch, "sessions.db");
  const db = new DatabaseSync(file);
  db.exec("CREATE TABLE sessions (id TEXT PRIMARY KEY, metadata TEXT)");
  const insert = db.prepare("INSERT INTO sessions (id, metadata) VALUES (?, ?)");
  for (const row of rows) insert.run(row.id, row.metadata);
  db.close();
  return file;
}

const REAL_METADATA = JSON.stringify({
  total_credit_cost: 0,
  total_acu_cost: 0.047276001423597336,
  response_dimensions: [
    { uid: "acu", kind: { CumulativeMetric: { label: "ACUs spent", value: 0.047276 } } },
  ],
});

describe("readSessionAcu", () => {
  it("reads the cumulative total the CLI stored", () => {
    const file = sessionDb([{ id: "fierce-leopard", metadata: REAL_METADATA }]);

    expect(readSessionAcu("fierce-leopard", { databasePaths: [file] })).toBeCloseTo(0.047276, 6);
  });

  /** `null` must not be confused with a free run. */
  it("returns null for an unknown session", () => {
    const file = sessionDb([{ id: "other", metadata: REAL_METADATA }]);

    expect(readSessionAcu("missing", { databasePaths: [file] })).toBeNull();
  });

  it("returns null when the database does not exist", () => {
    expect(readSessionAcu("any", { databasePaths: ["/nonexistent/sessions.db"] })).toBeNull();
  });

  it("returns null when the row has no ACU field", () => {
    const file = sessionDb([{ id: "s", metadata: JSON.stringify({ total_credit_cost: 0 }) }]);

    expect(readSessionAcu("s", { databasePaths: [file] })).toBeNull();
  });

  it("survives unreadable metadata", () => {
    const file = sessionDb([{ id: "s", metadata: "not json" }]);

    expect(readSessionAcu("s", { databasePaths: [file] })).toBeNull();
  });

  it("falls through to the next candidate database", () => {
    const file = sessionDb([{ id: "s", metadata: REAL_METADATA }]);

    expect(
      readSessionAcu("s", { databasePaths: ["/nonexistent/sessions.db", file] }),
    ).toBeCloseTo(0.047276, 6);
  });

  it("ignores a blank session id", () => {
    expect(readSessionAcu("  ", { databasePaths: [] })).toBeNull();
  });
});

describe("acuSpentThisRun", () => {
  const metadata = (acu: number) => JSON.stringify({ total_acu_cost: acu });

  it("charges only this run when a session is resumed", () => {
    const file = sessionDb([{ id: "s", metadata: metadata(10) }]);

    const spent = acuSpentThisRun({
      previousSessionId: "s",
      sessionId: "s",
      before: 4,
      options: { databasePaths: [file] },
    });

    expect(spent).toBe(6);
  });

  it("charges the whole total for a new session", () => {
    const file = sessionDb([{ id: "fresh", metadata: metadata(3) }]);

    const spent = acuSpentThisRun({
      previousSessionId: "old",
      sessionId: "fresh",
      before: 99,
      options: { databasePaths: [file] },
    });

    expect(spent).toBe(3);
  });

  /** A total that went down means the session's counter restarted. */
  it("uses the post-run total when the counter reset", () => {
    const file = sessionDb([{ id: "s", metadata: metadata(2) }]);

    const spent = acuSpentThisRun({
      previousSessionId: "s",
      sessionId: "s",
      before: 50,
      options: { databasePaths: [file] },
    });

    expect(spent).toBe(2);
  });

  it("reports null rather than zero when the lookup fails", () => {
    expect(
      acuSpentThisRun({ sessionId: "s", options: { databasePaths: ["/nope.db"] } }),
    ).toBeNull();
  });
});

describe("acuToLedgerCost", () => {
  /** The default: budgets denominated in ACUs. */
  it("passes ACUs through unscaled with no rate", () => {
    expect(acuToLedgerCost(2.5)).toBe(2.5);
    expect(acuToLedgerCost(2.5, null)).toBe(2.5);
  });

  it("converts to dollars when a rate is configured", () => {
    expect(acuToLedgerCost(2, 2.25)).toBe(4.5);
  });

  it("ignores a nonsensical rate rather than zeroing the cost", () => {
    expect(acuToLedgerCost(3, 0)).toBe(3);
    expect(acuToLedgerCost(3, Number.NaN)).toBe(3);
    expect(acuToLedgerCost(3, -1)).toBe(3);
  });
});

/**
 * Runs against the compiled ESM output, in a plain Node process.
 *
 * The unit tests above import TypeScript through vitest, which supplies a
 * `require` shim that the real ESM runtime does not. A first version of this
 * module used bare `require("node:sqlite")`: green here, and silently
 * cost-blind in production. This is the only test that would have caught it.
 */
describe("built output", () => {
  it("reads a real database from compiled ESM", async () => {
    const { execFileSync } = await import("node:child_process");
    const { existsSync } = await import("node:fs");
    const dist = path.resolve(__dirname, "../dist/server/acu.js");
    if (!existsSync(dist)) return; // `pnpm build` has not run

    const file = sessionDb([{ id: "compiled", metadata: REAL_METADATA }]);
    const output = execFileSync(
      process.execPath,
      [
        "--input-type=module",
        "-e",
        `import { readSessionAcu } from ${JSON.stringify(dist)};
         process.stdout.write(String(readSessionAcu("compiled", { databasePaths: [${JSON.stringify(file)}] })));`,
      ],
      { encoding: "utf8" },
    );

    expect(Number(output)).toBeCloseTo(0.047276, 6);
  });
});

/**
 * Rounding, stated explicitly.
 *
 * Paperclip stores `Math.round(costUsd * 100)`, so ACU spend is tracked to two
 * decimals. Each run is off by up to 0.005 ACU in either direction, and runs
 * below 0.005 record nothing at all. These assertions exist so the limit is a
 * documented property rather than a surprise found in a reconciliation.
 */
describe("ledger precision", () => {
  const cents = (acus: number) => Math.round(acuToLedgerCost(acus) * 100);

  it("keeps two decimals of an ACU", () => {
    expect(cents(0.057)).toBe(6);
    expect(cents(1.234)).toBe(123);
  });

  it("loses runs cheaper than half a cent", () => {
    expect(cents(0.004)).toBe(0);
  });

  it("still records anything at or above half a cent", () => {
    expect(cents(0.005)).toBe(1);
  });
});
