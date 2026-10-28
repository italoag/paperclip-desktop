/**
 * Paperclip skill sync for Devin.
 *
 * Devin discovers skills from `<XDG_CONFIG_HOME>/devin/skills/<name>/SKILL.md`
 * (`devin skills paths`), and it honours `XDG_CONFIG_HOME`. So each agent gets
 * its own Devin config home under the adapter's state directory, and Paperclip's
 * selected skills are materialised into it before every run.
 *
 * Why a private config home rather than the user's `~/.config/devin`:
 * agents are per-company and per-role, and writing into the human's own Devin
 * configuration would leak one agent's skills into every other agent *and* into
 * the user's interactive sessions. The cost is that Devin would also stop seeing
 * the user's `config.json` (permissions, hooks, model defaults), so those files
 * are linked through — only `skills/` is Paperclip-managed.
 */

import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import type {
  AdapterSkillContext,
  AdapterSkillSnapshot,
} from "@paperclipai/adapter-utils";
import {
  buildRuntimeMountedSkillSnapshot,
  readInstalledSkillTargets,
  readPaperclipRuntimeSkillEntries,
  resolvePaperclipDesiredSkillNames,
} from "@paperclipai/adapter-utils/server-utils";

import { ADAPTER_TYPE } from "../identity.js";

const moduleDir = path.dirname(fileURLToPath(import.meta.url));

/** Files linked from the user's real Devin config so behaviour is preserved. */
const INHERITED_CONFIG_FILES = ["config.json", "mcp_config.json"];

export function devinConfigHome(stateDir: string): string {
  return path.join(stateDir, "devin-config");
}

export function devinSkillsDir(stateDir: string): string {
  return path.join(devinConfigHome(stateDir), "devin", "skills");
}

async function buildSnapshot(
  config: Record<string, unknown>,
  stateDir: string,
): Promise<AdapterSkillSnapshot> {
  const availableEntries = await readPaperclipRuntimeSkillEntries(config, moduleDir);
  const desiredSkills = resolvePaperclipDesiredSkillNames(config, availableEntries);
  const skillsHome = devinSkillsDir(stateDir);

  return buildRuntimeMountedSkillSnapshot({
    adapterType: ADAPTER_TYPE,
    availableEntries,
    desiredSkills,
    configuredDetail:
      "Will be materialized into this agent's Devin skills directory on the next run.",
    externalInstalled: await readInstalledSkillTargets(skillsHome),
    externalLocationLabel: "<agent state>/devin-config/devin/skills",
    externalDetail: "Present in the agent's Devin skills directory but not managed by Paperclip.",
    skillsHome,
  });
}

export function createSkillHandlers(resolveStateDirFor: (ctx: AdapterSkillContext) => string) {
  return {
    listSkills: (ctx: AdapterSkillContext) =>
      buildSnapshot(ctx.config, resolveStateDirFor(ctx)),
    syncSkills: (ctx: AdapterSkillContext, _desired: string[]) =>
      buildSnapshot(ctx.config, resolveStateDirFor(ctx)),
  };
}

// ---------------------------------------------------------------------------
// Materialisation
// ---------------------------------------------------------------------------

async function linkInheritedConfig(configHome: string): Promise<void> {
  const userConfigDir = path.join(
    process.env.XDG_CONFIG_HOME?.trim() || path.join(os.homedir(), ".config"),
    "devin",
  );
  const managedDir = path.join(configHome, "devin");
  await fs.mkdir(managedDir, { recursive: true });

  for (const file of INHERITED_CONFIG_FILES) {
    const source = path.join(userConfigDir, file);
    const target = path.join(managedDir, file);

    // Symlinked, not copied: the user editing their Devin settings should reach
    // the agents too, and a copy would silently go stale.
    try {
      await fs.access(source);
    } catch {
      continue;
    }
    try {
      const existing = await fs.lstat(target).catch(() => null);
      if (existing?.isSymbolicLink()) {
        if ((await fs.readlink(target)) === source) continue;
        await fs.rm(target, { force: true });
      } else if (existing) {
        continue; // Something real is there; leave it alone.
      }
      await fs.symlink(source, target);
    } catch {
      // A missing link only costs the inherited settings, not the run.
    }
  }
}

/**
 * Copies the skills Paperclip selected into the agent's Devin skills directory,
 * and removes managed ones that are no longer selected.
 *
 * Copied rather than symlinked because Devin reads `SKILL.md` plus any sibling
 * files a skill ships, and a dangling link inside a skill directory turns into a
 * confusing runtime error rather than a missing skill.
 */
export async function materializeSkills(
  config: Record<string, unknown>,
  stateDir: string,
): Promise<{ installed: string[]; warnings: string[] }> {
  const warnings: string[] = [];
  const entries = await readPaperclipRuntimeSkillEntries(config, moduleDir);
  const desired = new Set(resolvePaperclipDesiredSkillNames(config, entries));

  const skillsDir = devinSkillsDir(stateDir);
  await fs.mkdir(skillsDir, { recursive: true });
  await linkInheritedConfig(devinConfigHome(stateDir));

  const installed: string[] = [];

  for (const entry of entries) {
    const name = entry.runtimeName || entry.key;
    if (!desired.has(name) && !desired.has(entry.key)) continue;

    if (entry.sourceStatus === "missing" || !entry.source) {
      warnings.push(`Skill "${name}" is unavailable: ${entry.missingDetail ?? "no source path"}`);
      continue;
    }

    const target = path.join(skillsDir, name);
    try {
      // Replaced wholesale so a skill that lost a file does not keep it.
      await fs.rm(target, { recursive: true, force: true });
      const stat = await fs.stat(entry.source);
      if (stat.isDirectory()) {
        await fs.cp(entry.source, target, { recursive: true, dereference: true });
      } else {
        await fs.mkdir(target, { recursive: true });
        await fs.copyFile(entry.source, path.join(target, "SKILL.md"));
      }
      installed.push(name);
    } catch (error) {
      warnings.push(
        `Could not install skill "${name}": ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }

  // Prune skills this adapter installed earlier that are no longer selected.
  // Anything Paperclip does not know about is left alone: an operator may have
  // dropped a skill in by hand.
  const known = new Set(entries.map((entry) => entry.runtimeName || entry.key));
  for (const present of await fs.readdir(skillsDir).catch(() => [])) {
    if (installed.includes(present)) continue;
    if (!known.has(present)) continue;
    await fs.rm(path.join(skillsDir, present), { recursive: true, force: true });
  }

  return { installed, warnings };
}
