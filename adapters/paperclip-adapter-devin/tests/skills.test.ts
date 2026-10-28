import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { devinConfigHome, devinSkillsDir, materializeSkills } from "../src/server/skills.js";

let scratch: string;

function setup() {
  scratch = mkdtempSync(path.join(tmpdir(), "devin-skills-"));
  return {
    state: path.join(scratch, "state"),
    source: path.join(scratch, "source"),
  };
}

afterEach(() => rmSync(scratch, { recursive: true, force: true }));

function skillSource(root: string, name: string, extra?: string): string {
  const dir = path.join(root, name);
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, "SKILL.md"), `---\nname: ${name}\n---\nbody\n`);
  if (extra) writeFileSync(path.join(dir, extra), "sibling");
  return dir;
}

function config(entries: Array<{ name: string; source: string }>, desired: string[]) {
  return {
    paperclipRuntimeSkills: entries.map((entry) => ({
      key: entry.name,
      runtimeName: entry.name,
      source: entry.source,
      sourceStatus: "available",
    })),
    paperclipSkillSync: { desiredSkills: desired },
  };
}

describe("materializeSkills", () => {
  it("installs a selected skill where Devin looks for it", async () => {
    const { state, source } = setup();
    const result = await materializeSkills(
      config([{ name: "alpha", source: skillSource(source, "alpha") }], ["alpha"]),
      state,
    );

    expect(result.installed).toEqual(["alpha"]);
    expect(existsSync(path.join(devinSkillsDir(state), "alpha", "SKILL.md"))).toBe(true);
  });

  /** A skill is a directory, not just SKILL.md — siblings must come along. */
  it("copies files the skill ships alongside SKILL.md", async () => {
    const { state, source } = setup();
    await materializeSkills(
      config([{ name: "alpha", source: skillSource(source, "alpha", "helper.md") }], ["alpha"]),
      state,
    );

    expect(existsSync(path.join(devinSkillsDir(state), "alpha", "helper.md"))).toBe(true);
  });

  it("does not install a skill that was not selected", async () => {
    const { state, source } = setup();
    const result = await materializeSkills(
      config(
        [
          { name: "alpha", source: skillSource(source, "alpha") },
          { name: "beta", source: skillSource(source, "beta") },
        ],
        ["alpha"],
      ),
      state,
    );

    expect(result.installed).toEqual(["alpha"]);
    expect(existsSync(path.join(devinSkillsDir(state), "beta"))).toBe(false);
  });

  it("removes a managed skill once it is deselected", async () => {
    const { state, source } = setup();
    const entries = [{ name: "alpha", source: skillSource(source, "alpha") }];

    await materializeSkills(config(entries, ["alpha"]), state);
    expect(existsSync(path.join(devinSkillsDir(state), "alpha"))).toBe(true);

    await materializeSkills(config(entries, []), state);
    expect(existsSync(path.join(devinSkillsDir(state), "alpha"))).toBe(false);
  });

  /** An operator may drop a skill in by hand; pruning must not eat it. */
  it("leaves skills Paperclip does not know about alone", async () => {
    const { state, source } = setup();
    const manual = path.join(devinSkillsDir(state), "hand-rolled");
    mkdirSync(manual, { recursive: true });
    writeFileSync(path.join(manual, "SKILL.md"), "manual");

    await materializeSkills(
      config([{ name: "alpha", source: skillSource(source, "alpha") }], ["alpha"]),
      state,
    );

    expect(existsSync(path.join(manual, "SKILL.md"))).toBe(true);
  });

  /**
   * The source path is deliberately non-empty: an entry with an empty source is
   * dropped during normalisation and never reaches the adapter, so an empty
   * string would test nothing.
   */
  it("warns instead of failing when a source is missing", async () => {
    const { state } = setup();
    const result = await materializeSkills(
      {
        paperclipRuntimeSkills: [
          {
            key: "gone",
            runtimeName: "gone",
            source: path.join(scratch, "never-built"),
            sourceStatus: "missing",
            missingDetail: "not built",
          },
        ],
        paperclipSkillSync: { desiredSkills: ["gone"] },
      },
      state,
    );

    expect(result.installed).toEqual([]);
    expect(result.warnings[0]).toContain("gone");
  });

  it("puts the skills directory under the XDG config home it advertises", () => {
    const { state } = setup();
    expect(devinSkillsDir(state)).toBe(path.join(devinConfigHome(state), "devin", "skills"));
  });
});
