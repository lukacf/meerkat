import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient, RealtimeChannel } from "../dist/index.js";

// W3-H: `RealtimeChannelTarget::MobMember { mob_id, agent_identity }` is
// a first-class wire variant. The SDK builds the MobMember variant
// directly — no pre-resolution of `mob/member_status → current_session_id`,
// no session-id pin — so respawn-driven session rotation does not
// require any SDK round-trip or reconnect. See
// `meerkat-contracts/src/wire/realtime.rs:224-232` for the contract and
// `meerkat-mob/tests/member_realtime_bindings.rs` for the machine-owned
// binding lifecycle this relies on.

describe("RealtimeChannel scaffold", () => {
  it("routes realtime wrappers through explicit client methods and request builders", async () => {
    const client = new MeerkatClient();
    const calls = [];

    client.request = async (method, params) => {
      calls.push({ method, params });
      if (method === "realtime/open_info") {
        return {
          ws_url: "ws://localhost:9999/realtime/ws",
          open_token: "token-1",
          expires_at: "2026-04-15T12:00:00Z",
          target: params.target,
          supported_protocol_versions: ["1"],
          default_protocol_version: "1",
          capabilities: {
            input_kinds: ["text", "audio"],
            output_kinds: ["text", "audio"],
            turning_modes: ["provider_managed"],
            interrupt_supported: true,
            transcript_supported: true,
            tool_lifecycle_events_supported: false,
            video_supported: false,
          },
        };
      }
      if (method === "realtime/status") {
        return {
          status: {
            state: "opening",
            attempt_count: 0,
          },
        };
      }
      if (method === "realtime/capabilities") {
        return {
          capabilities: {
            input_kinds: ["text", "audio"],
            output_kinds: ["text", "audio"],
            turning_modes: ["provider_managed"],
            interrupt_supported: true,
            transcript_supported: true,
            tool_lifecycle_events_supported: false,
            video_supported: false,
          },
        };
      }
      throw new Error(`unexpected method ${method}`);
    };

    const sessionChannel = RealtimeChannel.session(client, "session-1");
    assert.deepEqual(sessionChannel.openRequest().target, {
      type: "session_target",
      session_id: "session-1",
    });
    assert.equal(sessionChannel.openRequest().role, "primary");
    assert.equal(sessionChannel.openRequest().turning_mode, "provider_managed");

    const openInfo = await client.realtimeOpenInfo(sessionChannel.openRequest());
    const status = await client.realtimeStatus({ target: sessionChannel.target });
    const capabilities = await client.realtimeCapabilities({ target: sessionChannel.target });

    const scopedOpenInfo = await sessionChannel.openInfo();
    const scopedStatus = await sessionChannel.status();
    const scopedCapabilities = await sessionChannel.capabilities();

    assert.equal(openInfo.default_protocol_version, "1");
    assert.equal(status.status.state, "opening");
    assert.deepEqual(capabilities.capabilities.turning_modes, ["provider_managed"]);
    assert.equal(scopedOpenInfo.open_token, "token-1");
    assert.equal(scopedStatus.status.state, "opening");
    assert.deepEqual(scopedCapabilities.capabilities.input_kinds, ["text", "audio"]);
    assert.deepEqual(calls.map((call) => call.method), [
      "realtime/open_info",
      "realtime/status",
      "realtime/capabilities",
      "realtime/open_info",
      "realtime/status",
      "realtime/capabilities",
    ]);
  });

  it(
    "RealtimeChannel.mobMember builds the MobMember wire target directly",
    async () => {
      // W3-H: identity is a first-class wire fact. The SDK emits
      // `{type: mob_member, mob_id, agent_identity}` directly; the
      // server resolves `(mob_id, agent_identity)` against the
      // MobMachine's canonical binding map on every tick, so respawn
      // rotates the bound session without any SDK round-trip or
      // session-id pin.
      const client = new MeerkatClient();
      const calls = [];

      client.request = async (method, params) => {
        calls.push({ method, params });
        if (method === "realtime/open_info") {
          return {
            ws_url: "ws://localhost:9999/realtime/ws",
            open_token: "token-mob",
            expires_at: "2026-04-15T12:00:00Z",
            target: params.target,
            supported_protocol_versions: ["1"],
            default_protocol_version: "1",
            capabilities: {
              input_kinds: ["text"],
              output_kinds: ["text"],
              turning_modes: ["explicit_commit"],
              interrupt_supported: true,
              transcript_supported: true,
              tool_lifecycle_events_supported: false,
              video_supported: false,
            },
          };
        }
        if (method === "realtime/status") {
          return { status: { state: "opening", attempt_count: 0 } };
        }
        if (method === "realtime/capabilities") {
          return {
            capabilities: {
              input_kinds: ["text"],
              output_kinds: ["text"],
              turning_modes: ["explicit_commit"],
              interrupt_supported: true,
              transcript_supported: true,
              tool_lifecycle_events_supported: false,
              video_supported: false,
            },
          };
        }
        throw new Error(`unexpected method ${method}`);
      };

      const memberChannel = RealtimeChannel.mobMember(client, "mob-1", "agent-a", {
        role: "observer",
        turningMode: "explicit_commit",
      });

      // W3-H: target carries identity with no pre-resolve round-trip
      // and no pinned session id. `openRequest()` returns it as-is.
      const expectedTarget = {
        type: "mob_member",
        mob_id: "mob-1",
        agent_identity: "agent-a",
      };
      assert.deepEqual(memberChannel.target, expectedTarget);
      assert.deepEqual(memberChannel.openRequest().target, expectedTarget);

      // openInfo() sends the MobMember target directly.
      const openInfo = await memberChannel.openInfo();
      assert.deepEqual(openInfo.target, expectedTarget);
      assert.equal(memberChannel.role, "observer");
      assert.equal(memberChannel.turningMode, "explicit_commit");

      // status() and capabilities() also send the MobMember target
      // with no `mob/member_status` pre-resolve round-trip.
      const status = await memberChannel.status();
      assert.equal(status.status.state, "opening");
      await memberChannel.capabilities();

      // Call order encodes the new contract: only `realtime/*` calls
      // cross the wire. Respawn rotates the bound session inside the
      // server; the SDK is unaware.
      assert.deepEqual(calls.map((call) => call.method), [
        "realtime/open_info",
        "realtime/status",
        "realtime/capabilities",
      ]);

      // Every outbound target carries the MobMember variant — the SDK
      // never emits the retired `mob_member_target` shape and never
      // emits a `session_target` synthesized from `mob/member_status`.
      for (const call of calls) {
        const target = call.params?.target;
        assert.ok(target && typeof target === "object", `${call.method} lost the target`);
        assert.equal(
          target.type,
          "mob_member",
          `${call.method} leaked non-MobMember wire shape ${JSON.stringify(target)}`,
        );
        assert.notEqual(
          target.type,
          "mob_member_target",
          `${call.method} leaked retired mob_member_target wire shape`,
        );
      }
    },
  );
});
