#!/usr/bin/env node

import readline from "node:readline";

const CONTRACT_VERSION = "0.4.6";

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
        source_id: "fake-session",
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
      ...(error ? { error } : {}),
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

  if (method === "session/stream_open") {
    const sessionId = String(params.session_id ?? "");
    if (sessionId === "buffered-session") {
      write(streamEvent("stream-buffered", "e1", "hi", 1));
      write(streamEvent("stream-buffered", "e2", "there", 2));
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
              type: "text_delta",
              delta: "LATE_TAIL_PUBLIC",
            },
          },
        });
      }, 20);
      return;
    }
  }

  if (method === "session/stream_close") {
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
