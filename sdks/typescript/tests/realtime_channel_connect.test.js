import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { WebSocketServer } from "ws";
import { MeerkatClient, RealtimeChannel } from "../dist/index.js";

describe("RealtimeChannel websocket client", () => {
  it("includes realtime websocket bootstrap args for spawned SDK clients", () => {
    const args = MeerkatClient.buildArgs(false, undefined);
    assert.deepEqual(args.slice(0, 2), ["--realtime-ws", "127.0.0.1:0"]);
  });

  it("connects and exchanges typed channel frames", async () => {
    const seen = [];
    const server = new WebSocketServer({ port: 0 });
    await new Promise((resolve) => server.once("listening", resolve));
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 0;

    server.on("connection", (socket) => {
      socket.once("message", (message) => {
        seen.push(JSON.parse(String(message)));
        socket.send(
          JSON.stringify({
            type: "channel.opened",
            protocol_version: "2",
            status: { state: "ready", attempt_count: 0 },
            capabilities: {
              input_kinds: ["text"],
              output_kinds: ["text"],
              turning_modes: ["provider_managed"],
              interrupt_supported: true,
              transcript_supported: true,
              tool_lifecycle_events_supported: false,
              video_supported: false,
            },
            role: "primary",
          }),
        );

        socket.once("message", (input) => {
          seen.push(JSON.parse(String(input)));
          socket.send(
            JSON.stringify({
              type: "channel.event",
              event: { type: "output_text_delta", delta: "world" },
            }),
          );

          socket.once("message", (commit) => {
            seen.push(JSON.parse(String(commit)));
            socket.once("message", (interrupt) => {
              seen.push(JSON.parse(String(interrupt)));
              socket.once("message", (close) => {
                seen.push(JSON.parse(String(close)));
                socket.send(JSON.stringify({ type: "channel.closed", reason: "done" }));
              });
            });
          });
        });
      });
    });

    const client = new MeerkatClient();
    client.realtimeOpenInfo = async () => ({
      ws_url: `ws://127.0.0.1:${port}/realtime/ws`,
      open_token: "token-1",
      expires_at: "2026-04-15T12:00:00Z",
      target: { type: "session_target", session_id: "session-1" },
      supported_protocol_versions: ["2"],
      default_protocol_version: "2",
      capabilities: {
        input_kinds: ["text"],
        output_kinds: ["text"],
        turning_modes: ["provider_managed"],
        interrupt_supported: true,
        transcript_supported: true,
        tool_lifecycle_events_supported: false,
        video_supported: false,
      },
    });

    const channel = RealtimeChannel.session(client, "session-1");
    const connection = await channel.connect({ waitForAttachment: false });
    await connection.sendInput({ kind: "text_chunk", text: "hello" });
    const frame = await connection.nextFrame();
    assert.equal(frame.type, "channel.event");
    assert.equal(frame.event.type, "output_text_delta");
    assert.equal(frame.event.delta, "world");
    await connection.commitTurn();
    await connection.interrupt();
    await connection.close();
    const closed = await connection.nextFrame();
    assert.equal(closed?.type, "channel.closed");

    assert.deepEqual(seen, [
      {
        type: "channel.open",
        protocol_version: "2",
        open_token: "token-1",
        role: "primary",
        turning_mode: "provider_managed",
      },
      {
        type: "channel.input",
        chunk: { kind: "text_chunk", text: "hello" },
      },
      { type: "channel.commit_turn" },
      { type: "channel.interrupt" },
      { type: "channel.close" },
    ]);

    for (const socket of server.clients) {
      socket.close();
    }
    await new Promise((resolve) => server.close(resolve));
  });

  it("connects from supplied realtime open info without fetching bootstrap", async () => {
    const seen = [];
    const server = new WebSocketServer({ port: 0 });
    await new Promise((resolve) => server.once("listening", resolve));
    const address = server.address();
    const port = typeof address === "object" && address ? address.port : 0;

    server.on("connection", (socket) => {
      socket.once("message", (message) => {
        seen.push(JSON.parse(String(message)));
        socket.send(
          JSON.stringify({
            type: "channel.opened",
            protocol_version: "2",
            status: { state: "ready", attempt_count: 0 },
            capabilities: {
              input_kinds: ["text"],
              output_kinds: ["text"],
              turning_modes: ["provider_managed"],
              interrupt_supported: true,
              transcript_supported: true,
              tool_lifecycle_events_supported: false,
              video_supported: false,
            },
            role: "primary",
          }),
        );
        socket.send(JSON.stringify({ type: "channel.closed", reason: "done" }));
      });
    });

    const client = new MeerkatClient();
    client.realtimeOpenInfo = async () => {
      throw new Error("channel.connectWithOpenInfo should not fetch bootstrap");
    };

    const channel = RealtimeChannel.session(client, "session-1");
    const connection = await channel.connectWithOpenInfo({
      ws_url: `ws://127.0.0.1:${port}/realtime/ws`,
      open_token: "token-2",
      expires_at: "2026-04-15T12:00:00Z",
      target: { type: "session_target", session_id: "session-1" },
      supported_protocol_versions: ["2"],
      default_protocol_version: "2",
      capabilities: {
        input_kinds: ["text"],
        output_kinds: ["text"],
        turning_modes: ["provider_managed"],
        interrupt_supported: true,
        transcript_supported: true,
        tool_lifecycle_events_supported: false,
        video_supported: false,
      },
    });

    const closed = await connection.nextFrame();
    assert.equal(closed?.type, "channel.closed");
    assert.deepEqual(seen, [
      {
        type: "channel.open",
        protocol_version: "2",
        open_token: "token-2",
        role: "primary",
        turning_mode: "provider_managed",
      },
    ]);

    for (const socket of server.clients) {
      socket.close();
    }
    await new Promise((resolve) => server.close(resolve));
  });
});
