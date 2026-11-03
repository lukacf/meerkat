import assert from "node:assert/strict";
import { test } from "node:test";

import {
  COUNCIL_EXCHANGE_ROW_CAP,
  councilArgsByCallId,
  councilEntryFromFrame,
  councilStatusFromExitReason,
  isCouncilToolFrame,
} from "./council-entries";
import { conversationEntryText } from "@console-core";
import type { ConsoleFrame } from "../types";

const IDENTITY = { id: "planner", label: "Planner", role: "assistant" as const };

function resultFrame(result: unknown, overrides: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return {
    id: "f1",
    event: "tool_execution_completed",
    timestampMs: 1_756_000_000_000,
    data: { name: "council", tool_call_id: "call-1", result },
    ...overrides,
  } as ConsoleFrame;
}

function sealedResult(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    result: {
      council_id: "c-1",
      exit_reason: { reason: "completed" },
      rounds_completed: 2,
      participants: [
        {
          order: 0,
          role: "critic",
          source_mob_id: "m1",
          source_identity: "a",
          target_identity: "a#fork",
          seated: true,
        },
      ],
      exchanges: [
        {
          round: 0,
          sequence: 0,
          participant_order: 0,
          target_identity: "a#fork",
          outcome: { status: "completed", text: "yes", truncated: false },
        },
      ],
      merge: { kind: "bounded_text_summary", finalizer: "a#fork", text: "agreed", truncated: false },
      truncated_exchange_count: 0,
      durability: "process_bound",
      concluded_at: "2026-08-29T10:00:00Z",
      ...overrides,
    },
    cleanup: { debts: [], budget_exhausted: false },
    replayed: false,
  };
}

test("a council tool frame is recognised so it never renders as a generic tool row", () => {
  assert.equal(isCouncilToolFrame(resultFrame(sealedResult())), true);
  assert.equal(
    isCouncilToolFrame({ id: "x", event: "tool_call", data: { name: "delegate" } } as ConsoleFrame),
    false,
  );
  // Right name, wrong event class: not a tool lifecycle frame.
  assert.equal(
    isCouncilToolFrame({ id: "x", event: "agent_message", data: { name: "council" } } as ConsoleFrame),
    false,
  );
});

test("a sealed completed council folds into a card entry", () => {
  const entry = councilEntryFromFrame(resultFrame(sealedResult()), IDENTITY);
  assert.ok(entry);
  assert.equal(entry.kind, "council");
  assert.equal(entry.id, "council:c-1");
  assert.equal(entry.status, "completed");
  assert.equal(entry.roundsCompleted, 2);
  assert.equal(entry.participants.length, 1);
  assert.equal(entry.exchanges[0]?.status, "completed");
  assert.equal(entry.mergeText, "agreed");
  assert.equal(entry.durability, "process_bound");
});

test("the topic comes from the CALL frame, not the result frame", () => {
  // The result carries no topic; without the args map the card would render
  // its fallback title, which is the one field an operator scans for first.
  const frames = [
    {
      id: "f0",
      event: "tool_call",
      data: {
        name: "council",
        tool_call_id: "call-1",
        arguments: JSON.stringify({ topic: "Ship or hold 0.8.28?", participants: [] }),
      },
    } as ConsoleFrame,
  ];
  const args = councilArgsByCallId(frames);
  const entry = councilEntryFromFrame(resultFrame(sealedResult()), IDENTITY, args);
  assert.equal(entry?.topic, "Ship or hold 0.8.28?");

  const without = councilEntryFromFrame(resultFrame(sealedResult()), IDENTITY);
  assert.equal(without?.topic, "Council");
});

test("budget exits are 'bounded', not 'failed'", () => {
  // A council that hit a cap the caller set ran correctly. Colouring it the
  // same as a seating failure teaches operators to ignore the colour.
  assert.equal(councilStatusFromExitReason("max_exchanges_reached"), "bounded");
  assert.equal(councilStatusFromExitReason("deadline_exceeded"), "bounded");
  assert.equal(councilStatusFromExitReason("completed"), "completed");
});

test("an UNKNOWN exit reason fails closed rather than reading as success", () => {
  // TemporaryCouncilExitReason is #[non_exhaustive] upstream. A variant added
  // later must not render green here.
  assert.equal(councilStatusFromExitReason("some_future_variant"), "failed");
  assert.equal(councilStatusFromExitReason(undefined), "pending");
});

test("a failed exit carries its typed detail into the card", () => {
  const entry = councilEntryFromFrame(
    resultFrame(sealedResult({
      exit_reason: {
        reason: "exchange_failed",
        round: 1,
        target_identity: "b#fork",
        detail: "provider timeout",
      },
    })),
    IDENTITY,
  );
  assert.equal(entry?.status, "failed");
  assert.equal(entry?.exitReason, "exchange_failed");
  assert.ok(entry?.exitDetail?.includes("provider timeout"));
  assert.ok(entry?.exitDetail?.includes("round 2"), "round is rendered 1-based for humans");
});

test("a fork instruction refusal renders its typed cause as the exit detail", () => {
  const entry = councilEntryFromFrame(
    resultFrame(sealedResult({
      exit_reason: {
        reason: "participant_fork_instruction_refused",
        participant_order: 1,
        refusal: { cause: { kind: "policy_refuses" }, keys: [["host", "persona"]] },
      },
    })),
    IDENTITY,
  );
  assert.equal(entry?.status, "failed");
  assert.equal(entry?.exitReason, "participant_fork_instruction_refused");
  assert.ok(entry?.exitDetail?.includes("slot 1"));
  assert.ok(entry?.exitDetail?.includes("fork instruction refusal: policy refuses (1 key)"));
});

test("an exchange with no observed terminal stays pending, never assumed complete", () => {
  // A receipt left pending is what a coordinator crash looks like; silently
  // upgrading it to completed would hide exactly that.
  const entry = councilEntryFromFrame(
    resultFrame(sealedResult({
      exchanges: [{
        round: 0,
        sequence: 0,
        participant_order: 0,
        target_identity: "a#fork",
        outcome: { status: "pending" },
      }],
    })),
    IDENTITY,
  );
  assert.equal(entry?.exchanges[0]?.status, "pending");

  const missing = councilEntryFromFrame(
    resultFrame(sealedResult({
      exchanges: [{ round: 0, sequence: 0, participant_order: 0, target_identity: "a#fork" }],
    })),
    IDENTITY,
  );
  assert.equal(missing?.exchanges[0]?.status, "pending");
});

test("cleanup debt is reported separately and never folded into the status", () => {
  // A council can seal a good result and still fail to destroy its temporary
  // mob. Folding that into `failed` would misreport the answer.
  const payload = sealedResult();
  (payload as Record<string, unknown>).cleanup = {
    debts: [{ subject: "mob-tmp-1", detail: "destroy refused" }],
    budget_exhausted: true,
  };
  const entry = councilEntryFromFrame(resultFrame(payload), IDENTITY);
  assert.equal(entry?.status, "completed", "the answer is still good");
  assert.equal(entry?.cleanupDebts?.length, 1);
  assert.equal(entry?.cleanupBudgetExhausted, true);
});

test("exchanges past the render cap are counted, never silently dropped", () => {
  const many = Array.from({ length: COUNCIL_EXCHANGE_ROW_CAP + 5 }, (_unused, index) => ({
    round: 0,
    sequence: index,
    participant_order: 0,
    target_identity: "a#fork",
    outcome: { status: "completed", text: `turn ${index}` },
  }));
  const entry = councilEntryFromFrame(resultFrame(sealedResult({ exchanges: many })), IDENTITY);
  assert.equal(entry?.exchanges.length, COUNCIL_EXCHANGE_ROW_CAP);
  assert.equal(entry?.exchangeOverflowCount, 5);
});

test("an unparseable or resultless frame yields no card, so the tool row survives", () => {
  assert.equal(councilEntryFromFrame(resultFrame("not json"), IDENTITY), null);
  assert.equal(councilEntryFromFrame(resultFrame({ replayed: true }), IDENTITY), null);
  // A result without a council id cannot key a stable card.
  assert.equal(
    councilEntryFromFrame(resultFrame({ result: { exit_reason: { reason: "completed" } } }), IDENTITY),
    null,
  );
});

test("a JSON-string result is parsed the same as a decoded object", () => {
  const asString = resultFrame(JSON.stringify(sealedResult()));
  const entry = councilEntryFromFrame(asString, IDENTITY);
  assert.equal(entry?.councilId, "c-1");
});

test("artifact claims survive as claims, with no resolution implied", () => {
  const payload = sealedResult();
  const result = (payload as Record<string, unknown>).result as Record<string, unknown>;
  result.merge = {
    kind: "bounded_text_summary",
    text: "done",
    artifacts: [{ uri: "blob://x", media_type: "text/plain", digest: "abc123def456", byte_len: 12 }],
  };
  const entry = councilEntryFromFrame(resultFrame(payload), IDENTITY);
  assert.equal(entry?.artifactClaims?.length, 1);
  assert.equal(entry?.artifactClaims?.[0]?.uri, "blob://x");
  assert.equal(entry?.artifactClaims?.[0]?.byteLen, 12);
});

test("a replayed council is marked, so a cached answer is not read as a fresh one", () => {
  const payload = sealedResult();
  (payload as Record<string, unknown>).replayed = true;
  const entry = councilEntryFromFrame(resultFrame(payload), IDENTITY);
  assert.equal(entry?.replayed, true);
});

test("copy text is not empty - the card renders fine while copy silently yielded nothing", () => {
  // Regression: ConversationTimelineEntry falls through to
  // `copyText || text || blocks`, and a council entry has none of the three.
  // The card looked correct, which is what made the empty copy easy to miss.
  const entry = councilEntryFromFrame(resultFrame(sealedResult()), IDENTITY);
  assert.ok(entry);
  const text = conversationEntryText(entry);
  assert.notEqual(text, "");
  assert.ok(text.includes("completed"));
});

test("an artifact claim stays marked as a claim in COPIED text", () => {
  // Pasting a bare uri into a ticket is exactly how an unverified claim
  // becomes a fact somewhere it cannot be challenged.
  const payload = sealedResult();
  const result = (payload as Record<string, unknown>).result as Record<string, unknown>;
  result.merge = { kind: "bounded_text_summary", text: "done", artifacts: [{ uri: "blob://x" }] };
  const entry = councilEntryFromFrame(resultFrame(payload), IDENTITY);
  assert.ok(entry);
  assert.ok(conversationEntryText(entry).includes("claimed artifact: blob://x"));
});

test("both result-bearing frame events parse, which is why the adapter must dedupe", () => {
  // tool_result_received AND tool_execution_completed can each carry the
  // sealed result and each yields the SAME `council:{id}` key. Pushing both
  // duplicates the card and collides the React key; the adapter holds a
  // seen-set. This pins the precondition that makes that necessary.
  const ids = ["tool_result_received", "tool_execution_completed"].map((event) => {
    const frame = resultFrame(sealedResult(), { id: event, event });
    return councilEntryFromFrame(frame, IDENTITY)?.id;
  });
  assert.deepEqual(ids, ["council:c-1", "council:c-1"]);
});
