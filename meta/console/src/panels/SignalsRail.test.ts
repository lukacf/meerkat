import assert from "node:assert/strict";
import test from "node:test";
import { buildSignalGroupsForTest } from "./SignalsRail";
import type { ConsoleFrame } from "../types";
import { MEERKAT_1608_KICKOFF_NOTICES } from "../lib/fixtures/meerkat-1608-kickoff-notices";

test("signals rail deduplicates live and history copies of the same visible reply", () => {
  const frames: ConsoleFrame[] = [
    {
      id: "live-reply",
      event: "interaction_complete",
      identity: "incident-commander",
      interactionId: "turn-image-1",
      timestampMs: Date.now(),
      data: {
        result: "Image generation succeeded. Peer forwarding to scribe was sent with the generated image reference; ack not confirmed. +1 -0",
      },
    },
    {
      id: "scribe-comms",
      event: "system_notice",
      identity: "scribe",
      timestampMs: Date.now() - 1_000,
      sourceKind: "session_history",
      data: {
        kind: "comms",
        blocks: [{
          type: "comms",
          kind: "message",
          direction: "incoming",
          peer: {
            id: "incident-command-center/commander/incident-commander",
            display_name: "incident-command-center/commander/incident-commander",
          },
          request_id: "comms-1",
          content: [{ type: "text", text: "QA smoke: generated a synthetic CardinalPay status badge image." }],
        }],
      },
    },
    {
      id: "history-reply",
      event: "interaction_complete",
      identity: "incident-commander",
      interactionId: "turn-image-1",
      timestampMs: Date.now() - 2_000,
      sourceKind: "session_history",
      data: {
        message: {
          role: "block_assistant",
          blocks: [{
            block_type: "text",
            data: {
              text: "Image generation succeeded. Peer forwarding to scribe was sent with the generated image reference; ack not confirmed.",
            },
          }],
        },
      },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  const commanderReplies = groups.filter((group) =>
    group.title === "Incident Commander replied"
    && group.detail.startsWith("Image generation succeeded")
  );

  assert.equal(commanderReplies.length, 1);
});

test("signals rail merges non-adjacent frames from the same turn", () => {
  const frames: ConsoleFrame[] = [
    {
      id: "reply",
      event: "interaction_complete",
      identity: "incident-commander",
      interactionId: "turn-1",
      timestampMs: Date.now(),
      data: { result: "Worker created successfully." },
    },
    {
      id: "other-agent",
      event: "interaction_complete",
      identity: "scribe",
      interactionId: "turn-scribe",
      timestampMs: Date.now() - 500,
      data: { result: "Logged the worker creation." },
    },
    {
      id: "prompt",
      event: "user_input",
      identity: "incident-commander",
      interactionId: "turn-1",
      timestampMs: Date.now() - 1_000,
      data: { content: "Create a worker." },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  const commanderTurn = groups.find((group) => group.id === "interaction:turn-1");

  assert.equal(commanderTurn?.title, "Turn activity");
  assert.equal(commanderTurn?.items.length, 2);
});

test("signals rail previews never leak the meerkat 0.7.1 peer transport projection", () => {
  const projection =
    "Peer request from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c"
    + " (display_name: incident-command-center/commander/incident-commander)"
    + " (id: 964020b4-c9b6-4c31-ba6c-30598279b388)\n"
    + "Intent: mob.kickoff_started\n"
    + "Params: {\n"
    + "  \"peer_spec\": {\n"
    + "    \"address\": \"inproc://incident-command-center/commander/incident-commander\",\n"
    + "    \"pubkey\": [20, 129, 97, 58, 74, 93, 150, 7]\n"
    + "  }\n"
    + "}\n"
    + "Request ID: 964020b4-c9b6-4c31-ba6c-30598279b388\n"
    + "\n"
    + "This is a correlated peer request. Reply with send_response with arguments"
    + " {\"in_reply_to\":\"964020b4-c9b6-4c31-ba6c-30598279b388\",\"status\":\"completed\"}."
    + " Do not answer this request with send_message.";

  const frames: ConsoleFrame[] = [
    {
      id: "kickoff-notice",
      event: "system_notice",
      identity: "scribe",
      timestampMs: Date.now(),
      sourceKind: "session_history",
      data: {
        kind: "comms",
        blocks: [{
          type: "comms",
          kind: "request",
          direction: "incoming",
          peer: {
            id: "6f6114cd-2cf7-590f-a172-0e36feacd12c",
            display_name: "incident-command-center/commander/incident-commander",
          },
          request_id: "964020b4-c9b6-4c31-ba6c-30598279b388",
          intent: "mob.kickoff_started",
          summary: "Peer request: mob.kickoff_started",
          content: [{ type: "text", text: projection }],
        }],
      },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  const kickoff = groups.find((group) => group.title === "Kickoff started");

  assert.ok(kickoff, "a kickoff notice produces its typed kickoff signal");
  assert.equal(kickoff?.detail, "Incident Commander");
  for (const group of groups) {
    for (const item of group.items) {
      assert.ok(
        !/peer_id|pubkey|send_response/i.test(`${item.label} ${item.detail}`),
        `signal preview must not leak transport scaffold: ${item.label} ${item.detail}`,
      );
    }
  }
});

test("signals rail surfaces quarantined memory writes as a warning", () => {
  const frames: ConsoleFrame[] = [
    {
      id: "mem-quarantine",
      event: "memory.write.quarantined",
      identity: "distiller",
      timestampMs: Date.now(),
      data: { realm: "default", author: "agent", reason: "low trust source" },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  const signal = groups.find((group) => group.title === "Memory write quarantined");

  assert.ok(signal, "quarantined write should surface as a signal");
  assert.equal(signal?.severity, "warning");
  assert.ok(!/[{}]/.test(signal?.detail || ""), "signal detail must not leak JSON");
});

test("signals rail surfaces blocked quarantine releases as a warning with adapter copy", () => {
  const frames: ConsoleFrame[] = [
    {
      id: "mem-release-blocked",
      event: "memory.quarantine.release_blocked",
      identity: "steward",
      timestampMs: Date.now(),
      data: { realm: "default", record_id: "m-42", verdict: "release", class: "api_key" },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  const signal = groups.find((group) => group.title === "Quarantine release blocked");

  assert.ok(signal, "release_blocked should surface as a signal instead of being dropped");
  assert.equal(signal?.severity, "warning");
  assert.equal(
    signal?.detail,
    "Quarantine release blocked for m-42 — matches secret pattern api_key",
  );
});

test("signals rail drops routine memory dream lifecycle noise", () => {
  const frames: ConsoleFrame[] = [
    {
      id: "dream-start",
      event: "memory.dream.started",
      identity: "steward",
      timestampMs: Date.now(),
      data: { realm: "default", run_id: "run-1" },
    },
    {
      id: "clean-rotate",
      event: "memory.taint.transition",
      identity: "steward",
      timestampMs: Date.now() - 500,
      data: { session_key: "s", kind: "rotated_clean", source: "reset" },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  assert.equal(groups.length, 0, "dream.started and non-tainted transitions are dropped");
});

test("signals rail names a provider failure from meerkat's typed error_report", () => {
  const frames: ConsoleFrame[] = [
    {
      id: "turn-failed",
      event: "interaction_failed",
      identity: "assistant",
      interactionId: "turn-1",
      timestampMs: Date.now(),
      data: {
        type: "run_failed",
        session_id: "s",
        error_report: {
          class: "llm",
          reason: { reason_type: "llm_auth_error" },
          message: "LLM error: authentication failed (401)",
        },
      },
    },
  ];

  const groups = buildSignalGroupsForTest(frames);
  assert.equal(groups.length, 1, "one failed turn is one critical signal");
  const [signal] = groups[0].items;
  assert.equal(signal.label, "Agent turn failed");
  assert.equal(signal.detail, "LLM error: authentication failed (401) (llm_auth_error)");
});

test("signals rail shows a typed kickoff lifecycle notice by its phase, never its notice text", () => {
  // Meerkat #1608's serialized mob.kickoff_failed lifecycle notice.
  const failedNotice = MEERKAT_1608_KICKOFF_NOTICES.find((message) => message.blocks[0].intent === "mob.kickoff_failed");
  assert.ok(failedNotice);
  const frames: ConsoleFrame[] = [{
    id: "kickoff-failed",
    event: "system_notice",
    identity: "scribe",
    timestampMs: Date.now(),
    sourceKind: "session_history",
    data: structuredClone(failedNotice) as unknown as Record<string, unknown>,
  }];
  const groups = buildSignalGroupsForTest(frames);
  const failed = groups.find((group) => group.title === "Kickoff failed");
  assert.ok(failed, "a failed kickoff produces its typed signal");
  assert.equal(failed?.detail, "Delivery Lead (delivery)");
  for (const group of groups) {
    for (const item of group.items) {
      assert.ok(
        !/peer_id|send_response|nothing to answer/i.test(`${item.label} ${item.detail}`),
        `signal preview must not surface lifecycle notice text: ${item.label} ${item.detail}`,
      );
    }
  }
});
