import assert from "node:assert/strict";
import test from "node:test";
import { isLogFrameVisible, sanitizeLogFrameData, summarizeLogFrame } from "./LogsPanel";
import type { ConsoleFrame } from "../types";

test("log summaries omit raw session-history message envelopes", () => {
  const frame: ConsoleFrame = {
    id: "frame-1",
    event: "interaction_complete",
    identity: "scribe",
    data: {
      message: {
        blocks: [
          { block_type: "reasoning", data: { text: "private planning" } },
          { block_type: "text", data: { text: "Visible answer." } },
        ],
      },
      result: "Visible answer.",
      source_event_type: "session_history",
      text: "Visible answer.",
    },
  };

  const summary = summarizeLogFrame(frame);
  assert.equal(summary.includes("message="), false);
  assert.equal(summary.includes("reasoning"), false);
  assert.equal(summary.includes("private planning"), false);
  assert.equal(summary.includes("result=Visible answer."), true);
});

test("expanded log data strips reasoning and tool blocks from transcript messages", () => {
  const sanitized = sanitizeLogFrameData({
    message: {
      blocks: [
        { block_type: "reasoning", data: { text: "private planning" } },
        { block_type: "tool_use", data: { name: "peers" } },
        { block_type: "text", data: { text: "Visible answer." } },
      ],
    },
    result: "Visible answer.",
  });

  assert.deepEqual(sanitized, {
    message: {
      blocks: [
        { block_type: "text", data: { text: "Visible answer." } },
      ],
    },
    result: "Visible answer.",
  });
});

test("logs hide internal timeline stream snapshot handshakes", () => {
  const visible: ConsoleFrame = {
    id: "frame-visible",
    event: "interaction_complete",
    identity: "incident-commander",
    data: { result: "done" },
  };
  const snapshotComplete: ConsoleFrame = {
    id: "console:662",
    event: "snapshot_complete",
    identity: "_system",
    data: { type: "snapshot_complete", cursor: "console:662" },
  };
  const snapshotStarted: ConsoleFrame = {
    id: "snapshot-started",
    event: "snapshot_started",
    identity: "_system",
    data: { type: "snapshot_started" },
  };

  assert.equal(isLogFrameVisible(visible), true);
  assert.equal(isLogFrameVisible(snapshotComplete), false);
  assert.equal(isLogFrameVisible(snapshotStarted), false);
});

test("user input log summaries render prompt text instead of raw content JSON", () => {
  const frame: ConsoleFrame = {
    id: "frame-user-input",
    event: "user_input",
    identity: "incident-commander",
    data: {
      content: [
        { text: "You have been spawned as incident-commander." },
      ],
      source_event_type: "session_history",
    },
  };

  const summary = summarizeLogFrame(frame);
  assert.equal(summary, "input=You have been spawned as incident-commander.");
  assert.equal(summary.includes("[{\"text\""), false);
});
