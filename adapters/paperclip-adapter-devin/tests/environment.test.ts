import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import { afterAll, describe, expect, it } from "vitest";

import { checkCwd } from "../src/server/test.js";

const scratch = mkdtempSync(path.join(tmpdir(), "devin-adapter-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));

describe("checkCwd", () => {
  /**
   * Regression: an earlier version treated an empty cwd as an error, which
   * failed the environment test for every agent that relies on Paperclip's
   * execution workspace — the normal case.
   */
  it("accepts an empty cwd, because the execution workspace supplies one", async () => {
    const check = await checkCwd("");
    expect(check.level).toBe("info");
    expect(check.code).toBe("cwd_from_workspace");
  });

  it("rejects a relative path", async () => {
    const check = await checkCwd("relative/path");
    expect(check.level).toBe("error");
    expect(check.code).toBe("cwd_relative");
  });

  it("rejects a path that does not exist", async () => {
    const check = await checkCwd(path.join(scratch, "nope"));
    expect(check.level).toBe("error");
    expect(check.code).toBe("cwd_absent");
  });

  it("rejects a path that is a file", async () => {
    const file = path.join(scratch, "a-file");
    writeFileSync(file, "");
    const check = await checkCwd(file);
    expect(check.level).toBe("error");
    expect(check.code).toBe("cwd_not_a_directory");
  });

  it("accepts an existing absolute directory", async () => {
    const check = await checkCwd(scratch);
    expect(check.level).toBe("info");
    expect(check.code).toBe("cwd_ok");
  });
});
