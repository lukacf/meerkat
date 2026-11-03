/**
 * Installed-artifact attribution for SDK callback tools: the host names the
 * artifacts it installed (`runtime_options.installed_artifacts` at init), and
 * a callback tool's artifact reference crosses the build wire, where the
 * gateway verifies it before the build publishes any tool.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  Meta,
  MetaRuntime,
  SessionBuildOptions,
  parseAgentBuildDraft,
  agentBuildDraftToDict,
  parseExternalToolDef,
} from "../dist/index.js";

const WEATHER = { artifactId: "ext.weather", digest: "sha256:aa" };

describe("tool artifact provenance", () => {
  it("registerTool carries the artifact on the build wire", () => {
    const opts = new SessionBuildOptions();
    opts.registerTool("forecast", (args) => args, { artifact: WEATHER });
    assert.deepEqual(opts.toDict().tools, [
      { name: "forecast", artifact: { artifact_id: "ext.weather", digest: "sha256:aa" } },
    ]);
  });

  it("a tool without an artifact keeps its wire shape", () => {
    const opts = new SessionBuildOptions();
    opts.registerTool("plain", (args) => args);
    assert.deepEqual(opts.toDict().tools, ["plain"]);
  });

  it("the customizer draft round-trips the artifact", () => {
    const draft = parseAgentBuildDraft({});
    draft.registerTool("forecast", (args) => args, "Forecast", { type: "object" }, WEATHER);
    const wire = agentBuildDraftToDict(draft).external_tools as Record<string, unknown>[];
    assert.deepEqual(wire[0].artifact, { artifact_id: "ext.weather", digest: "sha256:aa" });
    assert.deepEqual(parseExternalToolDef(wire[0]).artifact, WEATHER);
    assert.equal(parseExternalToolDef({ name: "p", description: "", input_schema: {} }).artifact, undefined);
  });

  it("a replaced handler never inherits the previous artifact", () => {
    const opts = new SessionBuildOptions();
    opts.registerTool("forecast", () => 1, { artifact: WEATHER, description: "old" });
    const replacement = () => 2;
    opts.registerTool("forecast", replacement);
    assert.deepEqual(opts.toDict().tools, ["forecast"]);
    assert.equal(opts.toolHandlers.get("forecast"), replacement);

    const draft = parseAgentBuildDraft({});
    draft.registerTool("forecast", () => 1, { artifact: WEATHER });
    draft.registerTool("forecast", () => 2);
    assert.deepEqual(draft.externalTools.map((tool) => tool.name), ["forecast"]);
    assert.equal(draft.externalTools[0].artifact, undefined);
  });

  it("the draft accepts the options form and the positional form alike", () => {
    const draft = parseAgentBuildDraft({});
    draft.registerTool("a", () => 1, { description: "A", artifact: WEATHER });
    draft.registerTool("b", () => 1, "B", { type: "object" }, WEATHER);
    assert.deepEqual(
      draft.externalTools.map((tool) => [tool.name, tool.description, tool.artifact]),
      [
        ["a", "A", WEATHER],
        ["b", "B", WEATHER],
      ],
    );
  });

  it("a malformed replacement is refused and leaves the prior registration intact", () => {
    const bad = { artifactId: "ext", digest: "" };
    const original = () => 1;

    const opts = new SessionBuildOptions();
    opts.registerTool("forecast", original, { artifact: WEATHER, description: "old" });
    assert.throws(() => opts.registerTool("forecast", () => 2, { artifact: bad }));
    assert.equal(opts.toolHandlers.get("forecast"), original);
    assert.deepEqual(opts.toDict().tools, [
      {
        name: "forecast",
        description: "old",
        artifact: { artifact_id: "ext.weather", digest: "sha256:aa" },
      },
    ]);

    const draft = parseAgentBuildDraft({});
    draft.registerTool("forecast", original, { description: "old", artifact: WEATHER });
    assert.throws(() => draft.registerTool("forecast", () => 2, { artifact: bad }));
    assert.throws(() => draft.registerTool("forecast", () => 2, "new", { type: "object" }, bad));
    assert.equal(draft.toolHandlers.get("forecast"), original);
    assert.deepEqual(
      draft.externalTools.map((tool) => [tool.name, tool.description, tool.artifact]),
      [["forecast", "old", WEATHER]],
    );
  });

  it("malformed references are refused", () => {
    for (const bad of [
      { artifactId: "", digest: "d" },
      { artifactId: "ext", digest: "" },
      { artifactId: "ext weather", digest: "d" },
      { artifactId: "ext", digest: "d\n" },
      { artifactId: "x".repeat(257), digest: "d" },
    ]) {
      assert.throws(
        () => new SessionBuildOptions().registerTool("t", (a) => a, { artifact: bad }),
        JSON.stringify(bad),
      );
    }
  });

  it("installed artifacts ride init params, and are omitted when unset", () => {
    const builder = Meta.builder().installedArtifacts([WEATHER]);
    const params = (new MetaRuntime((builder as any)._config) as any)._buildInitParams();
    assert.deepEqual(params.runtime_options.installed_artifacts, [
      { artifact_id: "ext.weather", digest: "sha256:aa" },
    ]);
    const plain = (new MetaRuntime((Meta.builder() as any)._config) as any)._buildInitParams();
    assert.equal(plain.runtime_options?.installed_artifacts, undefined);
  });

  it("one artifact with two digests is refused", () => {
    assert.throws(() =>
      Meta.builder().installedArtifacts([WEATHER, { artifactId: "ext.weather", digest: "sha256:bb" }]),
    );
  });
});
