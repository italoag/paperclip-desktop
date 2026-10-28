import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";

import {
  patchAllowlists,
  patchBundle,
} from "../patch-builtin-adapter-allowlist.mjs";

const REAL_SHAPE = `        allowedAdapterTypes: ["claude_local", "codex_local", "gemini_local", "opencode_local", "process"],`;

describe("patchAllowlists", () => {
  it("appends the adapter to an existing allowlist", () => {
    const { source, patched } = patchAllowlists(REAL_SHAPE, ["devin_local"]);

    assert.ok(String(source).includes('"devin_local"'));
    assert.equal(patched, 1);
  });

  /** Upstream's choices stay; we only widen. */
  it("keeps every adapter that was already allowed", () => {
    const { source } = patchAllowlists(REAL_SHAPE, ["devin_local"]);

    for (const kept of ["claude_local", "codex_local", "gemini_local", "opencode_local", "process"]) {
      assert.ok(String(source).includes(`"${kept}"`));
    }
  });

  it("is idempotent, so repeated packaging does not duplicate entries", () => {
    const once = patchAllowlists(REAL_SHAPE, ["devin_local"]).source;
    const twice = patchAllowlists(once, ["devin_local"]);

    assert.equal(twice.patched, 0);
    assert.equal(twice.source, once);
  });

  it("patches every built-in definition in the file", () => {
    const { found, patched } = patchAllowlists(
      [REAL_SHAPE, REAL_SHAPE, REAL_SHAPE].join("\n"),
      ["devin_local"],
    );

    assert.equal(found, 3);
    assert.equal(patched, 3);
  });

  it("leaves the file alone when no adapters are bundled", () => {
    const { source, patched } = patchAllowlists(REAL_SHAPE, []);

    assert.equal(source, REAL_SHAPE);
    assert.equal(patched, 0);
  });

  it("handles several bundled adapters at once", () => {
    const { source } = patchAllowlists(REAL_SHAPE, ["devin_local", "other_local"]);

    assert.ok(String(source).includes('"devin_local"'));
    assert.ok(String(source).includes('"other_local"'));
  });
});

describe("patchBundle", () => {
  let scratch;
  afterEach(() => scratch && rmSync(scratch, { recursive: true, force: true }));

  function bundle(contents) {
    scratch = mkdtempSync(path.join(tmpdir(), "allowlist-"));
    const dir = path.join(scratch, "dist", "services");
    mkdirSync(dir, { recursive: true });
    if (contents !== null) writeFileSync(path.join(dir, "built-in-agents.js"), contents);
    return scratch;
  }

  it("writes the patched file to disk", () => {
    const dir = bundle(REAL_SHAPE);
    patchBundle(dir, ["devin_local"]);

    const written = readFileSync(
      path.join(dir, "dist", "services", "built-in-agents.js"),
      "utf8",
    );
    assert.ok(String(written).includes('"devin_local"'));
  });

  /**
   * The failure mode that matters. A server upgrade that renames the field would
   * otherwise ship an app where the built-ins silently refuse our adapters
   * again — the exact bug this script exists to prevent.
   */
  it("fails loudly when upstream drops the field", () => {
    const dir = bundle("export const definitions = [{ defaultTitle: 'Summarizer' }];");

    assert.throws(() => patchBundle(dir, ["devin_local"]), /no "allowedAdapterTypes" arrays/);
  });

  it("fails loudly when the file moves", () => {
    const dir = bundle(null);

    assert.throws(() => patchBundle(dir, ["devin_local"]), /cannot read/);
  });
});
