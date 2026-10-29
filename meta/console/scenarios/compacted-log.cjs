"use strict";
const assert = require("node:assert/strict");

// What a console timeline read returns of a member's streamed blocks. The
// member's text, and each of its reasoning blocks, reaches the console log as
// one delta row per provider chunk (`text_delta`, `reasoning_delta`) and then
// whole, in one complete row (`text_complete`, `reasoning_complete`). Once the
// complete row is stored, the log removes the block's delta rows but the
// first, which keeps the block's place (`ConsoleLogStore::compact_message_deltas`).
// A live stream still delivers every delta as it arrives; a later timeline
// read, or a replay from the log, carries the block's first delta row and its
// complete row.

const DELTA_KINDS = new Set(["text_delta", "reasoning_delta"]);
const BLOCK_KIND = new Map([
  ["text_delta", "text"], ["text_complete", "text"],
  ["reasoning_delta", "reasoning"], ["reasoning_complete", "reasoning"],
]);

/** The streamed block a row belongs to, keyed as the log keys it; undefined for any other row. */
function streamedBlock(frame) {
  const kind = BLOCK_KIND.get(frame.kind);
  const payload = frame.payload ?? {};
  if (!kind || frame.source?.kind !== "console_event" || typeof payload.assistant_message_id !== "string"
    || !Number.isSafeInteger(payload.source_sequence) || payload.source_sequence < 0) return undefined;
  return JSON.stringify([frame.runtime_key ?? null, frame.identity ?? null, kind, payload.assistant_message_id]);
}

/**
 * `frames`, in the cursor order a live stream delivered them, as the log keeps
 * them: each completed block's delta rows after its first are removed.
 */
function compactedLog(frames) {
  // Per block: the delta rows after its first, while it streams.
  const streaming = new Map();
  const removed = new Set();
  for (const frame of frames) {
    const block = streamedBlock(frame);
    if (block === undefined) continue;
    if (DELTA_KINDS.has(frame.kind)) {
      if (streaming.has(block)) streaming.get(block).push(frame);
      else streaming.set(block, []);
    } else {
      for (const delta of streaming.get(block) ?? []) removed.add(delta);
      streaming.delete(block);
    }
  }
  return frames.filter(frame => !removed.has(frame));
}

/**
 * One completed text block in a timeline read: its first `text_delta` row,
 * which begins the text, and its `text_complete` row, which carries all of it.
 */
function assertCompactedText(frames, source, label) {
  const rows = frames.filter(frame => frame.source?.kind === "console_event"
    && (frame.kind === "text_delta" || frame.kind === "text_complete"));
  assert.deepEqual(rows.map(frame => frame.kind), ["text_delta", "text_complete"],
    `${label}: the log keeps the text's first delta row and its complete row`);
  const [first, complete] = rows;
  assert.equal(complete.payload.content, source, `${label}: exact complete text`);
  assert(typeof first.payload.delta === "string" && first.payload.delta.length > 0 && source.startsWith(first.payload.delta),
    `${label}: the kept delta row begins the text`);
  assert.equal(first.payload.assistant_message_id, complete.payload.assistant_message_id,
    `${label}: both rows name one assistant message`);
  return { first, complete };
}

module.exports = { compactedLog, assertCompactedText };
