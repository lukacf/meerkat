#!/usr/bin/env node

import readline from "node:readline";
import { readFileSync } from "node:fs";

const versionInfo = JSON.parse(
  readFileSync(new URL("../../../../artifacts/schemas/version.json", import.meta.url), "utf8"),
);
const CONTRACT_VERSION = versionInfo.contract_version;

function write(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function streamEvent(streamId, eventId, delta, seq) {
  return {
    jsonrpc: "2.0",
    method: "session/stream_event",
    params: {
      stream_id: streamId,
      event: {
        event_id: eventId,
        source: {
          type: "session",
          session_id: "00000000-0000-4000-8000-000000000001",
        },
        source_id: "session:00000000-0000-4000-8000-000000000001",
        seq,
        timestamp_ms: seq,
        payload: {
          type: "text_delta",
          delta,
        },
      },
    },
  };
}

function streamEnd(streamId, outcome, error = undefined) {
  return {
    jsonrpc: "2.0",
    method: "session/stream_end",
    params: {
      stream_id: streamId,
      ended: true,
      outcome,
      ...(error !== undefined ? { error } : {}),
    },
  };
}

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

rl.on("line", (line) => {
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    return;
  }

  const { id, method, params = {} } = request;
  if (typeof id === "undefined" || !method) {
    return;
  }

  if (method === "initialize") {
    write({
      jsonrpc: "2.0",
      id,
      result: {
        contract_version: CONTRACT_VERSION,
        capabilities: [],
      },
    });
    return;
  }

  if (method === "capabilities/get") {
    write({
      jsonrpc: "2.0",
      id,
      result: {
        capabilities: [
          {
            id: "sessions",
            description: "Fake session capability",
            status: "Available",
          },
        ],
      },
    });
    return;
  }

  if (method === "session/stream_open" || method === "mob/stream_open") {
    const scenario = String(params.session_id ?? params.mob_id ?? "");
    const cases = {
      "queued-terminal-error": ["terminal_error", { code: "stream_queue_overflow", message: "synthetic stream overflow" }],
      "queued-gap-terminal-error": ["terminal_error", { code: "stream_queue_overflow", message: "synthetic stream overflow" }],
      "queued-unknown-error": ["future_terminal_outcome", { code: "future_stream_code", message: "future detail: preserve exactly" }],
      "queued-remote-end": ["remote_end", undefined],
      "queued-explicit-close": ["explicit_close", null],
      "live-terminal-error": ["terminal_error", { code: "stream_queue_overflow", message: "synthetic stream overflow" }],
    };
    if (Object.hasOwn(cases, scenario)) {
      const streamId = `${method}:${scenario}:${params.agent_identity ?? "all"}`;
      const [outcome, error] = cases[scenario];
      const response = { jsonrpc: "2.0", id, result: { stream_id: streamId } };
      const emit = () => {
        for (const [index, delta] of ["first", "second"].entries()) {
          const message = streamEvent(streamId,
            `00000000-0000-4000-8000-00000000002${index}`, delta, 41 + index);
          message.params.sequence = 7 + index;
          if (scenario === "queued-gap-terminal-error" && index === 1) {
            message.params.sequence = 9;
            message.params.event.seq = 0;
            message.params.event.payload = {
              type: "stream_truncated", reason: { kind: "stream_lagged", dropped: 1 },
            };
          }
          if (method === "mob/stream_open") {
            message.method = "mob/stream_event";
            if (!params.agent_identity) {
              message.params.event = {
                source: { identity: "synthetic-agent", generation: 0 },
                role: "synthetic-player",
                envelope: message.params.event,
              };
            }
          }
          write(message);
        }
        const terminal = streamEnd(streamId, outcome, error);
        if (method === "mob/stream_open") terminal.method = "mob/stream_end";
        write(terminal);
      };
      if (scenario === "live-terminal-error") {
        write(response);
        setTimeout(emit, 5);
      } else {
        emit();
        write(response);
      }
      return;
    }
  }

  if (method === "session/stream_open") {
    const sessionId = String(params.session_id ?? "");
    if (sessionId === "buffered-session") {
      write(streamEvent(
        "stream-buffered",
        "00000000-0000-4000-8000-000000000010",
        "hi",
        1,
      ));
      write(streamEvent(
        "stream-buffered",
        "00000000-0000-4000-8000-000000000011",
        "there",
        2,
      ));
      write(streamEnd("stream-buffered", "remote_end"));
      write({
        jsonrpc: "2.0",
        id,
        result: {
          stream_id: "stream-buffered",
        },
      });
      return;
    }

    if (sessionId === "terminal-error-session") {
      write(
        streamEnd("stream-terminal-error", "terminal_error", {
          code: "stream_queue_overflow",
          message: "transport stream notification queue overflow",
        }),
      );
      write({
        jsonrpc: "2.0",
        id,
        result: {
          stream_id: "stream-terminal-error",
        },
      });
      return;
    }

    write({
      jsonrpc: "2.0",
      id,
      error: {
        code: -32602,
        message: `Unknown fake session: ${sessionId}`,
      },
    });
    return;
  }

  if (method === "turn/start") {
    const sessionId = String(params.session_id ?? "");
    if (sessionId === "late-tail-stream-session") {
      write({
        jsonrpc: "2.0",
        id,
        result: {
          session_id: sessionId,
          text: "late tail final result",
          turns: 1,
          tool_calls: 0,
          usage: {
            input_tokens: 1,
            output_tokens: 1,
          },
        },
      });
      setTimeout(() => {
        write({
          jsonrpc: "2.0",
          method: "turn/event",
          params: {
            session_id: sessionId,
            event: {
              event_id: "00000000-0000-4000-8000-000000000012",
              source: {
                type: "session",
                session_id: "00000000-0000-4000-8000-000000000001",
              },
              seq: 3,
              timestamp_ms: 3,
              payload: {
                type: "text_delta",
                delta: "LATE_TAIL_PUBLIC",
              },
            },
          },
        });
      }, 20);
      return;
    }
  }

  if (method === "session/stream_close" || method === "mob/stream_close") {
    write({
      jsonrpc: "2.0",
      id,
      result: {
        closed: true,
        already_closed: false,
      },
    });
    return;
  }

  write({
    jsonrpc: "2.0",
    id,
    error: {
      code: -32601,
      message: `Method not found: ${method}`,
    },
  });
});
