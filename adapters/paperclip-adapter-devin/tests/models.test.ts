import { beforeEach, describe, expect, it } from "vitest";

import { DEVIN_MODELS } from "../src/models.js";
import {
  discoverDevinModels,
  listDevinModels,
  loadDevinModelCatalogue,
  parseDevinModelsJson,
  parseDevinModelsText,
  refreshDevinModels,
  resetDevinModelsCacheForTests,
  type ModelsListRunner,
} from "../src/server/models.js";
import { getConfigSchema } from "../src/server/schema.js";

const JSON_OUTPUT = JSON.stringify({
  families: [
    {
      family_label: "SWE-2",
      family_uid: "swe-2",
      slug: "swe-2",
      aliases: ["swe"],
      variants: [
        { model_uid: "swe-2-high", label: "SWE-2 High" },
        { model_uid: "swe-2-medium", label: "SWE-2 Medium" },
      ],
    },
    {
      family_label: "Claude Fable 5.1",
      family_uid: "claude-fable-5.1",
      slug: "claude-fable-5-1",
      aliases: ["fable"],
      variants: [
        { model_uid: "claude-fable-5-1-high", label: "Claude Fable 5.1 High" },
        // Malformed rows are dropped, not fatal.
        { label: "no uid" },
        "garbage",
      ],
    },
  ],
});

const TEXT_OUTPUT = [
  "Available models (2 families)",
  "",
  "SWE-2 (swe-2)",
  "  aliases: swe",
  "  swe-2-high      SWE-2 High  [262K context, $0.5 / 1M Input]",
  "  swe-2-medium    SWE-2 Medium",
  "",
  "Claude Fable 5.1 (claude-fable-5.1)",
  "  claude-fable-5-1-high   Claude Fable 5.1 High  [1M context]",
  "",
].join("\n");

/** A runner that answers the JSON probe and records every invocation. */
function runnerReturning(stdout: string) {
  const calls: string[][] = [];
  const run: ModelsListRunner = async (_command, args) => {
    calls.push(args);
    return { ok: true, stdout, stderr: "" };
  };
  return { calls, run };
}

function runnerFailing(stderr = "Error: Not logged in. Run `devin auth login` to authenticate.") {
  const calls: string[][] = [];
  const run: ModelsListRunner = async (_command, args) => {
    calls.push(args);
    return { ok: false, stdout: "", stderr };
  };
  return { calls, run };
}

beforeEach(() => resetDevinModelsCacheForTests());

describe("parseDevinModelsJson", () => {
  it("flattens families into labelled, grouped models", () => {
    expect(parseDevinModelsJson(JSON_OUTPUT)).toEqual([
      { id: "swe-2-high", label: "SWE-2 High", family: "SWE-2" },
      { id: "swe-2-medium", label: "SWE-2 Medium", family: "SWE-2" },
      { id: "claude-fable-5-1-high", label: "Claude Fable 5.1 High", family: "Claude Fable 5.1" },
    ]);
  });

  it("dedupes repeated ids", () => {
    const dupes = JSON.stringify({
      families: [
        { family_label: "F", variants: [{ model_uid: "m1" }, { model_uid: "m1", label: "again" }] },
      ],
    });
    expect(parseDevinModelsJson(dupes)).toEqual([{ id: "m1", label: "m1", family: "F" }]);
  });
});

describe("parseDevinModelsText", () => {
  it("reads the default text format, skipping aliases and metadata brackets", () => {
    expect(parseDevinModelsText(TEXT_OUTPUT)).toEqual([
      { id: "swe-2-high", label: "SWE-2 High", family: "SWE-2" },
      { id: "swe-2-medium", label: "SWE-2 Medium", family: "SWE-2" },
      { id: "claude-fable-5-1-high", label: "Claude Fable 5.1 High", family: "Claude Fable 5.1" },
    ]);
  });
});

describe("discoverDevinModels", () => {
  it("prefers the JSON format", async () => {
    const { calls, run } = runnerReturning(JSON_OUTPUT);
    const models = await discoverDevinModels({ command: "devin-test", run });
    expect(models.map((m) => m.id)).toContain("swe-2-high");
    expect(calls).toEqual([["models", "list", "--format", "json"]]);
  });

  it("falls back to the text format when --format json is unsupported", async () => {
    const calls: string[][] = [];
    const run: ModelsListRunner = async (_command, args) => {
      calls.push(args);
      if (args.includes("--format")) {
        return { ok: false, stdout: "", stderr: "unknown option --format" };
      }
      return { ok: true, stdout: TEXT_OUTPUT, stderr: "" };
    };

    const models = await discoverDevinModels({ command: "devin-test", run });
    expect(models.map((m) => m.id)).toEqual([
      "swe-2-high",
      "swe-2-medium",
      "claude-fable-5-1-high",
    ]);
    expect(calls).toEqual([
      ["models", "list", "--format", "json"],
      ["models", "list"],
    ]);
  });

  it("throws when the CLI cannot produce a catalogue (e.g. logged out)", async () => {
    const { run } = runnerFailing();
    await expect(discoverDevinModels({ command: "devin-test", run })).rejects.toThrow(
      /devin models list` failed/,
    );
  });
});

describe("loadDevinModelCatalogue", () => {
  it("serves the live catalogue and caches it per command", async () => {
    const { calls, run } = runnerReturning(JSON_OUTPUT);
    const first = await loadDevinModelCatalogue({ command: "devin-test", run });
    const second = await loadDevinModelCatalogue({ command: "devin-test", run });

    expect(first).toBe(second);
    expect(calls).toHaveLength(1);
  });

  it("serves the packaged snapshot when the probe fails", async () => {
    const { run } = runnerFailing();
    const catalogue = await loadDevinModelCatalogue({ command: "devin-test", run });
    expect(catalogue.map((m) => m.id)).toEqual(DEVIN_MODELS.map((m) => m.id));
  });

  it("re-probes after forceRefresh", async () => {
    const { calls, run } = runnerReturning(JSON_OUTPUT);
    await loadDevinModelCatalogue({ command: "devin-test", run });
    await loadDevinModelCatalogue({ command: "devin-test", run, forceRefresh: true });
    expect(calls).toHaveLength(2);
  });
});

describe("registry hooks", () => {
  it("listModels and refreshModels return id/label pairs", async () => {
    const models = await listDevinModels();
    expect(models.length).toBeGreaterThan(0);
    for (const model of models) {
      expect(Object.keys(model).sort()).toEqual(["id", "label"]);
    }
    const refreshed = await refreshDevinModels();
    expect(refreshed.length).toBeGreaterThan(0);
  });
});

describe("getConfigSchema", () => {
  function modelField(schema: { fields: { key: string }[] }) {
    const field = schema.fields.find((f) => f.key === "model");
    if (!field || !("options" in field) || !Array.isArray(field.options)) {
      throw new Error("model field missing options");
    }
    return field as { options: { value: string; label: string; group?: string }[] };
  }

  it("fills the model combobox from live discovery, grouped by family", async () => {
    const { run } = runnerReturning(JSON_OUTPUT);
    const schema = await getConfigSchema({ command: "devin-test", run });
    const field = modelField(schema);

    expect(field.options).toEqual([
      { value: "swe-2-high", label: "SWE-2 High", group: "SWE-2" },
      { value: "swe-2-medium", label: "SWE-2 Medium", group: "SWE-2" },
      { value: "claude-fable-5-1-high", label: "Claude Fable 5.1 High", group: "Claude Fable 5.1" },
    ]);
  });

  it("still renders when the CLI cannot be asked", async () => {
    const { run } = runnerFailing();
    const schema = await getConfigSchema({ command: "devin-test", run });
    const field = modelField(schema);

    expect(field.options.length).toBe(DEVIN_MODELS.length);
  });
});
