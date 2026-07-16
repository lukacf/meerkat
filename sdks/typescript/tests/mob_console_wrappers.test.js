/**
 * Phase 7 (T-A9): the multi-host console wrappers issue the exact RPC
 * method literals with snake_case params and parse canned results; the
 * live wrappers never touch console logging (token opacity, ADJ-P6B-14).
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { Mob } from "../dist/mob.js";

const CANNED = {
  "mob/grant_scopes": {
    record: {
      principal: "operator-1",
      scopes: ["read_history", "admin_host"],
      expires_at_ms: 1_750_000_000_000,
    },
  },
  "mob/revoke_scopes": { removed: false },
  "mob/grants": {
    grants: [
      {
        principal: "operator-1",
        scopes: ["read_history", "admin_host"],
        expires_at_ms: 1_750_000_000_000,
      },
    ],
  },
  "mob/member_history": {
    page: {
      from_index: 0,
      messages: [
        {
          role: "user",
          created_at: "2026-07-10T00:00:00Z",
          content: "hello",
        },
      ],
      message_count: 2,
      next_index: 1,
      complete: false,
    },
    generation: 1,
    provenance: "controlling_host_verified",
  },
  "mob/hosts": {
    hosts: [
      {
        host_id: "host-peer-1",
        endpoint: "tcp://10.0.0.2:7100",
        bind_phase: "bound",
        authority_epoch: 3,
        capabilities: {
          protocol_min: 2,
          protocol_max: 2,
          engine_version: "0.7.26",
          durable_sessions: false,
          autonomous_members: true,
          hard_cancel_member: true,
          tracked_input_cancel: true,
          memory_store: false,
          mcp: false,
          approval_forwarding: false,
        },
        materialized_member_count: 1,
      },
    ],
  },
  "mob/route_installs": { outstanding: [], complete: true },
  "mob/bind_host": {
    host_id: "host-peer-1",
    capabilities: {
      protocol_min: 2,
      protocol_max: 2,
      engine_version: "0.7.26",
      durable_sessions: true,
      autonomous_members: true,
      hard_cancel_member: true,
      tracked_input_cancel: true,
      memory_store: true,
      mcp: true,
      approval_forwarding: false,
    },
    authority_epoch: 1,
  },
  "mob/revoke_host": { host_id: "host-peer-1", released_members: ["b2"] },
  "mob/hard_cancel_member": { cancelled: true },
  "mob/member_live_open": {
    channel_id: "chan-1",
    transport: {
      transport: "websocket",
      url: "wss://host.test.invalid/live?channel=chan-1",
      token: "tok-single-use-secret",
    },
    capabilities: {
      audio_in: true,
      audio_out: true,
      barge_in_supported: true,
      image_in: false,
      provider_native_resume: false,
      text_in: true,
      text_out: true,
      transcript_supported: true,
      video_in: false,
    },
    continuity: { mode: "fresh" },
  },
  "mob/member_live_close": { status: "closed" },
  "mob/member_live_status": {
    channel_id: "chan-1",
    status: { status: "ready" },
  },
  "mob/member_live_control": { verb: "commit_input", status: "committed" },
};

function cannedClient() {
  const client = new MeerkatClient();
  const calls = [];
  client.request = async (method, params) => {
    calls.push({ method, params });
    const result = CANNED[method];
    if (!result) {
      throw new Error(`no canned result for ${method}`);
    }
    return structuredClone(result);
  };
  return { client, calls };
}

describe("Mob console wrappers (phase 7)", () => {
  it("grant wrappers decode the nested record envelope and issue exact params", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");

    const grant = await mob.grantScopes(
      "operator-1",
      ["read_history", "admin_host"],
      1_750_000_000_000,
    );
    assert.deepEqual(calls[0], {
      method: "mob/grant_scopes",
      params: {
        mob_id: "mob-1",
        principal: "operator-1",
        scopes: ["read_history", "admin_host"],
        expires_at_ms: 1_750_000_000_000,
      },
    });
    assert.deepEqual(grant, {
      principal: "operator-1",
      scopes: ["read_history", "admin_host"],
      expires_at_ms: 1_750_000_000_000,
    });

    assert.equal(await mob.revokeScopes("operator-1", ["admin_host"]), false);
    assert.deepEqual(calls[1], {
      method: "mob/revoke_scopes",
      params: {
        mob_id: "mob-1",
        principal: "operator-1",
        scopes: ["admin_host"],
      },
    });

    assert.deepEqual(await mob.grants(), [grant]);
    assert.deepEqual(calls[2], {
      method: "mob/grants",
      params: { mob_id: "mob-1" },
    });
  });

  it("grantMobScopes fails closed when the nested record is absent", async () => {
    const client = new MeerkatClient();
    client.request = async (method) => {
      assert.equal(method, "mob/grant_scopes");
      return { principal: "envelope-is-not-the-record", scopes: ["list"] };
    };

    await assert.rejects(
      new Mob(client, "mob-1").grantScopes("operator-1", ["list"]),
      /missing record/,
    );
  });

  it("grant projections reject malformed wire scope and expiry truth", async () => {
    for (const record of [
      { principal: "operator-1", scopes: ["future_scope"] },
      { principal: "operator-1", scopes: ["list"], expires_at_ms: null },
      {
        principal: "operator-1",
        scopes: ["list"],
        expires_at_ms: Number.MAX_SAFE_INTEGER + 1,
      },
    ]) {
      const client = new MeerkatClient();
      client.request = async () => ({ record });
      await assert.rejects(
        () => client.grantMobScopes("mob-1", "operator-1", ["list"]),
        /Invalid mob\/grant_scopes response/,
      );
    }
  });

  it("memberHistory issues mob/member_history with snake_case paging", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");
    const result = await mob.memberHistory("worker", { fromIndex: 5, limit: 10 });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, "mob/member_history");
    assert.deepEqual(calls[0].params, {
      mob_id: "mob-1",
      agent_identity: "worker",
      from_index: 5,
      limit: 10,
    });
    assert.equal(result.generation, 1);
    assert.equal(result.provenance, "controlling_host_verified");
    assert.equal(result.page.complete, false);
    assert.equal(result.page.next_index, 1);
  });

  it("hosts unwraps the roster rows and keeps typed capability facts", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");
    const hosts = await mob.hosts();
    assert.equal(calls[0].method, "mob/hosts");
    assert.deepEqual(calls[0].params, { mob_id: "mob-1" });
    assert.equal(hosts.length, 1);
    assert.equal(hosts[0].bind_phase, "bound");
    assert.equal(hosts[0].capabilities.durable_sessions, false);
    assert.equal(hosts[0].capabilities.tracked_input_cancel, true);
  });

  it("normalizes an absent legacy tracked-input-cancel capability to false", async () => {
    const client = new MeerkatClient();
    client.request = async (method) => {
      const result = structuredClone(CANNED[method]);
      if (method === "mob/hosts") {
        delete result.hosts[0].capabilities.tracked_input_cancel;
      } else if (method === "mob/bind_host") {
        delete result.capabilities.tracked_input_cancel;
      } else {
        throw new Error(`unexpected method ${method}`);
      }
      return result;
    };
    const mob = new Mob(client, "mob-1");

    const hosts = await mob.hosts();
    assert.equal(hosts[0].capabilities.tracked_input_cancel, false);
    assert.equal(
      Object.hasOwn(hosts[0].capabilities, "resolvable_providers"),
      false,
    );

    const bound = await mob.bindHost({});
    assert.equal(bound.capabilities.tracked_input_cancel, false);
    assert.equal(Object.hasOwn(bound.capabilities, "resolvable_providers"), false);
  });

  it("rejects null and non-boolean tracked-input-cancel capabilities", async () => {
    for (const method of ["mob/hosts", "mob/bind_host"]) {
      for (const invalid of [null, "yes"]) {
        const client = new MeerkatClient();
        client.request = async (requestedMethod) => {
          assert.equal(requestedMethod, method);
          const result = structuredClone(CANNED[method]);
          const capabilities =
            method === "mob/hosts" ? result.hosts[0].capabilities : result.capabilities;
          capabilities.tracked_input_cancel = invalid;
          return result;
        };
        const mob = new Mob(client, "mob-1");
        const invoke =
          method === "mob/hosts" ? () => mob.hosts() : () => mob.bindHost({});

        await assert.rejects(invoke, /tracked_input_cancel must be boolean/);
      }
    }
  });

  it("rejects malformed present resolvable-provider capabilities", async () => {
    for (const method of ["mob/hosts", "mob/bind_host"]) {
      for (const invalid of [null, "anthropic", [""], [42]]) {
        const client = new MeerkatClient();
        client.request = async (requestedMethod) => {
          assert.equal(requestedMethod, method);
          const result = structuredClone(CANNED[method]);
          const capabilities =
            method === "mob/hosts" ? result.hosts[0].capabilities : result.capabilities;
          capabilities.resolvable_providers = invalid;
          return result;
        };
        const mob = new Mob(client, "mob-1");
        const invoke =
          method === "mob/hosts" ? () => mob.hosts() : () => mob.bindHost({});

        await assert.rejects(invoke, /resolvable_providers/);
      }
    }
  });

  it("routeInstalls keeps the two-field envelope", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");
    const result = await mob.routeInstalls();
    assert.equal(calls[0].method, "mob/route_installs");
    assert.deepEqual(result.outstanding, []);
    assert.equal(result.complete, true);
  });

  it("bindHost passes the descriptor verbatim", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");
    const descriptor = {
      kind: "host",
      address: "tcp://10.0.0.2:7100",
      identity: { kind: "ed25519_public_key", public_key: "ed25519:abc" },
      bootstrap_token: "token-1",
    };
    const result = await mob.bindHost(descriptor);
    assert.equal(calls[0].method, "mob/bind_host");
    assert.deepEqual(calls[0].params, { mob_id: "mob-1", descriptor });
    assert.equal(result.host_id, "host-peer-1");
    assert.equal(result.authority_epoch, 1);
  });

  it("revokeHost returns the typed released-member list", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");
    const result = await mob.revokeHost("host-peer-1");
    assert.equal(calls[0].method, "mob/revoke_host");
    assert.deepEqual(calls[0].params, { mob_id: "mob-1", host_id: "host-peer-1" });
    assert.deepEqual(result.released_members, ["b2"]);
  });

  it("hardCancel requires a reason and unwraps cancelled", async () => {
    const { client, calls } = cannedClient();
    const mob = new Mob(client, "mob-1");
    const cancelled = await mob.hardCancel("worker", "operator interrupt");
    assert.equal(calls[0].method, "mob/hard_cancel_member");
    assert.deepEqual(calls[0].params, {
      mob_id: "mob-1",
      agent_identity: "worker",
      reason: "operator interrupt",
    });
    assert.equal(cancelled, true);
  });

  it("fails closed on malformed nested multi-host rows", async () => {
    const cases = [
      {
        method: "mob/member_history",
        result: {
          generation: 1,
          page: {
            from_index: 0,
            messages: [
              {
                role: "user",
                created_at: "now",
                content: "x",
                transcript_role: "fabricated",
              },
            ],
            message_count: 1,
            complete: true,
          },
          provenance: "host_claimed",
        },
        invoke: (mob) => mob.memberHistory("worker"),
        error: /transcript_role/,
      },
      {
        method: "mob/hosts",
        result: { hosts: ["not-a-host"] },
        invoke: (mob) => mob.hosts(),
        error: /hosts\[0\]/,
      },
      {
        method: "mob/route_installs",
        result: {
          complete: false,
          outstanding: [{ edge_a: "a", edge_b: "b", host: 42 }],
        },
        invoke: (mob) => mob.routeInstalls(),
        error: /host/,
      },
      {
        method: "mob/bind_host",
        result: { host_id: "h", authority_epoch: 1 },
        invoke: (mob) => mob.bindHost({}),
        error: /capabilities/,
      },
      {
        method: "mob/revoke_host",
        result: { host_id: "h", released_members: [42] },
        invoke: (mob) => mob.revokeHost("h"),
        error: /released_members/,
      },
    ];
    for (const testCase of cases) {
      const client = new MeerkatClient();
      client.request = async (method) => {
        assert.equal(method, testCase.method);
        return structuredClone(testCase.result);
      };
      await assert.rejects(testCase.invoke(new Mob(client, "mob-1")), testCase.error);
    }
  });

  it("rejects contradictory and non-progressing member-history pagination", async () => {
    const message = { role: "user", created_at: "now", content: "x" };
    const malformedPages = [
      {
        from_index: 0,
        messages: [message],
        message_count: 2,
        complete: true,
      },
      {
        from_index: 0,
        messages: [message],
        message_count: 1,
        next_index: 1,
        complete: false,
      },
      {
        from_index: 0,
        messages: [message],
        message_count: 2,
        complete: false,
      },
      {
        from_index: 0,
        messages: [],
        message_count: 2,
        next_index: 0,
        complete: false,
      },
      {
        from_index: 0,
        messages: [],
        message_count: 0,
        next_index: null,
        complete: true,
      },
    ];

    for (const page of malformedPages) {
      const client = new MeerkatClient();
      client.request = async () => ({
        page,
        generation: 1,
        provenance: "controlling_host_verified",
      });
      await assert.rejects(
        () => client.mobMemberHistory("mob-1", "worker-1"),
        /complete|progress|next_index/,
      );
    }
  });

  describe("live wrappers never log (token opacity)", () => {
    const consoleCalls = [];
    const original = {};
    beforeEach(() => {
      consoleCalls.length = 0;
      for (const level of ["log", "info", "warn", "error", "debug"]) {
        original[level] = console[level];
        console[level] = (...args) => {
          consoleCalls.push(args.map(String).join(" "));
        };
      }
    });
    afterEach(() => {
      for (const level of ["log", "info", "warn", "error", "debug"]) {
        console[level] = original[level];
      }
    });

    it("memberLiveOpen passes the bootstrap through verbatim", async () => {
      const { client, calls } = cannedClient();
      const mob = new Mob(client, "mob-1");
      const open = await mob.memberLiveOpen("worker", {
        turningMode: "provider_managed",
        transport: "websocket",
      });
      assert.equal(calls[0].method, "mob/member_live_open");
      assert.deepEqual(calls[0].params, {
        mob_id: "mob-1",
        agent_identity: "worker",
        turning_mode: "provider_managed",
        transport: "websocket",
      });
      // Verbatim pass-through: the token survives untouched...
      assert.equal(open.transport.token, "tok-single-use-secret");
      // ...and never reaches any console sink.
      assert.equal(consoleCalls.length, 0, `unexpected logging: ${consoleCalls}`);
      assert.ok(
        !consoleCalls.some((line) => line.includes("tok-single-use-secret")),
        "single-use token must never be logged",
      );
    });

    it("memberLiveClose / memberLiveStatus / memberLiveControl issue exact literals", async () => {
      const { client, calls } = cannedClient();
      const mob = new Mob(client, "mob-1");

      const closed = await mob.memberLiveClose("worker", "chan-1");
      assert.equal(closed.status, "closed");
      assert.equal(calls[0].method, "mob/member_live_close");
      assert.deepEqual(calls[0].params, {
        mob_id: "mob-1",
        agent_identity: "worker",
        channel_id: "chan-1",
      });

      // Discovery read: absent channelId is OMITTED from the params.
      const status = await mob.memberLiveStatus("worker");
      assert.equal(status.channel_id, "chan-1");
      assert.equal(calls[1].method, "mob/member_live_status");
      assert.deepEqual(calls[1].params, {
        mob_id: "mob-1",
        agent_identity: "worker",
      });

      const outcome = await mob.memberLiveControl("worker", "chan-1", {
        verb: "commit_input",
      });
      assert.equal(calls[2].method, "mob/member_live_control");
      assert.deepEqual(calls[2].params, {
        mob_id: "mob-1",
        agent_identity: "worker",
        channel_id: "chan-1",
        verb: { verb: "commit_input" },
      });
      assert.equal(outcome.verb, "commit_input");
      assert.equal(outcome.status, "committed");

      assert.equal(consoleCalls.length, 0, `unexpected logging: ${consoleCalls}`);
    });
  });
});
