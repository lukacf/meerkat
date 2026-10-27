/**
 * Tests for `MetaBuilder.declareSpecUpdate()`: the builder's projection of
 * the existing `runtime_options.declare_spec_update` gateway option with the
 * authoritative mob composition, its validation, and the exact wire form of
 * the expected revision (Python parity: test_builder.py).
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Meta, MetaRuntime, PersistentTransport } from "../dist/index.js";

const STATE = "/var/lib/meta-app";
const MOB_TOML = "[mob]\nid = 'app'\n";

/** A stand-in gateway that answers every JSON-RPC request with `{}`. */
function writeStandInGateway(dir: string): string {
  const script = join(dir, "stand_in_gateway.cjs");
  writeFileSync(
    script,
    [
      'const readline = require("node:readline");',
      "const rl = readline.createInterface({ input: process.stdin });",
      'rl.on("line", (line) => {',
      "  let request;",
      "  try { request = JSON.parse(line); } catch { return; }",
      '  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: {} }) + "\\n");',
      "});",
      'rl.on("close", () => process.exit(0));',
      "",
    ].join("\n"),
  );
  const gateway = join(dir, "stand_in_gateway.sh");
  writeFileSync(gateway, `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`, {
    mode: 0o755,
  });
  return gateway;
}

/** Records every request exactly as it is serialized for the gateway. */
class CapturingTransport extends PersistentTransport {
  readonly requests: Array<Record<string, any>> = [];

  override async sendAsync(
    request: Record<string, unknown>,
    options?: { timeoutMs?: number },
  ): Promise<unknown> {
    this.requests.push(JSON.parse(JSON.stringify(request)));
    return super.sendAsync(request, options);
  }
}

/** Build and connect through a capturing transport; return the sent meta/init. */
async function sentInit(
  configure: (builder: ReturnType<typeof Meta.builder>, dir: string) => unknown,
): Promise<Record<string, any>> {
  const dir = mkdtempSync(join(tmpdir(), "meta-spec-update-wire-"));
  const made: CapturingTransport[] = [];
  try {
    const builder = Meta.builder()
      .gateway(writeStandInGateway(dir))
      .gatewayTransport((args) => {
        const transport = new CapturingTransport(args.gatewayBin, { timeout: args.timeoutMs });
        made.push(transport);
        return transport;
      });
    configure(builder, dir);
    const runtime = await builder.build();
    try {
      assert.equal(made.length, 1);
      const init = made[0].requests.find((request) => request.method === "meta/init");
      assert.ok(init, "meta/init was sent");
      return init;
    } finally {
      await runtime.shutdown();
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function initParams(builder: unknown): Record<string, any> {
  const config = (builder as { _config: unknown })._config;
  const rt = new MetaRuntime(config as ConstructorParameters<typeof MetaRuntime>[0]);
  return (rt as any)._buildInitParams();
}

function validate(builder: unknown): void {
  (builder as { _validateConfig(): void })._validateConfig();
}

function withMobFile<T>(run: (path: string) => T): T {
  const dir = mkdtempSync(join(tmpdir(), "meta-spec-update-"));
  try {
    const path = join(dir, "mob.toml");
    writeFileSync(path, MOB_TOML);
    return run(path);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

describe("declareSpecUpdate()", () => {
  it("returns the builder and records the expected revision", () => {
    const builder = Meta.builder();
    assert.equal(builder.declareSpecUpdate(7), builder);
    assert.equal(builder._config.declareSpecUpdateExpectedRevision, 7);
  });

  it("defaults to omitted", () => {
    assert.equal(Meta.builder()._config.declareSpecUpdateExpectedRevision, null);
    const params = initParams(Meta.builder());
    assert.equal("declare_spec_update" in params.runtime_options, false);
    assert.equal("mob_composition" in params.runtime_options, false);
  });

  it("reaches the authoritative runtime options with the mob definition", () => {
    withMobFile((path) => {
      const builder = Meta.builder().mob(path).persistentState(STATE).declareSpecUpdate(7);
      validate(builder);

      const params = initParams(builder);

      assert.equal(params.mob_config, MOB_TOML);
      assert.equal(params.persistent_state, STATE);
      assert.deepEqual(params.runtime_options.mob_composition, { authority: "authoritative" });
      assert.deepEqual(params.runtime_options.declare_spec_update, { expected_revision: 7 });
    });
  });

  it("a prepared application supplies the declared update", () => {
    const builder = Meta.builder()
      .preparedApplication(new Uint8Array([1, 2, 3]), "sha256:abc")
      .persistentState(STATE)
      .declareSpecUpdate(7);
    validate(builder);

    const params = initParams(builder);

    assert.deepEqual(params.prepared_application, {
      bundle_base64: "AQID",
      digest: "sha256:abc",
    });
    assert.equal(params.mob_config, undefined);
    assert.deepEqual(params.runtime_options.mob_composition, { authority: "authoritative" });
    assert.deepEqual(params.runtime_options.declare_spec_update, { expected_revision: 7 });
  });

  it("passes the expected revision through exactly, including its wire form", () => {
    for (const revision of [0, 1, 7, Number.MAX_SAFE_INTEGER]) {
      const builder = Meta.builder()
        .preparedApplication(new Uint8Array([0]), "sha256:abc")
        .persistentState(STATE)
        .declareSpecUpdate(revision);
      validate(builder);

      const wire = JSON.parse(JSON.stringify(initParams(builder)));

      assert.deepEqual(wire.runtime_options.declare_spec_update, { expected_revision: revision });
      assert.deepEqual(wire.runtime_options.mob_composition, { authority: "authoritative" });
    }
  });

  it("the last declaration wins", () => {
    const builder = Meta.builder().declareSpecUpdate(3).declareSpecUpdate(4);
    assert.equal(builder._config.declareSpecUpdateExpectedRevision, 4);
  });

  for (const revision of [-1, -7]) {
    it(`rejects the negative revision ${revision}`, () => {
      assert.throws(
        () => Meta.builder().declareSpecUpdate(revision),
        (error: unknown) =>
          error instanceof RangeError && /non-negative integer/.test(error.message),
      );
    });
  }

  for (const revision of [true, 1.5, "7", null, undefined, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 53]) {
    it(`rejects the non-integer revision ${String(revision)}`, () => {
      assert.throws(
        () => Meta.builder().declareSpecUpdate(revision as never),
        (error: unknown) =>
          error instanceof TypeError && /non-negative integer/.test(error.message),
      );
    });
  }

  it("a refused revision leaves the configuration unchanged", () => {
    const builder = Meta.builder().declareSpecUpdate(5);
    assert.throws(() => builder.declareSpecUpdate(-1), RangeError);
    assert.throws(() => builder.declareSpecUpdate(1.5), TypeError);
    assert.equal(builder._config.declareSpecUpdateExpectedRevision, 5);
  });

  it("requires a replacement definition", () => {
    const builder = Meta.builder().persistentState(STATE).declareSpecUpdate(1);
    assert.throws(
      () => validate(builder),
      /declareSpecUpdate\(\) requires mob\(\) or preparedApplication\(\)/,
    );
  });

  it("requires durable state", () => {
    withMobFile((path) => {
      const builder = Meta.builder().mob(path).declareSpecUpdate(1);
      assert.throws(() => validate(builder), /declareSpecUpdate\(\) requires persistentState\(\)/);
    });
  });

  it("build() refuses an invalid declaration before starting anything", async () => {
    const builder = Meta.builder().declareSpecUpdate(1);
    await assert.rejects(builder.build(), /declareSpecUpdate\(\) requires mob\(\)/);
  });

  it("carries the expected revision end to end in the sent meta/init", async () => {
    for (const revision of [0, 42, Number.MAX_SAFE_INTEGER]) {
      const init = await sentInit((builder, dir) =>
        builder
          .preparedApplication(new Uint8Array([1, 2, 3]), "sha256:abc")
          .persistentState(join(dir, "state"))
          .declareSpecUpdate(revision),
      );
      assert.deepEqual(init.params.runtime_options.mob_composition, { authority: "authoritative" });
      assert.deepEqual(init.params.runtime_options.declare_spec_update, {
        expected_revision: revision,
      });
      assert.equal(typeof init.params.persistent_state, "string");
      assert.deepEqual(init.params.prepared_application, {
        bundle_base64: "AQID",
        digest: "sha256:abc",
      });
    }
  });

  it("sends neither option in meta/init without a declaration", async () => {
    const init = await sentInit((builder, dir) =>
      builder
        .preparedApplication(new Uint8Array([1, 2, 3]), "sha256:abc")
        .persistentState(join(dir, "state")),
    );
    assert.equal("declare_spec_update" in init.params.runtime_options, false);
    assert.equal("mob_composition" in init.params.runtime_options, false);
  });
});
