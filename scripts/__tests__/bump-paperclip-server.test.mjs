import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";

import { bumpPaperclipServer, bumpPatch } from "../bump-paperclip-server.mjs";

let scratch = null;

function makeRepo() {
  scratch = mkdtempSync(path.join(tmpdir(), "bump-server-"));
  mkdirSync(path.join(scratch, "adapters", "adapter-a"), { recursive: true });
  mkdirSync(path.join(scratch, "adapters", "adapter-b"), { recursive: true });

  writeJson(path.join(scratch, "package.json"), {
    name: "paperclip-desktop",
    version: "3.2.11",
    devDependencies: { "@paperclipai/server": "2026.817.0" },
  });
  writeJson(path.join(scratch, "adapters", "adapter-a", "package.json"), {
    name: "adapter-a",
    dependencies: { "@paperclipai/adapter-utils": "2026.817.0", lodash: "^4" },
    devDependencies: { "@paperclipai/plugin-sdk": "2026.817.0", vitest: "^3" },
  });
  writeJson(path.join(scratch, "adapters", "adapter-b", "package.json"), {
    name: "adapter-b",
    dependencies: { express: "^4" },
  });
  // A directory without a package.json must be skipped, not fatal.
  mkdirSync(path.join(scratch, "adapters", "no-pkg"), { recursive: true });

  return scratch;
}

function writeJson(file, value) {
  writeFileSync(file, JSON.stringify(value));
}

function readJson(file) {
  return JSON.parse(readFileSync(file, "utf8"));
}

afterEach(() => {
  if (scratch) rmSync(scratch, { recursive: true, force: true });
  scratch = null;
});

describe("bumpPaperclipServer", () => {
  it("moves the server pin, adapter pins, and the desktop patch version together", () => {
    const root = makeRepo();
    const result = bumpPaperclipServer("2026.1023.0", root);

    const rootPkg = readJson(path.join(root, "package.json"));
    assert.equal(rootPkg.devDependencies["@paperclipai/server"], "2026.1023.0");
    // The updater compares the desktop version — without a bump no client
    // would ever see the new server.
    assert.equal(rootPkg.version, "3.2.12");

    const adapterA = readJson(path.join(root, "adapters", "adapter-a", "package.json"));
    assert.equal(adapterA.dependencies["@paperclipai/adapter-utils"], "2026.1023.0");
    assert.equal(adapterA.devDependencies["@paperclipai/plugin-sdk"], "2026.1023.0");
    // Third-party deps are never the bump's business.
    assert.equal(adapterA.dependencies.lodash, "^4");
    assert.equal(adapterA.devDependencies.vitest, "^3");

    const adapterB = readJson(path.join(root, "adapters", "adapter-b", "package.json"));
    assert.equal(adapterB.dependencies.express, "^4");

    assert.deepEqual(result, {
      previousServer: "2026.817.0",
      server: "2026.1023.0",
      previousDesktop: "3.2.11",
      desktop: "3.2.12",
      changed: result.changed,
    });
    assert.equal(result.changed.length, 2);
  });

  /**
   * The daily workflow runs unconditionally; a same-version run must not
   * produce a phantom commit — and must NOT burn a desktop patch version.
   */
  it("is a no-op when the pin already matches", () => {
    const root = makeRepo();
    const before = readJson(path.join(root, "package.json"));

    const result = bumpPaperclipServer("2026.817.0", root);

    assert.deepEqual(result.changed, []);
    assert.deepEqual(readJson(path.join(root, "package.json")), before);
  });

  it("fails loudly when the server pin is absent", () => {
    scratch = mkdtempSync(path.join(tmpdir(), "bump-server-"));
    writeJson(path.join(scratch, "package.json"), { name: "x", version: "1.0.0" });
    mkdirSync(path.join(scratch, "adapters"), { recursive: true });

    assert.throws(() => bumpPaperclipServer("2026.1023.0", scratch), /@paperclipai\/server/);
  });
});

describe("bumpPatch", () => {
  it("bumps the patch component", () => {
    assert.equal(bumpPatch("3.2.11"), "3.2.12");
    assert.equal(bumpPatch("3.2.9"), "3.2.10");
  });
});
