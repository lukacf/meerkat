import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient, RealtimeChannel } from "../dist/index.js";

// Phase 5G/T5i retired the `mob_member_target` wire shape; the canonical
// contract is now that `RealtimeChannelTarget` is always `SessionTarget`.
// See `meerkat-contracts/src/wire/realtime.rs:200-210` for the contract,
// and `meerkat-rpc/tests/router_realtime_target.rs:108` for the server-
// side regression that explicitly asserts `mob_member_target` is
// rejected with `-32602 INVALID_PARAMS`. The SDK's
// `RealtimeChannel.mobMember()` helper is retained for ergonomics but
// resolves `mob/member_status.currentSessionId` on the first wire call
// and opens with a `SessionTarget`.

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
    "RealtimeChannel.mobMember defers target resolution to mob/member_status and opens with SessionTarget",
    async () => {
      // Post-T5i identity-first contract: the SDK takes a mob-member
      // binding, resolves it via `mob/member_status.currentSessionId`
      // on the first wire call, and opens with a `session_target`.
      // The `mob_member_target` wire type is gone from the contract.
      const client = new MeerkatClient();
      const calls = [];

      client.request = async (method, params) => {
        calls.push({ method, params });
        if (method === "mob/member_status") {
          return {
            status: "active",
            agent_runtime_id: { identity: "agent-a", generation: 1 },
            fence_token: 1,
            tokens_used: 0,
            is_final: false,
            current_session_id: "resolved-session-42",
          };
        }
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
        throw new Error(`unexpected method ${method}`);
      };

      const memberChannel = RealtimeChannel.mobMember(client, "mob-1", "agent-a", {
        role: "observer",
        turningMode: "explicit_commit",
      });

      // Before the first wire call the channel has no resolved target
      // and calling `openRequest()` must refuse rather than silently
      // emit a placeholder — the wire target is always a
      // `SessionTarget` and it cannot be materialized without
      // resolving the mob binding through `mob/member_status` first.
      assert.equal(memberChannel.target, null);
      assert.throws(
        () => memberChannel.openRequest(),
        (err) => err && err.code === "INVALID_REQUEST",
        "openRequest() must refuse an unresolved mob-member binding",
      );

      // openInfo() triggers the resolve-then-open path.
      const openInfo = await memberChannel.openInfo();

      // The wire target that crossed the RPC boundary must be a
      // `session_target` with the session_id surfaced by
      // mob/member_status — never the retired `mob_member_target`
      // shape. See server-side `router_realtime_target.rs:108`: the
      // RPC router explicitly rejects `mob_member_target` with
      // `-32602`, so any SDK that emitted that shape would fail at
      // runtime.
      assert.deepEqual(openInfo.target, {
        type: "session_target",
        session_id: "resolved-session-42",
      });
      assert.notEqual(
        openInfo.target.type,
        "mob_member_target",
        "retired mob_member_target wire type must never be emitted",
      );
      assert.equal(memberChannel.role, "observer");
      assert.equal(memberChannel.turningMode, "explicit_commit");

      // After resolution the channel caches the session target so
      // subsequent wire calls do not re-resolve.
      assert.deepEqual(memberChannel.target, {
        type: "session_target",
        session_id: "resolved-session-42",
      });
      const status = await memberChannel.status();
      assert.equal(status.status.state, "opening");

      // Call order encodes the new contract: mob/member_status must
      // land before any realtime/* call that targets the member.
      assert.deepEqual(calls.map((call) => call.method), [
        "mob/member_status",
        "realtime/open_info",
        "realtime/status",
      ]);
      // mob/member_status fires exactly once thanks to caching.
      const memberStatusCalls = calls.filter((call) => call.method === "mob/member_status");
      assert.equal(memberStatusCalls.length, 1);
      assert.deepEqual(memberStatusCalls[0].params, {
        mob_id: "mob-1",
        agent_identity: "agent-a",
      });

      // Negative regression guard: not a single request the SDK made
      // to the server may carry the retired wire shape.
      for (const call of calls) {
        const target = call.params?.target;
        if (target && typeof target === "object") {
          assert.notEqual(
            target.type,
            "mob_member_target",
            `${call.method} leaked retired mob_member_target wire shape`,
          );
        }
      }
    },
  );
});
