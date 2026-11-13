/**
 * `send(identity, content, { requester })`: a person the host attests as the
 * original requester of the content it relays. A gateway that predates the
 * field would ignore it and send without a requester, so the SDK refuses one,
 * typed, unless the gateway advertises the `send_host_attested_requester`
 * feature capability. A requester does not combine with `idempotencyKey`.
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { CapabilityUnavailableError } from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

const PERSON = { kind: "person", id: "person-7" } as const;

async function makeRuntime(serves: boolean) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params: Record<string, unknown> = {},
  ) => {
    calls.push({ method, params });
    if (method === "meta/capabilities") {
      return {
        contract_version: "0.5.0",
        methods: ["meta/send"],
        feature_capabilities: serves ? ["send_host_attested_requester"] : [],
        loaded_modules: [],
      };
    }
    if (method === "meta/send") return { fencing_token: 1 };
    return {};
  };
  const sent = (name: string) => calls.filter((c) => c.method === name).map((c) => c.params);
  return { rt, calls, sent };
}

describe("send with a host-attested requester", () => {
  it("forwards the requester to a gateway that serves it", async () => {
    const { rt, sent } = await makeRuntime(true);
    await rt.send("keeper", "hello", { requester: PERSON, trackTurn: true });
    assert.deepEqual(sent("meta/send"), [
      { identity: "keeper", content: "hello", track_turn: true, requester: PERSON },
    ]);
  });

  it("leaves a send without a requester unchanged", async () => {
    const { rt, sent } = await makeRuntime(true);
    await rt.send("keeper", "hello");
    assert.deepEqual(sent("meta/send"), [{ identity: "keeper", content: "hello" }]);
    assert.deepEqual(sent("meta/capabilities"), []);
  });

  it("refuses the requester, typed, on a gateway that does not serve it", async () => {
    const { rt, sent } = await makeRuntime(false);
    await assert.rejects(
      rt.send("keeper", "hello", { requester: PERSON }),
      CapabilityUnavailableError,
    );
    assert.deepEqual(sent("meta/send"), []);
  });

  it("refuses a malformed requester before any call", async () => {
    for (const requester of [
      { kind: "agent", id: "person-7" },
      { kind: "person" },
      { kind: "person", id: "" },
      { kind: "person", id: "person-7", display_name: "P" },
    ]) {
      const { rt, calls } = await makeRuntime(true);
      await assert.rejects(
        rt.send("keeper", "hello", { requester: requester as never }),
        TypeError,
      );
      assert.deepEqual(calls, []);
    }
  });

  it("refuses a requester with a key", async () => {
    const { rt, calls } = await makeRuntime(true);
    await assert.rejects(
      rt.send("keeper", "hello", {
        requester: PERSON,
        idempotencyKey: "k1",
      } as never),
      /does not combine with requester/,
    );
    assert.deepEqual(calls, []);
  });
});
