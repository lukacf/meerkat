import assert from "node:assert/strict";
import { test } from "node:test";
import { Meta, MetaRuntime, type MetaBuilder } from "../dist/index.js";

const registration = {
  principal: "alice@example.com",
  realm: "ops",
  authBinding: { realm: "ops", binding: "openai", profile: "console" },
  voice: "marin",
  sessionInstructions: "Be concise.",
};

function runtimeOptions(builder: MetaBuilder): object {
  const runtime = new MetaRuntime(builder._config);
  const buildInit: unknown = Reflect.get(runtime, "_buildInitParams");
  assert.ok(typeof buildInit === "function");
  const params: unknown = buildInit.call(runtime);
  assert.ok(params && typeof params === "object" && "runtime_options" in params);
  const options = params.runtime_options;
  assert.ok(options && typeof options === "object");
  return options;
}

test("console voice remains omitted until explicitly registered", () => {
  const builder = Meta.builder();
  assert.equal(builder._config.consoleVoiceConfig, null);
  assert.equal("console_voice" in runtimeOptions(builder), false);
});

test("console voice serializes the authenticated HTTP registration without enabling stdio live", () => {
  const builder = Meta.builder();
  assert.equal(builder.consoleVoice(registration), builder);
  const options = runtimeOptions(builder);
  assert.ok("console_voice" in options);
  assert.deepEqual(options.console_voice, {
    principal: "alice@example.com",
    realm: "ops",
    auth_binding: { realm: "ops", binding: "openai", profile: "console" },
    voice: "marin",
    session_instructions: "Be concise.",
  });
  assert.equal("openai_live" in options, false);
  assert.equal("experimental_live" in options, false);
});

test("console voice reuses strict public binding validation", () => {
  assert.throws(
    () => runtimeOptions(Meta.builder().consoleVoice({
      ...registration,
      authBinding: { ...registration.authBinding, realm: "another-realm" },
    })),
    /realm/,
  );
  assert.throws(
    () => runtimeOptions(Meta.builder().consoleVoice(
      Object.assign({}, registration, { apiKey: "invalid-fixture-only" }),
    )),
    /unknown field apiKey/,
  );
});
