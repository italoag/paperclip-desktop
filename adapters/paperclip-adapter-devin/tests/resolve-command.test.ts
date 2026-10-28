import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterAll, describe, expect, it } from "vitest";

import { resolveCommandPath } from "../src/server/resolve-command.js";
import { buildAgentCommand } from "../src/config.js";

const scratch = mkdtempSync(path.join(tmpdir(), "devin-resolve-"));
const binDir = path.join(scratch, "bin");
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

function fakeBinary(name: string, mode = 0o755): string {
  const file = path.join(binDir, name);
  writeFileSync(file, "#!/bin/sh\n");
  chmodSync(file, mode);
  return file;
}

describe("resolveCommandPath", () => {
  it("finds an executable on PATH and returns it absolute", () => {
    require("node:fs").mkdirSync(binDir, { recursive: true });
    const expected = fakeBinary("devin");
    expect(resolveCommandPath("devin", { pathEnv: binDir })).toBe(expected);
  });

  /**
   * Devin installs to ~/.local/bin, which a GUI-launched app does not have on
   * PATH — the exact reason runs failed with "devin: not found".
   */
  it("falls back to ~/.local/bin when PATH does not cover it", () => {
    const home = mkdtempSync(path.join(tmpdir(), "devin-home-"));
    const localBin = path.join(home, ".local", "bin");
    require("node:fs").mkdirSync(localBin, { recursive: true });
    const file = path.join(localBin, "devin");
    writeFileSync(file, "#!/bin/sh\n");
    chmodSync(file, 0o755);

    expect(resolveCommandPath("devin", { pathEnv: "/usr/bin:/bin", home })).toBe(file);
    rmSync(home, { recursive: true, force: true });
  });

  it("ignores a non-executable file of the same name", () => {
    fakeBinary("not-exec", 0o644);
    expect(resolveCommandPath("not-exec", { pathEnv: binDir })).toBe("not-exec");
  });

  it("returns the input unchanged when nothing is found", () => {
    expect(resolveCommandPath("definitely-absent", { pathEnv: binDir, home: "/nonexistent" }))
      .toBe("definitely-absent");
  });

  it("leaves an absolute path alone", () => {
    expect(resolveCommandPath("/opt/devin/devin", { pathEnv: binDir })).toBe("/opt/devin/devin");
  });
});

describe("buildAgentCommand with a resolved binary", () => {
  it("bakes the absolute path into the command ACPX runs", () => {
    expect(buildAgentCommand({ model: "opus" }, "/Users/me/.local/bin/devin")).toBe(
      "/Users/me/.local/bin/devin acp --model opus",
    );
  });

  it("quotes a resolved path containing spaces", () => {
    expect(buildAgentCommand({}, "/Applications/My Tools/devin")).toBe(
      "'/Applications/My Tools/devin' acp",
    );
  });

  it("falls back to the configured command when resolution returned nothing", () => {
    expect(buildAgentCommand({ command: "devin" }, "")).toBe("devin acp");
  });
});
