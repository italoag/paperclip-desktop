#!/usr/bin/env node
/**
 * Reconciles what Paperclip recorded against what Devin actually charged.
 *
 * The adapter reads ACU spend out of Devin's session database and reports it as
 * the ledger's cost (see `adapters/paperclip-adapter-devin/src/server/acu.ts`).
 * That pipeline is tested, but tests only prove the arithmetic — they cannot
 * prove that every run reached the ledger. Runs that failed before returning, or
 * that were rounded below a cent, leave no trace and no error.
 *
 * This compares the two totals directly. A gap does not say which side is wrong,
 * only that they disagree — which is the signal worth having.
 *
 * Usage:
 *   node scripts/acu-audit.mjs [--since YYYY-MM-DD] [--server http://localhost:3100]
 */

import { DatabaseSync } from "node:sqlite";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const DB_CANDIDATES = [
  ".local/share/devin/cli/sessions.db",
  ".local/share/devin/cli-next/sessions.db",
];

function parseArgs(argv) {
  const args = { server: "http://localhost:3100", since: startOfMonth() };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === "--since") args.since = argv[++i];
    else if (argv[i] === "--server") args.server = argv[++i];
  }
  return args;
}

function startOfMonth() {
  const now = new Date();
  return `${now.getUTCFullYear()}-${String(now.getUTCMonth() + 1).padStart(2, "0")}-01`;
}

/**
 * Session ids the adapter actually drove, from its own state directories.
 *
 * An earlier version guessed by working directory. That was wrong in both
 * directions: agents run inside ordinary project checkouts, not under
 * Paperclip's own folders, so real agent sessions were missed — and the guess
 * reported a 57 ACU shortfall that did not exist. The adapter writes one file
 * per ACP session containing `acp_session_id`, which is the Devin session id, so
 * the join can be exact instead.
 */
function adapterSessionIds() {
  const roots = [
    path.join(os.homedir(), ".paperclip-adapter-devin"),
    // Where the state lives when it was never relocated; see `state-dir.ts`.
    path.join(os.homedir(), "Library/Application Support/Paperclip/instances"),
    path.join(os.homedir(), ".paperclip/instances"),
  ].filter(existsSync);

  const ids = new Set();
  for (const root of roots) {
    for (const file of walkSessionFiles(root)) {
      try {
        const record = JSON.parse(readFileSync(file, "utf8"));
        if (typeof record.acp_session_id === "string") ids.add(record.acp_session_id);
      } catch {
        // A half-written session file is not worth failing the audit over.
      }
    }
  }
  return ids;
}

function* walkSessionFiles(dir, depth = 0) {
  if (depth > 8) return;
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) yield* walkSessionFiles(full, depth + 1);
    else if (path.basename(dir) === "sessions" && entry.name.endsWith(".json")) yield full;
  }
}

/** ACUs Devin charged for the sessions the adapter drove. */
function devinSpend(since) {
  const epoch = Math.floor(new Date(`${since}T00:00:00Z`).getTime() / 1000);
  const ours = adapterSessionIds();
  let total = 0;
  const sessions = [];

  for (const candidate of DB_CANDIDATES) {
    const file = path.join(os.homedir(), candidate);
    if (!existsSync(file)) continue;

    const db = new DatabaseSync(file, { readOnly: true });
    try {
      const rows = db
        .prepare(
          `SELECT id, json_extract(metadata, '$.total_acu_cost') AS acu
           FROM sessions WHERE last_activity_at >= ?`,
        )
        .all(epoch);
      for (const row of rows) {
        if (!ours.has(row.id)) continue;
        const acu = typeof row.acu === "number" ? row.acu : Number(row.acu);
        if (!Number.isFinite(acu) || acu <= 0) continue;
        total += acu;
        sessions.push({ id: row.id, acu });
      }
    } finally {
      db.close();
    }
  }

  return { total, sessions, known: ours.size };
}

async function paperclipSpend(server) {
  const companies = await fetchJson(`${server}/api/companies`);
  const list = Array.isArray(companies) ? companies : (companies.companies ?? []);

  let total = 0;
  const agents = [];
  for (const company of list) {
    const payload = await fetchJson(`${server}/api/companies/${company.id}/agents`);
    const rows = Array.isArray(payload) ? payload : (payload.agents ?? []);
    for (const agent of rows) {
      if (agent.adapterType !== "devin_local") continue;
      const acu = (agent.spentMonthlyCents ?? 0) / 100;
      total += acu;
      agents.push({ name: agent.name, acu });
    }
  }
  return { total, agents };
}

async function fetchJson(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(15_000) });
  if (!response.ok) throw new Error(`${url} -> HTTP ${response.status}`);
  return response.json();
}

function format(value) {
  return value.toFixed(4).padStart(11);
}

async function main() {
  const { server, since } = parseArgs(process.argv.slice(2));

  const devin = devinSpend(since);
  let paperclip;
  try {
    paperclip = await paperclipSpend(server);
  } catch (error) {
    console.error(
      `[acu-audit] could not reach Paperclip at ${server}: ${error.message}\n` +
        "Start the app and connect to the local server first.",
    );
    process.exit(1);
  }

  console.log(`ACU reconciliation since ${since}\n`);
  console.log("  Devin sessions (source of truth)");
  for (const session of devin.sessions.sort((a, b) => b.acu - a.acu).slice(0, 8)) {
    console.log(`    ${format(session.acu)}  ${session.id}`);
  }
  console.log(
    `    ${format(devin.total)}  TOTAL ` +
      `(${devin.sessions.length} of ${devin.known} adapter sessions in range)\n`,
  );

  console.log("  Paperclip ledger (devin_local agents)");
  for (const agent of paperclip.agents.sort((a, b) => b.acu - a.acu)) {
    console.log(`    ${format(agent.acu)}  ${agent.name}`);
  }
  console.log(`    ${format(paperclip.total)}  TOTAL\n`);

  const gap = paperclip.total - devin.total;
  // Half a cent per run is lost or gained to the ledger's rounding, so a small
  // gap is expected; a large one is not.
  const tolerance = Math.max(0.01, devin.sessions.length * 0.005);

  if (Math.abs(gap) <= tolerance) {
    console.log(`  Agreement within rounding (gap ${gap.toFixed(4)} ACU).`);
    return;
  }

  console.log(
    `  MISMATCH: ledger is ${gap > 0 ? "over" : "under"} by ${Math.abs(gap).toFixed(4)} ACU ` +
      `(tolerance ${tolerance.toFixed(4)}).`,
  );
  console.log(
    gap < 0
      ? "  Under-reporting usually means runs failed before returning a result,\n" +
          "  or ran before the ACU code shipped. Check the run log for\n" +
          '  "run failed after spending".'
      : "  Over-reporting usually means sessions were shared between agents, so a\n" +
          "  delta was attributed to the wrong run.",
  );
  process.exitCode = 1;
}

main();
