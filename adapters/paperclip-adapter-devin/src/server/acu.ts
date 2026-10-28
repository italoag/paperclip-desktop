/**
 * Reads how many ACUs a Devin session has consumed.
 *
 * # Why the session database
 *
 * ACUs (Agent Compute Units) are Cognition's billing unit, and the ACP stream
 * does not carry them: its only usage signal is context-window occupancy
 * (`usage updated: 45793/1000000`), which is not cost. The CLI has no
 * non-interactive `usage` command either — `/usage` and `/session-stats` are
 * slash commands inside the TUI.
 *
 * What the CLI does do is persist what the server reported, in its own session
 * database, as `metadata.total_acu_cost`. That number matches `/session-stats`
 * exactly, and the ACP session id is the primary key of that table, so a run can
 * look up its own spend.
 *
 * # The coupling, stated plainly
 *
 * This reads another tool's private storage. Nothing documents that schema and
 * a Devin release may change it. Two things keep that honest: the database is
 * opened read-only and never written, and every failure path returns `null`
 * rather than throwing, so a schema change costs cost-reporting and not the
 * run. The environment test surfaces whether it currently works.
 */

import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";

/**
 * Resolves `node:sqlite` without a static import.
 *
 * Static would be simpler, but the module does not exist before Node 22.5 and
 * an unresolvable import fails at load time — taking the whole adapter with it
 * instead of just cost reporting.
 *
 * `createRequire` rather than bare `require`: this package compiles to ESM,
 * where `require` is not defined. That mistake fails silently here — the lookup
 * just returns `null` and every run reports no cost — so `readsTheRealDatabase`
 * in the tests exercises the built ESM output rather than trusting the bundler's
 * CJS interop.
 */
let cachedDatabaseSync: typeof import("node:sqlite").DatabaseSync | null | undefined;

function loadDatabaseSync(): typeof import("node:sqlite").DatabaseSync | null {
  if (cachedDatabaseSync !== undefined) return cachedDatabaseSync;
  try {
    const nodeRequire = createRequire(import.meta.url);
    cachedDatabaseSync = (nodeRequire("node:sqlite") as typeof import("node:sqlite"))
      .DatabaseSync;
  } catch {
    cachedDatabaseSync = null;
  }
  return cachedDatabaseSync;
}

/**
 * Candidate database locations, in priority order.
 *
 * `cli-next` exists alongside `cli` on machines that have run a preview build;
 * both are checked because either may hold the session.
 */
const DB_CANDIDATES = [
  ".local/share/devin/cli/sessions.db",
  ".local/share/devin/cli-next/sessions.db",
];

const ACU_QUERY =
  "SELECT json_extract(metadata, '$.total_acu_cost') AS acu FROM sessions WHERE id = ?";

/**
 * Narrows a value from the database to a usable number.
 *
 * Explicit about null and empty string because `Number(null)` and `Number("")`
 * are both `0` — a missing column would otherwise read as a free run, which is
 * exactly the confusion this module is supposed to prevent.
 */
function coerceAcu(raw: unknown): number | null {
  if (typeof raw === "number") return Number.isFinite(raw) ? raw : null;
  if (typeof raw !== "string" || raw.trim() === "") return null;
  const parsed = Number(raw);
  return Number.isFinite(parsed) ? parsed : null;
}

export interface AcuLookupOptions {
  home?: string | undefined;
  /** Overrides the candidate list; for tests. */
  databasePaths?: string[] | undefined;
}

export function sessionDatabasePaths(options: AcuLookupOptions = {}): string[] {
  if (options.databasePaths) return options.databasePaths;
  const home = options.home ?? os.homedir();
  return DB_CANDIDATES.map((candidate) => path.join(home, candidate));
}

/**
 * Cumulative ACUs for `sessionId`, or `null` when unavailable.
 *
 * `null` is not zero: it means "could not read", which must not be mistaken for
 * a free run when computing a delta.
 */
export function readSessionAcu(
  sessionId: string | null | undefined,
  options: AcuLookupOptions = {},
): number | null {
  const id = sessionId?.trim();
  if (!id) return null;

  const DatabaseSync = loadDatabaseSync();
  if (!DatabaseSync) return null;

  for (const file of sessionDatabasePaths(options)) {
    if (!existsSync(file)) continue;

    let database: InstanceType<typeof DatabaseSync> | null = null;
    try {
      // Read-only so a bug here can never corrupt the user's Devin history, and
      // so an open CLI holding the database cannot be disturbed.
      database = new DatabaseSync(file, { readOnly: true });
      const row = database.prepare(ACU_QUERY).get(id) as { acu?: unknown } | undefined;
      const value = coerceAcu(row?.acu);
      if (value !== null) return value;
    } catch {
      // Locked, missing table, changed schema — try the next candidate.
    } finally {
      try {
        database?.close();
      } catch {
        /* already closed */
      }
    }
  }

  return null;
}

export interface AcuSpendInput {
  /** Session the run resumed, if any. */
  previousSessionId?: string | null | undefined;
  /** Session the run ended on. */
  sessionId?: string | null | undefined;
  options?: AcuLookupOptions;
  /** Cumulative total read before the run started. */
  before?: number | null;
}

/**
 * ACUs attributable to one run.
 *
 * The stored total is cumulative per session, so a run's own spend is the delta
 * against the pre-run reading. Two cases collapse to "use the total as-is":
 * a run that started a new session, and a total that went *down* — which means
 * the session was replaced and its counter restarted.
 */
export function acuSpentThisRun(input: AcuSpendInput): number | null {
  const after = readSessionAcu(input.sessionId, input.options ?? {});
  if (after === null) return null;

  const sameSession =
    Boolean(input.previousSessionId) && input.previousSessionId === input.sessionId;
  if (!sameSession || input.before === null || input.before === undefined) return after;

  const delta = after - input.before;
  return delta >= 0 ? delta : after;
}

/**
 * Converts ACUs into the number Paperclip's ledger stores.
 *
 * `costUsd` is the only numeric channel into the budget — `billingType` merely
 * labels the entry — so ACUs have to travel through it. With no rate configured
 * the value passes through unscaled, which denominates the budget in ACUs: a
 * limit of 500 means 500 ACUs. Setting `acuRateUsd` converts to real money
 * instead, for organisations that know their contracted rate.
 *
 * Either way the ledger rounds to cents, so spend is tracked to 0.01 units.
 */
export function acuToLedgerCost(acus: number, acuRateUsd?: number | null): number {
  const rate = typeof acuRateUsd === "number" && Number.isFinite(acuRateUsd) && acuRateUsd > 0
    ? acuRateUsd
    : 1;
  return acus * rate;
}
