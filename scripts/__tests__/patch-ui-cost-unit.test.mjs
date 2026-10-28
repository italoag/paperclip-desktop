import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { afterEach, describe, it } from "node:test";

import { patchCostUnit, patchUiDist } from "../patch-ui-cost-unit.mjs";

/** Verbatim from the shipped bundle. */
const FORMATTER =
  'function Lr(e){return`$${(e/100).toLocaleString("en-US",{minimumFractionDigits:2,maximumFractionDigits:2})}`}';
const BUDGET_INPUT = 's.jsx("span",{className:"text-sm",children:"$"}),s.jsx(Lt,{type:"number"})';
const BUNDLE = `${FORMATTER};${BUDGET_INPUT};`;

describe("patchCostUnit", () => {
  it("replaces the dollar sign with the unit suffix", () => {
    const { source } = patchCostUnit(BUNDLE);

    assert.ok(source.includes(')} ACU`}'));
    assert.ok(!source.includes("return`$${"));
  });

  it("keeps the two-decimal formatting the ledger's cents need", () => {
    const { source } = patchCostUnit(BUNDLE);

    assert.ok(source.includes("minimumFractionDigits:2,maximumFractionDigits:2"));
  });

  it("relabels the budget input prefix", () => {
    const { source } = patchCostUnit(BUNDLE);

    assert.ok(source.includes('children:"ACU"})'));
  });

  /** Minified names change every release, so matching must be shape-based. */
  it("matches regardless of the minified function name", () => {
    const renamed = BUNDLE.replace(/Lr/g, "q7$x");
    const { formatter } = patchCostUnit(renamed);

    assert.equal(formatter, 1);
  });

  it("reports when the formatter is absent instead of guessing", () => {
    const { formatter } = patchCostUnit("function other(e){return e}");

    assert.equal(formatter, 0);
  });

  it("accepts a custom unit", () => {
    const { source } = patchCostUnit(BUNDLE, "credits");

    assert.ok(source.includes(")} credits`}"));
  });
});

describe("patchUiDist", () => {
  let scratch;
  afterEach(() => scratch && rmSync(scratch, { recursive: true, force: true }));

  function uiDist(contents) {
    scratch = mkdtempSync(path.join(tmpdir(), "ui-unit-"));
    const assets = path.join(scratch, "assets");
    mkdirSync(assets, { recursive: true });
    writeFileSync(path.join(assets, "index-abc123.js"), contents);
    // A lazy chunk importing the shared bundle, as Vite emits ~65 of.
    writeFileSync(
      path.join(assets, "lazy-chunk.js"),
      'import{x}from"./index-abc123.js";',
    );
    writeFileSync(
      path.join(scratch, "index.html"),
      '<script type="module" src="/assets/index-abc123.js"></script>',
    );
    return scratch;
  }

  it("writes the relabelled bundle", () => {
    const dir = uiDist(BUNDLE);
    const { newName } = patchUiDist(dir);

    const written = readFileSync(path.join(dir, "assets", newName), "utf8");
    assert.ok(written.includes(")} ACU`}"));
  });

  /**
   * Assets are served `immutable` for a year, and Vite's hash comes from the
   * upstream content, so patching in place would leave installed webviews on
   * the old bytes forever. The rename is the whole reason the fix reaches users.
   */
  it("renames the bundle so the cached URL cannot be reused", () => {
    const dir = uiDist(BUNDLE);
    const { oldName, newName } = patchUiDist(dir);

    assert.notEqual(oldName, newName);
    assert.ok(!existsSync(path.join(dir, "assets", oldName)));
  });

  it("repoints index.html and the lazy chunks at the new name", () => {
    const dir = uiDist(BUNDLE);
    const { newName, rewritten } = patchUiDist(dir);

    assert.equal(rewritten, 2);
    assert.ok(readFileSync(path.join(dir, "index.html"), "utf8").includes(newName));
    assert.ok(
      readFileSync(path.join(dir, "assets", "lazy-chunk.js"), "utf8").includes(newName),
    );
  });

  it("is idempotent across repeated packaging", () => {
    const dir = uiDist(BUNDLE);
    patchUiDist(dir);
    const second = patchUiDist(dir);

    assert.equal(second.alreadyPatched, true);
  });

  /**
   * The failure that matters: an upstream change would otherwise ship a bundle
   * that silently puts dollar signs back on ACU figures.
   */
  it("fails loudly when the formatter changed shape", () => {
    const dir = uiDist('function Lr(e){return`$${e/100}`}');

    assert.throws(() => patchUiDist(dir), /expected exactly one cents formatter/);
  });

  it("fails loudly when the budget prefix is gone", () => {
    const dir = uiDist(FORMATTER);

    assert.throws(() => patchUiDist(dir), /expected exactly one budget/);
  });
});
