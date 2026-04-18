import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient, RealtimeChannel } from "../dist/index.js";

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

    const memberChannel = RealtimeChannel.mobMember(client, "mob-1", "agent-a", {
      role: "observer",
      turningMode: "explicit_commit",
    });
    assert.deepEqual(memberChannel.openRequest().target, {
      type: "mob_member_target",
      mob_id: "mob-1",
      agent_identity: "agent-a",
    });
    assert.equal(memberChannel.openRequest().role, "observer");
    assert.equal(memberChannel.openRequest().turning_mode, "explicit_commit");

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
});
