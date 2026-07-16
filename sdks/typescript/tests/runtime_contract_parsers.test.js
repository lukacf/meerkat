import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { MeerkatClient } from "../dist/client.js";

const LIVE_CAPABILITIES = {
  audio_in: true,
  audio_out: true,
  barge_in_supported: true,
  image_in: true,
  provider_native_resume: false,
  text_in: true,
  text_out: true,
  transcript_supported: true,
  video_in: false,
};

function liveOpenResult(overrides = {}) {
  return {
    channel_id: "channel-1",
    capabilities: { ...LIVE_CAPABILITIES },
    continuity: { mode: "fresh" },
    transport: {
      transport: "websocket",
      url: "wss://example.invalid/live",
      token: "opaque-token",
    },
    ...overrides,
  };
}

function liveStatusResult(status) {
  return { channel_id: "channel-1", status };
}

async function invokeOpen(surface, result) {
  const client = new MeerkatClient();
  client.request = async () => structuredClone(result);
  return surface === "session"
    ? client.liveOpen({ session_id: "session-1" })
    : client.openMobMemberLive("mob-1", "worker-1");
}

async function invokeStatus(surface, result) {
  const client = new MeerkatClient();
  client.request = async () => structuredClone(result);
  return surface === "session"
    ? client.liveStatus({ channel_id: "channel-1" })
    : client.mobMemberLiveStatus("mob-1", "worker-1", "channel-1");
}

describe("shared live result parsers", () => {
  it("accepts every generated open discriminator and preserves opaque tokens", async () => {
    const opaqueToken = "  opaque+/= token remains verbatim  ";
    const transports = [
      {
        transport: "websocket",
        url: "wss://example.invalid/live",
        token: opaqueToken,
      },
      {
        transport: "webrtc",
        answer_method: "live/webrtc/answer",
        http_url: "https://example.invalid/offer",
        token: opaqueToken,
      },
      { transport: "unknown", debug: "future transport" },
    ];
    const continuities = [
      { mode: "fresh" },
      { mode: "transcript_only" },
      { mode: "degraded" },
      { mode: "provider_native_resume", provider_session_id: "provider-1" },
      { mode: "unknown", debug: "future continuity" },
    ];

    for (const surface of ["session", "mob"]) {
      for (const transport of transports) {
        for (const continuity of continuities) {
          const result = await invokeOpen(
            surface,
            liveOpenResult({ transport, continuity }),
          );
          assert.deepEqual(result.transport, transport);
          assert.deepEqual(result.continuity, continuity);
          if ("token" in transport) {
            assert.equal(result.transport.token, opaqueToken);
          }
        }
      }
    }
  });

  it("fails closed on malformed capabilities, continuity, and transports on both surfaces", async () => {
    const missingCapability = { ...LIVE_CAPABILITIES };
    delete missingCapability.video_in;
    const wrongCapability = { ...LIVE_CAPABILITIES, audio_in: "yes" };
    const malformed = [
      liveOpenResult({ capabilities: missingCapability }),
      liveOpenResult({ capabilities: wrongCapability }),
      liveOpenResult({ continuity: { mode: "future_mode" } }),
      liveOpenResult({ continuity: { mode: "provider_native_resume" } }),
      liveOpenResult({ continuity: { mode: "unknown" } }),
      liveOpenResult({
        transport: { transport: "websocket", url: "wss://example.invalid/live" },
      }),
      liveOpenResult({
        transport: { transport: "webrtc", answer_method: "answer" },
      }),
      liveOpenResult({
        transport: { transport: "webrtc", token: "token" },
      }),
      liveOpenResult({
        transport: {
          transport: "webrtc",
          answer_method: "answer",
          token: "token",
          http_url: null,
        },
      }),
      liveOpenResult({ transport: { transport: "future_transport" } }),
      liveOpenResult({ transport: { transport: "unknown" } }),
    ];

    for (const surface of ["session", "mob"]) {
      for (const result of malformed) {
        await assert.rejects(() => invokeOpen(surface, result), /Invalid .* response/);
      }
    }
  });

  it("accepts every generated status and degradation discriminator", async () => {
    const reasons = [
      { kind: "rate_limited" },
      { kind: "provider_throttled" },
      { kind: "network_unstable" },
      { kind: "other", detail: "provider maintenance" },
      { kind: "unknown", debug: "future reason" },
    ];
    const statuses = [
      { status: "idle" },
      { status: "opening" },
      { status: "ready" },
      { status: "closing" },
      { status: "closed" },
      { status: "unknown", debug: "future status" },
      ...reasons.map((reason) => ({ status: "degraded", reason })),
    ];

    for (const surface of ["session", "mob"]) {
      for (const status of statuses) {
        const result = await invokeStatus(surface, liveStatusResult(status));
        assert.deepEqual(result.status, status);
      }
    }
  });

  it("fails closed on malformed status and degradation payloads on both surfaces", async () => {
    const malformed = [
      liveStatusResult({ status: "future_status" }),
      liveStatusResult({ status: "unknown" }),
      liveStatusResult({ status: "degraded" }),
      liveStatusResult({ status: "degraded", reason: { kind: "future_reason" } }),
      liveStatusResult({ status: "degraded", reason: { kind: "other" } }),
      liveStatusResult({ status: "degraded", reason: { kind: "unknown" } }),
    ];

    for (const surface of ["session", "mob"]) {
      for (const result of malformed) {
        await assert.rejects(() => invokeStatus(surface, result), /Invalid .* response/);
      }
    }
  });
});

function parseHistoryMessage(message) {
  return MeerkatClient.parseSessionHistory({
    session_id: "session-1",
    message_count: 1,
    offset: 0,
    has_more: false,
    messages: [message],
  });
}

describe("recursive generated history validation", () => {
  it("accepts valid nested system-notice, assistant, and tool-result payloads", () => {
    const wireContent = [
      { type: "text", text: "hello" },
      { type: "image", media_type: "image/png" },
      { type: "video", media_type: "video/mp4", duration_ms: 42 },
      { type: "structured", data: { answer: 42 } },
      { type: "unknown" },
    ];
    const history = MeerkatClient.parseSessionHistory({
      session_id: "session-1",
      message_count: 4,
      offset: 0,
      has_more: false,
      messages: [
        {
          role: "user",
          created_at: "2026-07-11T11:59:59Z",
          content: wireContent,
        },
        {
          role: "system_notice",
          created_at: "2026-07-11T12:00:00Z",
          kind: "tool_scope",
          blocks: [
            {
              type: "tool_config",
              payload: {
                operation: "reload",
                persisted: false,
                target: "filesystem",
                status_info: {
                  kind: "boundary_applied",
                  base_changed: true,
                  revision: 4,
                  visible_changed: false,
                },
              },
            },
          ],
        },
        {
          role: "block_assistant",
          created_at: "2026-07-11T12:00:01Z",
          blocks: [
            {
              block_type: "transcript",
              data: {
                text: "spoken answer",
                source: { kind: "spoken" },
              },
            },
            {
              block_type: "transcript",
              data: {
                text: "future lane",
                source: { kind: "unknown", debug: "provider-lane-v2" },
              },
            },
          ],
          stop_reason: "end_turn",
        },
        {
          role: "tool_results",
          created_at: "2026-07-11T12:00:02Z",
          results: [
            {
              tool_use_id: "tool-1",
              content: [
                { type: "video", media_type: "video/mp4", duration_ms: 42 },
              ],
              is_error: false,
            },
          ],
        },
      ],
    });
    assert.equal(history.messages.length, 4);
    assert.deepEqual(history.messages[0].content, wireContent);
    assert.equal(Object.hasOwn(history.messages[0].content[1], "data"), false);
    assert.equal(Object.hasOwn(history.messages[0].content[2], "data"), false);
    assert.deepEqual(history.messages[2].blocks[0].source, { kind: "spoken" });
    assert.deepEqual(history.messages[2].blocks[1].source, {
      kind: "unknown",
      debug: "provider-lane-v2",
    });
    assert.deepEqual(
      MeerkatClient.serializeSessionAssistantBlock(history.messages[2].blocks[1])
        .data.source,
      { kind: "unknown", debug: "provider-lane-v2" },
    );
    assert.deepEqual(history.messages[3].results[0].content, [
      { type: "video", media_type: "video/mp4", duration_ms: 42 },
    ]);
  });

  it("rejects malformed nested content and system-notice blocks", () => {
    const malformed = [
      {
        role: "user",
        created_at: "now",
        content: [{ type: "text" }],
      },
      {
        role: "user",
        created_at: "now",
        content: [{ type: "future_content" }],
      },
      {
        role: "system_notice",
        created_at: "now",
        kind: "comms",
        blocks: [{ type: "comms", kind: "message" }],
      },
      {
        role: "system_notice",
        created_at: "now",
        kind: "tool_scope",
        blocks: [
          {
            type: "tool_config",
            payload: {
              operation: "add",
              persisted: false,
              target: "tool",
              status_info: {
                kind: "boundary_applied",
                base_changed: true,
                visible_changed: true,
              },
            },
          },
        ],
      },
      {
        role: "system_notice",
        created_at: "now",
        kind: "mcp",
        blocks: [{ type: "mcp", pending_sources: [42] }],
      },
    ];
    for (const message of malformed) {
      assert.throws(() => parseHistoryMessage(message), /Invalid session history response/);
    }
  });

  it("rejects malformed nested assistant blocks and tool results", () => {
    const assistant = (block) => ({
      role: "block_assistant",
      created_at: "now",
      blocks: [block],
      stop_reason: "end_turn",
    });
    const malformed = [
      assistant({ block_type: "future_block" }),
      assistant({ block_type: "text", data: {} }),
      assistant({
        block_type: "transcript",
        data: { text: "spoken", source: { kind: "future_source" } },
      }),
      assistant({ block_type: "tool_use", data: { id: "1", name: "tool" } }),
      assistant({
        block_type: "server_tool_content",
        data: { content: {}, kind: null },
      }),
      assistant({
        block_type: "image",
        data: {
          blob_ref: {},
          height: 1,
          image_id: "image-1",
          media_type: "image/png",
          revised_prompt: {},
          width: 1,
        },
      }),
      {
        role: "tool_results",
        created_at: "now",
        results: [
          {
            tool_use_id: "tool-1",
            content: [{ type: "video", media_type: "video/mp4" }],
          },
        ],
      },
      {
        role: "tool_results",
        created_at: "now",
        results: [{ tool_use_id: "tool-1", content: "ok", is_error: null }],
      },
    ];
    for (const message of malformed) {
      assert.throws(() => parseHistoryMessage(message), /Invalid session history response/);
    }
  });

  it("uses the same recursive validator for mob member history", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({
      generation: 1,
      page: {
        from_index: 0,
        messages: [
          {
            role: "tool_results",
            created_at: "now",
            results: [{ tool_use_id: "tool-1", content: [{ type: "text" }] }],
          },
        ],
        message_count: 1,
        complete: true,
      },
      provenance: "controlling_host_verified",
    });
    await assert.rejects(
      () => client.mobMemberHistory("mob-1", "worker-1"),
      /page.messages\[0\].*content\[0\]/,
    );
  });
});
