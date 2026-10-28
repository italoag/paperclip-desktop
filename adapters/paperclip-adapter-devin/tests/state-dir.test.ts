import { describe, expect, it } from "vitest";

import {
  acpxDefaultStateDir,
  resolveStateDir,
  stateDirHasSpace,
} from "../src/server/state-dir.js";

const base = { companyId: "c1", agentId: "a1", home: "/Users/me" };

describe("acpxDefaultStateDir", () => {
  it("mirrors acpx_local's layout", () => {
    expect(acpxDefaultStateDir({ ...base, paperclipHome: "/srv/pc" })).toBe(
      "/srv/pc/instances/default/companies/c1/acpx-local/agents/a1",
    );
  });

  it("falls back to ~/.paperclip like acpx_local does", () => {
    expect(acpxDefaultStateDir(base)).toContain("/Users/me/.paperclip/instances/default");
  });
});

describe("resolveStateDir", () => {
  /**
   * The real-world failure: Paperclip's macOS home is
   * ~/Library/Application Support/Paperclip, and ACPX splits the wrapper path on
   * the space, spawning "/Users/me/Library/Application".
   */
  it("relocates away from a home containing a space", () => {
    const resolved = resolveStateDir({
      ...base,
      paperclipHome: "/Users/me/Library/Application Support/Paperclip",
    });
    expect(resolved).toBe(
      "/Users/me/.paperclip-adapter-devin/instances/default/companies/c1/agents/a1",
    );
    expect(resolved).not.toContain(" ");
  });

  it("leaves acpx_local's default alone when it has no space", () => {
    expect(resolveStateDir({ ...base, paperclipHome: "/srv/pc" })).toBeUndefined();
  });

  it("honours an explicitly configured stateDir, space or not", () => {
    expect(
      resolveStateDir({
        ...base,
        configured: "/my/state dir",
        paperclipHome: "/Users/me/Library/Application Support/Paperclip",
      }),
    ).toBeUndefined();
  });

  it("gives up when the home directory itself has a space", () => {
    expect(
      resolveStateDir({
        ...base,
        home: "/Users/my name",
        paperclipHome: "/Users/my name/Library/Application Support/Paperclip",
      }),
    ).toBeUndefined();
  });

  it("keeps the instance id so instances stay isolated", () => {
    expect(
      resolveStateDir({
        ...base,
        instanceId: "staging",
        paperclipHome: "/Users/me/Library/Application Support/Paperclip",
      }),
    ).toContain("/instances/staging/");
  });
});

describe("stateDirHasSpace", () => {
  it("is false once the relocation applies", () => {
    expect(
      stateDirHasSpace({
        ...base,
        paperclipHome: "/Users/me/Library/Application Support/Paperclip",
      }),
    ).toBe(false);
  });

  it("is true for an operator-chosen path with a space", () => {
    expect(stateDirHasSpace({ ...base, configured: "/my/state dir" })).toBe(true);
  });

  it("is true when nothing can be done about the home directory", () => {
    expect(
      stateDirHasSpace({
        ...base,
        home: "/Users/my name",
        paperclipHome: "/Users/my name/Library/Application Support/Paperclip",
      }),
    ).toBe(true);
  });
});
