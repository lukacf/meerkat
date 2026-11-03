/**
 * A host turn authored from the identity's registered row: the SDK reads the
 * full row (`meta/registered_spec`), forwards its digest on `meta/send` as
 * `expected_spec_digest`, and types the gateway's refusal (-32006).
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  REGISTERED_SPEC_CHANGED_CODE,
  RegisteredSpecChangedError,
  RpcError,
} from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

async function makeRuntime(answers: Record<string, Record<string, unknown>>) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params: Record<string, unknown>,
  ) => {
    calls.push({ method, params });
    return answers[method] ?? {};
  };
  return { rt, calls };
}

describe("registered row", () => {
  it("reads the full registered row and its digest", async () => {
    const { rt, calls } = await makeRuntime({
      "meta/registered_spec": {
        identity: "keeper",
        spec: { identity: "keeper", additional_instructions: ["Be brief."] },
        spec_digest: "sha256:aaaa",
      },
    });
    const row = await rt.registeredSpec("keeper");
    assert.equal(row.identity, "keeper");
    assert.deepEqual(row.spec.additional_instructions, ["Be brief."]);
    assert.equal(row.specDigest, "sha256:aaaa");
    assert.deepEqual(calls, [
      { method: "meta/registered_spec", params: { identity: "keeper" } },
    ]);
  });

  it("forwards the expected row digest only when given", async () => {
    const { rt, calls } = await makeRuntime({
      "meta/send": { fencing_token: 1 },
    });
    await rt.send("keeper", "current", { expectedSpecDigest: "sha256:bbbb" });
    await rt.send("keeper", "plain");
    assert.equal(calls[0].params.expected_spec_digest, "sha256:bbbb");
    assert.equal("expected_spec_digest" in calls[1].params, false);
  });

  it("RegisteredSpecChangedError carries the -32006 refusal code", () => {
    const err = new RegisteredSpecChangedError("changed", "rid", "meta/send", {
      kind: "registered_spec_changed",
      submitted: false,
    });
    assert.ok(err instanceof RpcError);
    assert.equal(err.code, REGISTERED_SPEC_CHANGED_CODE);
    assert.equal(REGISTERED_SPEC_CHANGED_CODE, -32006);
    assert.equal(err.name, "RegisteredSpecChangedError");
  });
});
