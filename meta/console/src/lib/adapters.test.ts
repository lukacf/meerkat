import assert from "node:assert/strict";
import test from "node:test";
import { normalizePendingApproval as normalizePendingApprovalForInputTest, approvalMatchesConversation as approvalMatchesInputConversation } from "../../../packages/console-core/src/pending-approvals";

import {
  appendOptimisticConversationEntry,
  buildActivityRailViewState,
  buildQuickPromptSuggestions,
  buildRoutingSectionView,
  buildSidebarViewState,
  buildWorkGraphOperatorResultFrame,
  createWorkGraphHydrationGate,
  describeMemoryTimelineEvent,
  inferResponsePhaseFromFrames,
  mapFramesToTimelineEntries,
  mergeConversationFrames,
  optimisticUserMessageForPanel,
  resolvePanelResponsePhase,
  sortConversationTimelineEntries,
  stripPeerTransportScaffold,
  systemNoticeClearsBusyState,
  WORKGRAPH_CARD_ITEM_ROW_LIMIT,
} from "./adapters";
import { MEERKAT_1608_KICKOFF_NOTICES } from "./fixtures/meerkat-1608-kickoff-notices";
import { describeFailure, summarizeFailureData } from "./failure-summary";
import type { ConsoleFrame } from "../types";

test("authorization feedback projects a typed operation refusal without ending the active run", () => {
  const frames = [
    { id: "auth-start", event: "run_started", data: {}, interactionId: "auth-run" },
    { id: "auth-refused", event: "system_notice", interactionId: "auth-run", data: { message: {
      role: "system_notice", kind: "generic", body: "private-owner-detail-canary",
      blocks: [{ type: "runtime_notice", category: "operation_refused", payload: { code: "operation_refused" } }],
    } } },
  ];
  const entries = mapFramesToTimelineEntries({ agent_id: "a", label: "Agent" }, frames);
  const feedback = entries.find(entry => "operationFeedback" in entry && entry.operationFeedback)?.operationFeedback;
  assert.equal(feedback?.kind, "operation-refused");
  assert.doesNotMatch(JSON.stringify(feedback), /private-owner-detail-canary/);
  assert.equal(inferResponsePhaseFromFrames(frames, null), "waiting");
  assert.equal(systemNoticeClearsBusyState(frames[1]), false);
  const continued = [...frames, { id: "auth-answer", event: "text_delta", interactionId: "auth-run", data: { delta: "Continuing with the permitted action." } }];
  assert.equal(inferResponsePhaseFromFrames(continued, null), "generating");
  assert.match(JSON.stringify(mapFramesToTimelineEntries({ agent_id: "a", label: "Agent" }, continued)), /Continuing with the permitted action/);
});

test("authorization feedback distinguishes actual tool refusal from ordinary text mentioning refusal", () => {
  const tool = { id: "auth-tool", event: "tool_result_received", data: { id: "call", name: "calendar", is_error: true,
    content: [{ type: "text", text: JSON.stringify({ error: "operation_refused", message: "operation refused" }) }] } };
  const entries = mapFramesToTimelineEntries({ agent_id: "a", label: "Agent" }, [tool]);
  assert.equal(entries.filter(entry => "operationFeedback" in entry && entry.operationFeedback?.kind === "operation-refused").length, 1);
  const ordinary = mapFramesToTimelineEntries({ agent_id: "a", label: "Agent" }, [{
    ...tool, id: "ordinary", data: { ...tool.data, is_error: false, content: [{ type: "text", text: "operation_refused is an example string" }] },
  }]);
  assert.equal(ordinary.some(entry => "operationFeedback" in entry && entry.operationFeedback), false);
});

test("authorization feedback names no permission cause for an operation refusal", () => {
  // The host refuses an operation for a policy denial, for an authorization
  // that changed after the operation was prepared and for request facts it
  // could not evaluate, and sends all of them alike: the runtime notice and
  // the tool error name the refusal, never its reason. The card says only
  // what holds for every one of them.
  const expected = {
    kind: "operation-refused",
    title: "Action refused",
    detail: "This action was refused under the current authorization, so it did not run. The agent can continue with other work.",
  };
  const notice = (id: string, payload: Record<string, unknown>) => ({ id, event: "system_notice", data: { message: {
    role: "system_notice", kind: "generic", body: "The requested model operation is unavailable under current authorization.",
    blocks: [{ type: "runtime_notice", category: "operation_refused", payload }],
  } } });
  const tool = (id: string, refusal: Record<string, unknown>) => ({ id, event: "tool_result_received", data: {
    id: "call", name: "calendar", is_error: true, content: [{ type: "text", text: JSON.stringify(refusal) }],
  } });
  const message = "operation unavailable under current authorization";
  for (const frame of [
    notice("notice", { code: "operation_refused" }),
    tool("tool", { error: "operation_refused", message }),
    // A reason no host contract defines falls back to the same card, and is
    // never shown.
    notice("notice-with-reason", { code: "operation_refused", kind: "reprepare_required" }),
    tool("tool-with-reason", { error: "operation_refused", message, data: { kind: "malformed_facts" } }),
  ]) {
    const entries = mapFramesToTimelineEntries({ agent_id: "a", label: "Agent" }, [frame]);
    const feedback = entries.flatMap(entry => "operationFeedback" in entry && entry.operationFeedback ? [entry.operationFeedback] : []);
    assert.deepEqual(feedback, [expected], frame.id);
    assert.doesNotMatch(JSON.stringify(feedback), /Permission denied|not permitted|reprepare|malformed/, frame.id);
  }
});

test("authorization feedback keeps audit observation failure distinct and nonterminal", () => {
  const frames = [
    { id: "audit-start", event: "run_started", data: {} },
    { id: "audit-feedback", event: "operation_observation_failed", data: { operation_id: "op-1", phase: "outcome", secret: "audit-private-canary" } },
  ];
  const entries = mapFramesToTimelineEntries({ agent_id: "a", label: "Agent" }, frames);
  const feedback = entries.find(entry => "operationFeedback" in entry && entry.operationFeedback)?.operationFeedback;
  assert.equal(feedback?.kind, "audit-unavailable");
  assert.equal(feedback?.operationId, "op-1");
  assert.doesNotMatch(JSON.stringify(feedback), /audit-private-canary/);
  assert.equal(inferResponsePhaseFromFrames(frames, null), "waiting");
});
import {
  mapFramesToTimelineEntries as mapFramesToTimelineEntriesShared,
  inferResponsePhaseFromFrames as inferResponsePhaseFromFramesShared,
  resolvePanelResponsePhase as resolvePanelResponsePhaseShared,
} from "../../../packages/console-core/src/adapters";
import {
  conversationRichBlockCopyText,
  groupConversationTimelineEntries, describeMemoryTimelineEvent as describeMemoryTimelineEventCore } from "@console-core";

const confinementCauses = ["invalid_requirement", "invalid_launch", "unsupported_requirement",
  "backend_unavailable", "preparation_failed"] as const;

const reviewOutcomes = [
  ["review_unsatisfied", "review-unsatisfied", "reviewUnsatisfied", [
    "denied", "escalated", "context_changed", "human_consent_required", "already_consumed",
  ]],
  ["review_unavailable", "review-unavailable", "reviewUnavailable", [
    "deadline_expired", "reviewer_failed", "reviewer_missing", "unsupported_entry", "owner_unavailable", "dispatch_ended",
  ]],
] as const;

for (const [name, map, phase] of [
  ["stock", mapFramesToTimelineEntries, inferResponsePhaseFromFrames],
  ["shared", mapFramesToTimelineEntriesShared, inferResponsePhaseFromFramesShared],
] as const) {
  test(`${name} native review feedback preserves the exact call, permitted sibling and active phase`, () => {
    for (const [error, category, reasonField, reasons] of reviewOutcomes) {
      for (const kind of reasons) {
        for (const event of ["tool_result_received", "tool_execution_completed"]) {
          const callId = "  exact-reviewed-call  ";
          const result = { id: "review-result", event, runId: "run", data: {
            id: callId, name: "write_file", is_error: true,
            content: [{ type: "text", text: JSON.stringify({ error, message: "private-review-canary", data: { kind } }) }],
            ...(event === "tool_execution_completed" ? { duration_ms: 0 } : {}),
          } };
          const pairedEvent = event === "tool_result_received" ? "tool_execution_completed" : "tool_result_received";
          const paired = { ...result, id: "review-replay", event: pairedEvent, data: {
            id: callId, name: "write_file", is_error: true, content: result.data.content,
            ...(pairedEvent === "tool_execution_completed" ? { duration_ms: 0 } : {}),
          } };
          const frames = [{ id: "start", event: "run_started", runId: "run", data: {} },
            { id: "review-start", event: "tool_call_requested", runId: "run", data: { id: callId, name: "write_file", args: {} } },
            result,
            paired,
            { id: "sibling-start", event: "tool_execution_started", runId: "run", data: { id: "permitted", name: "read_file" } },
            { id: "sibling", event: "tool_execution_completed", runId: "run", data: {
              id: "permitted", name: "read_file", is_error: false, duration_ms: 1,
              content: [{ type: "text", text: "Permitted sibling result" }],
            } }];
          const before = JSON.stringify(frames);
          const entries = map(null, frames);
          const feedback = entries.flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
          assert.equal(feedback.length, 1, `${error}/${kind}/${event}`);
          assert.equal(feedback[0].kind, category);
          assert.equal(feedback[0][reasonField], kind);
          assert.equal(feedback[0].toolCallId, callId);
          assert.equal(feedback[0].operationId, undefined);
          assert.doesNotMatch(JSON.stringify(entries), /private-review-canary/);
          if (kind === "already_consumed" || kind === "dispatch_ended") {
            assert.match(feedback[0].detail, /does not establish.*execution result/);
            assert.doesNotMatch(feedback[0].detail, /did not run|never entered|no effect/);
          }
          assert.equal(phase(frames, null), "waiting");
          const continued = [...frames, { id: "answer", event: "text_delta", runId: "run", data: { delta: "Continuing with permitted work." } }];
          assert.equal(phase(continued, null), "generating");
          assert.match(JSON.stringify(map(null, continued)), /Permitted sibling result/);
          assert.match(JSON.stringify(map(null, continued)), /Continuing with permitted work/);
          assert.equal(phase([...continued, { id: "done", event: "run_completed", runId: "run", data: {} }], null), null);
          assert.equal(JSON.stringify(frames), before);
        }
      }
    }
  });

  test(`${name} native consent references remain optional display data without changing local settlement`, () => {
    const approvalId = "b6503200-06b6-4157-afc0-2a3c8d124178";
    for (const reference of [approvalId, approvalId.toUpperCase(), undefined, null, "", "not-an-approval",
      ` ${approvalId}`, `${approvalId}\n`, approvalId.replaceAll("-", ""), [approvalId], { approval_id: approvalId }]) {
      const data = { kind: "human_consent_required", ...(reference === undefined ? {} : { approval_id: reference }) };
      const result = { id: "consent-result", event: "tool_result_received", runId: "run", data: {
        id: "reviewed-call", name: "write_file", is_error: true,
        content: [{ type: "text", text: JSON.stringify({ error: "review_unsatisfied", message: "private-consent-canary", data }) }],
      } };
      const frames = [{ id: "start", event: "run_started", runId: "run", data: {} }, result,
        { ...result, id: "consent-completed", event: "tool_execution_completed", data: { ...result.data, duration_ms: 0 } },
        { id: "sibling-start", event: "tool_execution_started", runId: "run", data: { id: "permitted", name: "read_file" } },
        { id: "sibling", event: "tool_execution_completed", runId: "run", data: {
          id: "permitted", name: "read_file", is_error: false, duration_ms: 1,
          content: [{ type: "text", text: "Permitted sibling result" }],
        } }];
      const before = JSON.stringify(frames);
      const feedback = map(null, frames).flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
      assert.equal(feedback.length, 1);
      assert.equal(feedback[0].kind, "review-unsatisfied");
      assert.equal(feedback[0].reviewUnsatisfied, "human_consent_required");
      assert.equal(feedback[0].approvalId, reference === approvalId || reference === approvalId.toUpperCase() ? reference : undefined);
      assert.equal(feedback[0].toolCallId, "reviewed-call");
      assert.equal(feedback[0].operationId, undefined);
      assert.doesNotMatch(JSON.stringify(feedback), /private-consent-canary|Approved|Awaiting approval/);
      assert.equal(phase(frames, null), "waiting");
      const continued = [...frames, { id: "answer", event: "text_delta", runId: "run", data: { delta: "Continuing with permitted work." } }];
      assert.equal(phase(continued, null), "generating");
      assert.match(JSON.stringify(map(null, continued)), /Permitted sibling result/);
      assert.match(JSON.stringify(map(null, continued)), /Continuing with permitted work/);
      assert.equal(phase([...continued, { id: "done", event: "run_completed", runId: "run", data: {} }], null), null);
      assert.equal(JSON.stringify(frames), before);
    }
    for (const [error, kind, data] of [
      ["review_unsatisfied", "denied", { kind: "denied" }],
      ["review_unavailable", "reviewer_missing", { kind: "reviewer_missing", approval_id: approvalId }],
    ] as const) {
      const frames = [{ id: "unreferenced", event: "tool_result_received", data: { id: approvalId, name: "write_file", is_error: true,
        content: [{ type: "text", text: JSON.stringify({ error, message: approvalId, approval_id: approvalId, data }) }],
      } }];
      const feedback = map(null, frames).flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
      assert.equal(feedback.length, 1, kind);
      assert.equal(feedback[0].approvalId, undefined, "never infer a reference from a call ID, prose, another error family or another field");
    }
    const modelNotice = { id: "model", event: "system_notice", data: { message: {
      role: "system_notice", kind: "generic", body: approvalId, blocks: [{ type: "runtime_notice", category: "operation_review_required",
        payload: { code: "REVIEW_UNSATISFIED", approval_id: approvalId,
          review: { review: "unsatisfied", kind: "human_consent_required", approval_id: approvalId } },
      }],
    } } };
    const noticeFeedback = map(null, [modelNotice]).flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
    assert.equal(noticeFeedback.length, 1);
    assert.equal(noticeFeedback[0].approvalId, undefined, "the model-notice contract has not acquired a consent reference");
    const consumed = map(null, [{ id: "consumed", event: "tool_result_received", data: { id: "consumed-call", name: "write_file", is_error: true,
      content: [{ type: "text", text: JSON.stringify({ error: "review_unsatisfied", message: "review already used",
        data: { kind: "already_consumed", approval_id: approvalId } }) }],
    } }]).flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
    assert.equal(consumed.length, 1);
    assert.equal(consumed[0].approvalId, approvalId);
    assert.match(consumed[0].detail, /does not establish.*execution result/);
    assert.doesNotMatch(consumed[0].detail, /did not run|never entered|no effect/);
  });

  test(`${name} native model review notices require matching typed codes and add no action reference`, () => {
    for (const [error, category, reasonField, reasons] of reviewOutcomes) {
      for (const kind of reasons) {
        for (const wrapped of [false, true]) {
          const message = { role: "system_notice", kind: "generic", body: "private-model-review-canary", blocks: [{
            type: "runtime_notice", category: "operation_review_required", detail: null,
            payload: { code: error.toUpperCase(), review: { review: error.slice("review_".length), kind } },
          }] };
          const notice = { id: "model-review", event: "system_notice", runId: "run", data: wrapped ? { message } : message };
          const frames = [{ id: "start", event: "run_started", runId: "run", data: {} }, notice];
          const entries = map(null, frames);
          const feedback = entries.flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
          assert.equal(feedback.length, 1);
          assert.equal(feedback[0].kind, category);
          assert.equal(feedback[0][reasonField], kind);
          assert.equal(feedback[0].toolCallId, undefined);
          assert.equal(feedback[0].operationId, undefined);
          assert.doesNotMatch(JSON.stringify(entries), /private-model-review-canary/);
          assert.equal(phase(frames, null), "waiting");
          assert.equal(systemNoticeClearsBusyState(notice), false);
        }
      }
    }
  });

  test(`${name} malformed native review data stays generic and audit failure stays infrastructure`, () => {
    const valid = { error: "review_unsatisfied", message: "review detail", data: { kind: "denied" } };
    const invalid = [
      { ...valid, data: { kind: "future_reason" } }, { ...valid, data: { kind: "reviewer_failed" } },
      { ...valid, data: { kind: null } }, { ...valid, data: { kind: ["denied"] } },
      { ...valid, data: {} }, { ...valid, data: null }, { ...valid, message: null },
      { error: "review_unsatisfied", data: { kind: "denied" } },
      { ...valid, error: "review_unavailable" },
      { error: "operation_observation_unavailable", message: "review_unsatisfied", data: { kind: "denied" } },
    ];
    const result = { id: "review", event: "tool_result_received", data: { id: "call", name: "write_file", is_error: true,
      content: [{ type: "text", text: JSON.stringify(valid) }],
    } };
    for (const frame of [
      ...invalid.map(payload => ({ ...result, data: { ...result.data, content: [{ type: "text", text: JSON.stringify(payload) }] } })),
      { ...result, data: { ...result.data, is_error: false } },
      { ...result, data: { ...result.data, id: "" } },
      { ...result, data: { ...result.data, content: [{ type: "text", text: "review_unsatisfied denied" }] } },
      { ...result, data: { ...result.data, content: [...result.data.content, { type: "text", text: "extra" }] } },
    ]) {
      assert.equal(map(null, [frame]).some(entry => entry.kind === "message" && entry.operationFeedback), false);
    }
    for (const payload of [
      { code: "REVIEW_UNAVAILABLE", review: { review: "unsatisfied", kind: "denied" } },
      { code: "REVIEW_UNSATISFIED", review: { review: "unsatisfied", kind: "owner_unavailable" } },
      { code: "REVIEW_UNSATISFIED", review: { review: "unsatisfied", kind: "future_reason" } },
      { code: "review_unsatisfied", review: { review: "unsatisfied", kind: "denied" } },
      { code: "REVIEW_UNSATISFIED", review: [{ review: "unsatisfied", kind: "denied" }] },
      { code: "REVIEW_UNSATISFIED" },
    ]) {
      const notice = { id: "bad-notice", event: "system_notice", data: { message: {
        role: "system_notice", kind: "generic", body: "ordinary notice",
        blocks: [{ type: "runtime_notice", category: "operation_review_required", payload }],
      } } };
      assert.equal(map(null, [notice]).some(entry => entry.kind === "message" && entry.operationFeedback), false);
    }
    const frames = [{ id: "start", event: "run_started", data: {} }, {
      id: "audit", event: "operation_observation_failed", data: { operation_id: "actual-operation", phase: "outcome" },
    }];
    const feedback = map(null, frames).flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
    assert.equal(feedback.length, 1);
    assert.equal(feedback[0].kind, "audit-unavailable");
    assert.equal(feedback[0].operationId, "actual-operation");
    assert.equal(feedback[0].reviewUnsatisfied, undefined);
    assert.equal(feedback[0].reviewUnavailable, undefined);
    assert.equal(phase(frames, null), "waiting");
  });

  test(`${name} local confinement feedback preserves every cause and exact call without ending the run`, () => {
    for (const refusal of confinementCauses) {
      for (const reverse of [false, true]) {
        const launch = { id: "launch", event: "hook_launch_refused", runId: "run", data: {
          hook_id: "guard", point: "pre_tool_execution", tool_use_id: "blocked",
          reason: { reason_code: "confinement_refused", refusal },
        } };
        const result = { id: "result", event: "tool_result_received", runId: "run", data: {
          id: "blocked", name: "write_file", is_error: true,
          content: [{ type: "text", text: JSON.stringify({ error: "confinement_refused",
            message: "permission denied private-canary", data: { refusal } }) }],
        } };
        const local = reverse ? [result, launch] : [launch, result];
        const frames = [{ id: "start", event: "run_started", runId: "run", data: {} }, ...local,
          { id: "sibling-start", event: "tool_execution_started", runId: "run", data: { id: "permitted", name: "read_file" } },
          { id: "sibling", event: "tool_execution_completed", runId: "run", data: {
            id: "permitted", name: "read_file", is_error: false,
            content: [{ type: "text", text: "Sibling completed" }],
          } }];
        const feedback = map(null, frames).flatMap(entry => entry.kind === "message" && entry.operationFeedback ? [entry.operationFeedback] : []);
        assert.equal(feedback.length, 1);
        assert.equal(feedback[0].kind, "confinement-refused");
        assert.equal(feedback[0].confinementRefusal, refusal);
        assert.equal(feedback[0].toolCallId, "blocked");
        for (const id of ["  exact-call  ", "call-".repeat(80), " \t  "]) {
          const exact = map(null, [{ ...result, data: { ...result.data, id } }]).find(entry => entry.kind === "message" && entry.operationFeedback);
          assert.equal(exact?.kind === "message" ? exact.operationFeedback?.toolCallId : undefined, id);
        }
        for (const event of ["tool_result_received", "tool_execution_completed"]) {
          const canonical = map(null, [{ ...result, event }]).find(entry => entry.kind === "message" && entry.operationFeedback);
          assert.equal(canonical?.kind, "message");
          if (canonical?.kind !== "message") throw new Error("missing canonical refusal feedback");
          assert.equal(canonical.operationFeedback?.confinementRefusal, refusal);
          assert.equal(canonical.operationFeedback?.toolCallId, "blocked");
        }
        assert.match(feedback[0].detail, /did not run/);
        assert.doesNotMatch(JSON.stringify(feedback), /Permission denied|private-canary/);
        assert.equal(phase(frames, null), "waiting");
        const continued = [...frames, { id: "answer", event: "text_delta", runId: "run", data: { delta: "Continuing after the permitted sibling." } }];
        assert.equal(phase(continued, null), "generating");
        assert.match(JSON.stringify(map(null, continued)), /Sibling completed/);
        assert.match(JSON.stringify(map(null, continued)), /Continuing after the permitted sibling/);
        assert.equal(phase([...continued, { id: "done", event: "run_completed", runId: "run", data: {} }], null), null);
      }
    }
  });

  test(`${name} explicit hook denial keeps its owner and reason distinct from permission feedback`, () => {
    for (const reason_code of ["policy_violation", "safety_violation", "schema_violation", "timeout", "runtime_error"]) {
      const frames = [{ id: "start", event: "run_started", data: {} }, {
        id: "denied", event: "tool_execution_completed", data: { id: "blocked", is_error: true,
          content: [{ type: "text", text: JSON.stringify({ error: "hook_denied", message: "private-denial-canary",
            data: { hook_id: "guard", point: "pre_tool_execution", reason_code, payload: { secret: "private-payload-canary" } } }) }],
        },
      }];
      const feedback = map(null, frames).find(entry => entry.kind === "message" && entry.operationFeedback);
      assert.equal(feedback?.kind, "message");
      if (feedback?.kind !== "message") throw new Error("missing hook feedback");
      assert.equal(feedback.operationFeedback?.kind, "hook-denied");
      assert.equal(feedback.operationFeedback?.hookId, "guard");
      assert.equal(feedback.operationFeedback?.hookReasonCode, reason_code);
      assert.equal(feedback.operationFeedback?.toolCallId, "blocked");
      assert.doesNotMatch(JSON.stringify(feedback.operationFeedback), /Permission denied|private-.*-canary/);
      assert.equal(phase(frames, null), "waiting");
    }
  });

  test(`${name} actual hook launch IO remains infrastructure feedback without an invented confinement cause`, () => {
    const frames = [{ id: "start", event: "run_started", data: {} }, {
      id: "launch-io", event: "hook_launch_refused", data: { hook_id: "guard", point: "pre_tool_execution", tool_use_id: "blocked",
        reason: { reason_code: "execution_failed", message: "permission denied private-io-canary" },
      },
    }];
    const entry = map(null, frames).find(entry => entry.kind === "message" && entry.operationFeedback);
    assert.equal(entry?.kind, "message");
    if (entry?.kind !== "message") throw new Error("missing launch feedback");
    assert.equal(entry.operationFeedback?.kind, "hook-launch-failed");
    assert.equal(entry.operationFeedback?.toolCallId, "blocked");
    assert.equal(entry.operationFeedback?.confinementRefusal, undefined);
    assert.doesNotMatch(JSON.stringify(entry.operationFeedback), /Permission denied|private-io-canary/);
    assert.equal(phase(frames, null), "waiting");
  });

  test(`${name} malformed, unrelated and entered failures do not fabricate local refusal facts`, () => {
    const invalidPayloads = [
      { error: "confinement_refused", data: { refusal: "future_cause" } },
      { error: "confinement_refused", data: { refusal: null } },
      { error: "hook_denied", data: { hook_id: "guard", point: "pre_tool_execution", reason_code: "future_reason" } },
      { error: "hook_denied", data: { hook_id: "guard", point: "run_completed", reason_code: "runtime_error" } },
      { error: "execution_failed", message: "confinement_refused operation_refused permission denied" },
      { error: "operation_authorization_unavailable" },
      { error: "operation_observation_unavailable" },
    ];
    for (const payload of invalidPayloads) {
      const entries = map(null, [{ id: "bad", event: "tool_result_received", data: { id: "call", is_error: true,
        content: [{ type: "text", text: JSON.stringify(payload) }],
      } }]);
      assert.equal(entries.some(entry => entry.kind === "message" && entry.operationFeedback), false);
    }
    for (const event of ["hook_failed", "run_failed"]) {
      const frame = { id: event, event, data: { hook_id: "guard", point: "pre_tool_execution", tool_use_id: "blocked",
        reason: { reason_code: "confinement_refused", refusal: "invalid_launch" },
      } };
      assert.equal(map(null, [frame]).some(entry => entry.kind === "message" && entry.operationFeedback), false);
    }
    const missingCall = { id: "unbound", event: "hook_launch_refused", data: {
      hook_id: "guard", point: "pre_tool_execution", reason: { reason_code: "execution_failed", message: "failed" },
    } };
    assert.equal(map(null, [missingCall]).some(entry => entry.kind === "message" && entry.operationFeedback), false);
  });
}

function typedCommsNotice(args: {
  peer: string;
  body: string;
  peerId?: string;
  peerDisplayName?: string;
  kind?: string;
  direction?: "incoming" | "outgoing";
  intent?: string;
  requestId?: string;
  payload?: unknown;
}) {
  const kind = args.kind || "message";
  return {
    role: "system_notice",
    kind: "comms",
    body: args.body,
    blocks: [{
      type: "comms",
      kind,
      direction: args.direction || "incoming",
      peer: {
        id: args.peerId || args.peer,
        display_name: args.peerDisplayName || args.peer,
      },
      request_id: args.requestId || `req-${args.peer}-${kind}`,
      ...(args.intent ? { intent: args.intent } : {}),
      ...(args.payload !== undefined ? { payload: args.payload } : {}),
      content: [{ type: "text", text: args.body }],
    }],
  };
}

test("buildSidebarViewState preserves host-derived watch and degraded fields", () => {
  const viewState = buildSidebarViewState({
    selectedMemberId: "member-1",
    pinnedAgentIds: new Set(["member-1"]),
    agents: [
      {
        agent_id: "identity:luka",
        member_id: "member-1",
        label: "Example",
        kind: "operator",
        role: "console",
        state: "running",
        watched: true,
        alertLevel: "elevated",
        degraded: true,
        degradedReason: "lease_expired",
      },
    ],
  });

  const item = viewState.blocks[1]?.sections?.[0]?.items?.[0];
  assert.equal(item?.id, "member-1");
  assert.equal(item?.pinned, true);
  assert.equal(item?.selected, true);
  assert.equal(item?.watched, true);
  assert.equal(item?.alertLevel, "elevated");
  assert.equal(item?.degraded, true);
  assert.equal(item?.degradedReason, "lease_expired");
  assert.equal(item?.meta?.[0]?.tone, "accent");
});

test("buildSidebarViewState surfaces an unhealthy liveness chip and stays silent when healthy", () => {
  const agentBase = {
    agent_id: "identity:net",
    member_id: "member-net",
    label: "Network",
    kind: "domain",
    state: "active",
  };
  const wedged = buildSidebarViewState({
    selectedMemberId: "",
    agents: [{
      ...agentBase,
      progress: {
        run_state: "run_open",
        in_flight_work: 3,
        last_progress_at_ms: 1752300000000,
        last_progress_event: "unchanged",
        health: "wedged",
      },
    }],
  });
  const wedgedItem = wedged.blocks[1]?.sections?.[0]?.items?.[0];
  const healthChip = wedgedItem?.meta?.find((chip) => chip.id === "health");
  assert.equal(healthChip?.label, "wedged");
  assert.equal(healthChip?.tone, "negative");

  const degraded = buildSidebarViewState({
    selectedMemberId: "",
    agents: [{
      ...agentBase,
      progress: {
        run_state: "idle",
        in_flight_work: 0,
        last_progress_at_ms: 0,
        last_progress_event: "became_idle",
        health: "degraded",
      },
    }],
  });
  const degradedChip = degraded.blocks[1]?.sections?.[0]?.items?.[0]?.meta?.find((chip) => chip.id === "health");
  assert.equal(degradedChip?.label, "degraded");
  assert.equal(degradedChip?.tone, "warning");

  const healthy = buildSidebarViewState({
    selectedMemberId: "",
    agents: [{
      ...agentBase,
      progress: {
        run_state: "idle",
        in_flight_work: 0,
        last_progress_at_ms: 0,
        last_progress_event: "became_idle",
        health: "healthy",
      },
    }],
  });
  const healthyItem = healthy.blocks[1]?.sections?.[0]?.items?.[0];
  assert.equal(healthyItem?.meta?.some((chip) => chip.id === "health"), false);
});

test("buildRoutingSectionView projects runtime routing and delivery results without host invention", () => {
  const view = buildRoutingSectionView({
    routesResponse: {
      routes: [
        {
          route_key: "vip-route",
          recipient: "vip@example.com",
          channel: "notification",
          sink: "sms",
          target_module: "delivery",
          retry_max: 0,
          backoff_ms: 5,
          rate_limit_per_minute: 9,
        },
      ],
    },
    historyResponse: {
      deliveries: [
        {
          delivery_id: "delivery-1",
          route_id: "route-000001",
          recipient: "vip@example.com",
          sink: "sms",
          target_module: "delivery",
          status: "sent",
          first_attempt_ms: 100,
          final_attempt_ms: 200,
          idempotency_key: "delivery-key-1",
          sink_adapter: "sms-mock",
          attempts: [
            { attempt: 1, status: "sent", backoff_ms: 0 },
          ],
        },
      ],
    },
  });

  assert.deepEqual(view, {
    routes: [
      {
        route_key: "vip-route",
        recipient: "vip@example.com",
        channel: "notification",
        sink: "sms",
        target_module: "delivery",
        retry_max: 0,
        backoff_ms: 5,
        rate_limit_per_minute: 9,
      },
    ],
    deliveries: [
      {
        delivery_id: "delivery-1",
        route_id: "route-000001",
        recipient: "vip@example.com",
        sink: "sms",
        target_module: "delivery",
        status: "sent",
        first_attempt_ms: 100,
        final_attempt_ms: 200,
        idempotency_key: "delivery-key-1",
        sink_adapter: "sms-mock",
        attempts: [
          { attempt: 1, status: "sent", backoff_ms: 0 },
        ],
      },
    ],
  });
});

test("mapFramesToTimelineEntries renders a partial assistant message while deltas are still streaming", () => {
  // Regression: the live-overlay path used to pass `renderTextDeltas: false`,
  // which dropped text_delta frames on the floor so the conversation pane
  // stayed visually empty until `interaction_complete` arrived (user-visible
  // as "waiting… persists while the agent is actually typing").
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      { id: "evt-1", event: "interaction_started", data: {} },
      { id: "evt-2", event: "text_delta", data: { delta: "Status " } },
      { id: "evt-3", event: "text_delta", data: { delta: "is " } },
      { id: "evt-4", event: "text_delta", data: { delta: "stable." } },
    ],
  );

  // One partial message entry with all three deltas concatenated.
  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.kind, "message");
  const entry = entries[0]!;
  const text = "text" in entry
    ? entry.text
    : "blocks" in entry && Array.isArray(entry.blocks) && (entry.blocks[0]?.type === "paragraph" || entry.blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entry.blocks[0]) : "";
  assert.equal(text, "Status is stable.");
});

test("mapFramesToTimelineEntries stamps the turn interactionId on streamed text entries (audit fix)", () => {
  // Reversion detector for the finalization-remount fix: a streamed text
  // entry without interactionId keys its group by entry-id fallback, then
  // flips to the interaction anchor when the terminal entry joins —
  // remounting the live response group. Stamping from the first frame keeps
  // the group anchored throughout.
  const iid = "11111111-2222-4333-8444-555555555555";
  const streaming = mapFramesToTimelineEntries(
    { agent_id: "a", member_id: "a", label: "A", kind: "identity" },
    [
      { id: "evt-1", event: "interaction_started", interactionId: iid, data: {} },
      { id: "evt-2", event: "text_delta", interactionId: iid, data: { delta: "Working on it" } },
    ],
  );
  assert.equal(streaming.length, 1);
  assert.equal(
    streaming[0]?.kind === "message" ? streaming[0].interactionId : undefined,
    iid,
    "streamed text entry must carry the turn interactionId from the first frame",
  );

  const streamingGroups = groupConversationTimelineEntries(streaming);
  const finalized = mapFramesToTimelineEntries(
    { agent_id: "a", member_id: "a", label: "A", kind: "identity" },
    [
      { id: "evt-1", event: "interaction_started", interactionId: iid, data: {} },
      { id: "evt-2", event: "text_delta", interactionId: iid, data: { delta: "Working on it" } },
      { id: "evt-3", event: "interaction_complete", interactionId: iid, data: { text: "Working on it — done." } },
    ],
  );
  const finalizedGroups = groupConversationTimelineEntries(finalized);
  assert.equal(
    finalizedGroups[0]?.id,
    streamingGroups[0]?.id,
    "the response group id must not flip when the terminal entry joins",
  );
});

test("streamed reasoning entries carry the turn interactionId (audit fix, both adapters)", () => {
  // Reversion detectors for the three stamp sites the text-delta test does
  // not reach: the bundled REASONING stamp, and the SHARED package adapter's
  // text + reasoning stamps (embedders consume the shared mapper — a silent
  // revert there ships the finalization-remount bug to every embedder while
  // the bundled console stays green).
  const iid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
  const agent = { agent_id: "a", member_id: "a", label: "A", kind: "identity" };
  const frames = [
    { id: "evt-1", event: "interaction_started", interactionId: iid, data: {} },
    { id: "evt-2", event: "reasoning_delta", interactionId: iid, data: { delta: "thinking hard" } },
    { id: "evt-3", event: "text_delta", interactionId: iid, data: { delta: "answer" } },
  ];
  for (const [name, mapper] of [
    ["bundled", mapFramesToTimelineEntries],
    ["shared", mapFramesToTimelineEntriesShared],
  ] as const) {
    const entries = mapper(agent, frames as never);
    const reasoning = entries.find(
      (entry) => entry.kind === "message" && "blocks" in entry
        && entry.blocks?.some((block) => block.type === "thinking"),
    );
    assert.ok(reasoning, `${name}: reasoning entry rendered`);
    assert.equal(
      reasoning?.kind === "message" ? reasoning.interactionId : undefined,
      iid,
      `${name}: streamed reasoning entry must carry the turn interactionId`,
    );
    const text = entries.find(
      (entry) => entry.kind === "message"
        && (("text" in entry && entry.text) || ("blocks" in entry
          && entry.blocks?.some((block) => (block.type === "paragraph" || block.type === "markdown")))),
    );
    assert.ok(text, `${name}: text entry rendered`);
    assert.equal(
      text?.kind === "message" ? text.interactionId : undefined,
      iid,
      `${name}: streamed text entry must carry the turn interactionId`,
    );
  }
});

test("mapFramesToTimelineEntries keeps timestamp-less reasoning before the answer text", () => {
  // Regression: reasoning frames frequently arrive without a timestamp. The
  // transcript sort used to send timestamp-less frames to MAX_SAFE_INTEGER, so
  // reasoning sorted AFTER the answer text and rendered "thinking" at the end of
  // the turn. Here the reasoning frame has no timestampMs and is listed after the
  // text frames; it must still render before the answer.
  const entries = mapFramesToTimelineEntries(
    { agent_id: "a", member_id: "a", label: "A", kind: "identity" },
    [
      { id: "evt-1", event: "interaction_started", interactionId: "int-1", timestampMs: 1000, data: {} },
      { id: "evt-3", event: "text_delta", interactionId: "int-1", timestampMs: 1200, data: { delta: "The answer." } },
      { id: "evt-4", event: "text_complete", interactionId: "int-1", timestampMs: 1200, data: { content: "The answer." } },
      { id: "evt-2", event: "reasoning_delta", interactionId: "int-1", data: { delta: "Thinking first." } },
      { id: "evt-5", event: "interaction_complete", interactionId: "int-1", timestampMs: 1300, data: {} },
    ],
    { renderInteractionStartsAsUser: true },
  );
  const thinkingIdx = entries.findIndex(
    (e) => "blocks" in e && Array.isArray(e.blocks) && e.blocks.some((b) => b.type === "thinking"),
  );
  const answerIdx = entries.findIndex((e) => {
    const t = "text" in e
      ? e.text
      : "blocks" in e && Array.isArray(e.blocks) && (e.blocks[0]?.type === "paragraph" || e.blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(e.blocks[0]) : "";
    return typeof t === "string" && t.includes("The answer.");
  });
  assert.ok(thinkingIdx >= 0, "thinking entry present");
  assert.ok(answerIdx >= 0, "answer entry present");
  assert.ok(thinkingIdx < answerIdx, `thinking (${thinkingIdx}) should precede answer (${answerIdx})`);
});

test("mapFramesToTimelineEntries preserves incomplete streamed Markdown as one raw document", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "risk-red-team",
      member_id: "risk-red-team",
      label: "Risk Red Team",
      kind: "identity",
    },
    [
      { id: "evt-1", event: "text_delta", data: { delta: "## Risk\n\n" } },
      { id: "evt-2", event: "text_delta", data: { delta: "- first\n- seco" } },
    ],
  );

  assert.equal(entries.length, 1);
  const blocks = "blocks" in entries[0]! ? entries[0].blocks : [];
  assert.deepEqual(blocks, [{ type: "markdown", id: "evt-1:text:0", source: "## Risk\n\n- first\n- seco", streaming: true }]);
});

test("mapFramesToTimelineEntries keeps the same Markdown document on stream completion", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "risk-red-team",
      member_id: "risk-red-team",
      label: "Risk Red Team",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "text_delta",
        interactionId: "turn-1",
        data: { delta: "## Risk\n\n- first\n- second" },
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        interactionId: "turn-1",
        data: {},
      },
    ],
  );

  const assistant = entries.find((entry) => entry.kind === "message" && entry.identity.role === "assistant");
  const blocks = assistant && "blocks" in assistant ? assistant.blocks : [];
  assert.deepEqual(blocks, [{ type: "markdown", id: "evt-1:text:0", source: "## Risk\n\n- first\n- second", streaming: false }]);
});

test("mapFramesToTimelineEntries renders run_started parent prompts as the inbound turn", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "person-worker-alpha",
      member_id: "person-worker-alpha",
      label: "Person Worker",
      kind: "identity",
    },
    [
      {
        id: "tool-first-in-cursor-order",
        event: "tool_call_requested",
        cursor: "console:10",
        timestampMs: 200,
        data: { id: "call-1", name: "index_search", arguments: "{}" },
      },
      {
        id: "run-started-backfilled-late",
        event: "run_started",
        cursor: "console:11",
        timestampMs: 100,
        data: {
          prompt: "Peer message from parent: audit this person",
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.kind, "message");
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal("text" in entries[0]! ? entries[0].text : "", "Peer message from parent: audit this person");
  assert.equal(entries[1]?.kind, "message");
  assert.equal(entries[1]?.identity.role, "assistant");
  assert.equal(
    entries[1] && "blocks" in entries[1] && Array.isArray(entries[1].blocks)
      ? entries[1].blocks[0]?.type
      : "",
    "tool-call",
  );
});

test("mapFramesToTimelineEntries preserves repeated identical run_started parent prompts", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "person-worker-alpha",
      member_id: "person-worker-alpha",
      label: "Person Worker",
      kind: "identity",
    },
    [
      {
        id: "run-started-1",
        event: "run_started",
        timestampMs: Date.parse("2026-05-23T20:29:50.000Z"),
        data: { prompt: "continue" },
      },
      {
        id: "run-started-2",
        event: "run_started",
        timestampMs: Date.parse("2026-05-23T20:30:50.000Z"),
        data: { prompt: "continue" },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.deepEqual(
    entries.map((entry) => ("text" in entry ? entry.text : "")),
    ["continue", "continue"],
  );
});

test("mapFramesToTimelineEntries suppresses duplicate terminal text after streamed deltas", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "text_delta",
        data: { delta: "Hello! How can I assist you today?" },
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        data: { text: "Hello! How can I assist you today?" },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.kind, "message");
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : "",
    "Hello! How can I assist you today?",
  );
});

test("mapFramesToTimelineEntries suppresses session-history terminal text already delivered live", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-live-delta",
        event: "text_delta",
        timestampMs: 10,
        data: { delta: "Acknowledged. Standing by." },
      },
      {
        id: "evt-live-complete",
        event: "interaction_complete",
        timestampMs: 20,
        data: { result: "Acknowledged. Standing by." },
      },
      {
        id: "history-user",
        event: "user_input",
        sourceKind: "session_history",
        timestampMs: 30,
        data: { content: "You have been spawned as incident-commander." },
      },
      {
        id: "history-assistant",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: 31,
        data: {
          text: "Acknowledged. Standing by.",
          result: "Acknowledged. Standing by.",
          source_event_type: "session_history",
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const assistantTexts = entries
    .filter((entry) => entry.kind === "message" && entry.identity.role === "assistant")
    .map((entry) => {
      if ("text" in entry) return entry.text;
      if ("blocks" in entry && Array.isArray(entry.blocks)) {
        return entry.blocks
          .map((block) => (block.type === "paragraph" || block.type === "markdown") ? conversationRichBlockCopyText(block) : "")
          .join("");
      }
      return "";
    });

  assert.deepEqual(assistantTexts, ["Acknowledged. Standing by."]);
});

test("mapFramesToTimelineEntries ignores text_complete so the terminal event does not duplicate the same answer", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "text_delta",
        data: { delta: "Status is stable." },
      },
      {
        id: "evt-2",
        event: "text_complete",
        data: { content: "Status is stable." },
      },
      {
        id: "evt-3",
        event: "interaction_complete",
        data: { text: "Status is stable." },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : "",
    "Status is stable.",
  );
});

test("mapFramesToTimelineEntries ignores live text_complete before matching interaction completion", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-text-complete",
        event: "text_complete",
        interactionId: "turn-1",
        data: { content: "I’m online as incident-commander." },
      },
      {
        id: "evt-run-complete",
        event: "interaction_complete",
        interactionId: "turn-1",
        data: { result: "I’m online as incident-commander." },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : entries[0] && "text" in entries[0]
        ? entries[0].text
        : "",
    "I’m online as incident-commander.",
  );
});

test("mapFramesToTimelineEntries ignores hidden turn markers before terminal completion", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      { id: "evt-1", event: "text_delta", data: { delta: "Status is stable." } },
      { id: "evt-2", event: "text_complete", data: { content: "Status is stable." } },
      { id: "evt-3", event: "turn_completed", data: { stop_reason: "end_turn" } },
      { id: "evt-4", event: "interaction_complete", data: { text: "Status is stable." } },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : "",
    "Status is stable.",
  );
});

test("inferResponsePhaseFromFrames clears working state on terminal text and terminal turn-completed frames", () => {
  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "user_input", data: { content: "Hello" } },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "user_input", status: "completed", data: { content: "Hello" } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-2", event: "text_complete", data: { content: "Done." } },
      { id: "evt-3", event: "interaction_complete", data: { result: "Done." } },
      { id: "evt-4", event: "user_input", status: "completed", data: { content: "Hello" } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-2", event: "text_complete", data: { content: "Done." } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "run_started", data: { prompt: "Hello" } },
      { id: "evt-2", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-3", event: "text_complete", data: { content: "Done." } },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "run_started", data: { prompt: "Hello" } },
      { id: "evt-2", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-3", event: "text_complete", data: { content: "Done." } },
      { id: "evt-4", event: "run_completed", data: { result: "Done." } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "interaction_started", data: {} },
      { id: "evt-2", event: "run_started", data: { prompt: "Step one" } },
      { id: "evt-3", event: "text_complete", data: { content: "Step one done." } },
      { id: "evt-4", event: "run_completed", data: { result: "Step one done." } },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "interaction_started", data: {} },
      { id: "evt-2", event: "run_started", data: { prompt: "Step one" } },
      { id: "evt-3", event: "text_complete", data: { content: "Step one done." } },
      { id: "evt-4", event: "run_completed", data: { result: "Step one done." } },
      { id: "evt-5", event: "interaction_complete", data: { result: "All done." } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-2", event: "turn_completed", data: { stop_reason: "end_turn" } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "interaction_started", data: {} },
      { id: "evt-2", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-3", event: "turn_completed", data: { stop_reason: "end_turn" } },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "tool_execution_started", data: {} },
      { id: "evt-2", event: "turn_completed", data: { stop_reason: "tool_use" } },
    ]),
    "tool-executing",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "text_delta", data: { delta: "Partial." } },
      { id: "evt-2", event: "turn_completed", data: { stop_reason: "max_tokens" } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "text_delta", data: { delta: "Stopped." } },
      { id: "evt-2", event: "turn_completed", data: { stop_reason: "stop_sequence" } },
    ]),
    null,
  );

  // Tool-completion events should not claim "tool-executing" — at that
  // instant no tool is running — but they must still keep the turn visibly
  // active until terminal text/run evidence arrives. Spawned workers can
  // lack run_started/interaction_started projections; clearing here lets
  // operator sends bypass the local queue while the worker is still active.
  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "text_delta", data: { delta: "Done." } },
      { id: "evt-2", event: "text_complete", data: { content: "Done." } },
      { id: "evt-3", event: "tool_call_requested", data: { name: "save_investigation_result" } },
      { id: "evt-4", event: "tool_execution_started", data: {} },
      { id: "evt-5", event: "tool_execution_completed", data: {} },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "tool_call_requested", data: { name: "save_investigation_result" } },
      { id: "evt-2", event: "tool_result_received", data: {} },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "tool_execution_completed", data: {} },
      { id: "evt-2", event: "turn_completed", data: { stop_reason: "end_turn" } },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "tool_execution_completed", data: {} },
      {
        id: "evt-2",
        event: "system_notice",
        data: {
          blocks: [{
            content: [{ type: "text", text: "Peer message from worker:\nDone." }],
          }],
        },
      },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "tool_execution_completed", data: {} },
      { id: "evt-2", event: "reasoning_delta", data: { delta: "Planning next step" } },
    ]),
    "generating",
  );

  // A new tool call after a completed one should re-arm the indicator.
  assert.equal(
    inferResponsePhaseFromFrames([
      { id: "evt-1", event: "tool_execution_completed", data: {} },
      { id: "evt-2", event: "tool_call_requested", data: { name: "next_tool" } },
    ]),
    "tool-executing",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      {
        id: "evt-1",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            name: "web_search",
            type: "response.web_search_call.searching",
          },
        },
      },
    ]),
    "tool-executing",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      {
        id: "evt-1",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            name: "web_search",
            type: "response.web_search_call.in_progress",
          },
        },
      },
      {
        id: "evt-2",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            name: "web_search",
            type: "web_search_call",
            status: "completed",
          },
        },
      },
    ]),
    "waiting",
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      {
        id: "evt-1",
        event: "server_tool_content",
        data: {
          content: {
            id: "msg-1",
            name: "web_search_annotations",
            type: "message_annotations",
            annotations: [{ title: "Example", url: "https://example.com" }],
          },
        },
      },
    ]),
    null,
  );

  assert.equal(
    inferResponsePhaseFromFrames([
      {
        id: "evt-1",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            name: "web_search",
            type: "response.web_search_call.in_progress",
          },
        },
      },
      { id: "evt-2", event: "interaction_complete", data: { result: "Done." } },
    ]),
    null,
  );
});

for (const [surface, inferPhase, resolvePhase] of [
  ["stock", inferResponsePhaseFromFrames, resolvePanelResponsePhase],
  ["shared", inferResponsePhaseFromFramesShared, resolvePanelResponsePhaseShared],
] as const) {
  for (const event of ["text_complete", "interaction_complete"]) {
    const historyStep = {
      id: "saved-intermediate", event, sourceKind: "session_history",
      runtimeKey: "runtime-a", identity: "worker", sessionId: "session-a",
      runId: "00000000-0000-0000-0000-000000000001",
      interactionId: "00000000-0000-0000-0000-000000000002",
      timestampMs: 2,
      data: { result: "I will inspect the file.", text: "I will inspect the file.", message: {
        role: "block_assistant", stop_reason: "tool_use", blocks: [
          { block_type: "text", data: { text: "I will inspect the file." } },
          { block_type: "tool_use", data: { id: "read-1", name: "read_file", args: { path: "README.md" } } },
        ],
      } },
    };
    test(`${surface}: saved intermediate ${event} cannot override working evidence without a retained start`, () => {
      assert.equal(inferPhase([historyStep], "tool-executing"), "tool-executing");
      assert.equal(resolvePhase({ frames: [historyStep], serverPhase: "generating" }), "generating");
      assert.equal(resolvePhase({ frames: [historyStep], serverPhase: null }), null,
        "a saved step alone does not manufacture current working state");
      const open = { ...historyStep, id: "actual-start", event: "run_started", sourceKind: "console_event", timestampMs: 1, data: {} };
      const liveText = { ...historyStep, id: "next-text", event: "text_complete", sourceKind: "console_event", timestampMs: 3, data: { text: "Next step." } };
      assert.equal(inferPhase([open, historyStep, liveText]), "waiting",
        "a legacy saved interaction_complete cannot close the real open run");
      assert.equal(resolvePhase({ frames: [open, historyStep, liveText], serverPhase: "generating" }), "waiting");
    });
    test(`${surface}: actual terminals remain authoritative around saved intermediate ${event} replay`, () => {
      for (const terminalEvent of ["run_completed", "interaction_complete"]) {
        const terminal = { ...historyStep, id: "actual-terminal", event: terminalEvent, sourceKind: "console_event", timestampMs: 3, data: { result: "Done." } };
        for (const frames of [[historyStep, terminal], [terminal, historyStep]]) {
          assert.equal(inferPhase(frames, "waiting"), null);
          assert.equal(resolvePhase({ frames, serverPhase: "tool-executing" }), null,
            "an intermediate replay cannot revive a completed run");
        }
      }
      assert.equal(resolvePhase({ frames: [{ ...historyStep, sourceKind: "console_event" }], serverPhase: "generating" }), null,
        "the saved-step exception does not reinterpret live lifecycle events");
    });
  }
}

for (const [event, inputStatus, terminalStatus, data] of [
  ["message_delivery_failed", "delivery_failed", "delivery_failed", { reason: "host-human input refused", data: { kind: "host_human_input_unsupported" } }],
  ["interaction_complete", "delivered", "completed", { reason: "steer_delivered", handling_mode: "steer" }],
] as const) {
  test(`keeps owner A phase after input B emits ${event}`, () => {
    const scope = { identity: "owner", runtimeKey: "owner-runtime", sessionId: "owner-session" };
    // The queried input status reflects the same B delivery terminal.
    const frames = [
      { ...scope, id: "interaction-A-start", event: "interaction_started", interactionId: "interaction-A", data: {} },
      { ...scope, id: "run-A-start", event: "run_started", interactionId: "interaction-A", runId: "run-A", data: {} },
      { ...scope, id: "input-B", event: "user_input", sourceKind: "send", interactionId: "interaction-B", status: inputStatus, data: { content: "Steer B", handling_mode: "steer" } },
      { ...scope, id: "input-B-terminal", event, sourceKind: "synthetic", interactionId: "interaction-B", status: terminalStatus, data },
    ];
    assert.equal(inferResponsePhaseFromFrames(frames), "waiting");
    assert.equal(resolvePanelResponsePhase({ frames, serverPhase: "generating" }), "waiting");
    const completed = [...frames, { ...scope, id: "owner-A-terminal", event: "interaction_complete",
      interactionId: "interaction-A", data: { text: "Owner A completed" } }];
    assert.equal(inferResponsePhaseFromFrames(completed), null);
    assert.equal(resolvePanelResponsePhase({ frames: completed, serverPhase: "generating" }), null);
  });
}

test("sparse owner linking cannot merge conflicting known sessions", () => {
  const interaction = { identity: "owner", runtimeKey: "runtime", sessionId: "session-A" };
  const run = { identity: "owner", runtimeKey: "runtime", sessionId: "session-B" };
  const frames = [
    { ...interaction, id: "interaction-start", event: "interaction_started", interactionId: "interaction-A", data: {} },
    { ...run, id: "run-start", event: "run_started", runId: "run-B", data: {} },
    { identity: "owner", id: "sparse-link", event: "run_started", interactionId: "interaction-A", runId: "run-B", data: {} },
    { ...interaction, id: "interaction-terminal", event: "interaction_complete", interactionId: "interaction-A", data: {} },
  ];
  assert.equal(inferResponsePhaseFromFrames(frames), "waiting");
  assert.equal(resolvePanelResponsePhase({ frames, serverPhase: "generating" }), "waiting");
  const completed = [...frames, { ...run, id: "run-terminal", event: "run_completed", runId: "run-B", data: {} }];
  assert.equal(inferResponsePhaseFromFrames(completed), null);
});

test("sparse owner linking preserves the run's known scope", () => {
  const scope = { identity: "owner", runtimeKey: "runtime", sessionId: "session-A" };
  const frames = [
    { id: "interaction-start", event: "interaction_started", interactionId: "interaction-A", data: {} },
    { ...scope, id: "run-start", event: "run_started", runId: "run-A", data: {} },
    { id: "sparse-link", event: "run_started", interactionId: "interaction-A", runId: "run-A", data: {} },
    { ...scope, sessionId: "session-B", id: "wrong-session-terminal", event: "interaction_complete", interactionId: "interaction-A", data: {} },
  ];
  assert.equal(inferResponsePhaseFromFrames(frames), "waiting");
  assert.equal(resolvePanelResponsePhase({ frames, serverPhase: "generating" }), "waiting");
  const completed = [...frames, { ...scope, id: "owner-terminal", event: "interaction_complete", interactionId: "interaction-A", data: {} }];
  assert.equal(inferResponsePhaseFromFrames(completed), null);
  assert.equal(resolvePanelResponsePhase({ frames: completed, serverPhase: "generating" }), null);
});

test("resolvePanelResponsePhase lets local terminal history clear stale server phase", () => {
  assert.equal(
    resolvePanelResponsePhase({
      frames: [],
      serverPhase: "generating",
    }),
    "generating",
  );

  assert.equal(
    resolvePanelResponsePhase({
      frames: [
        { id: "evt-1", event: "interaction_started", timestampMs: 1, data: { content: "Work" } },
        { id: "evt-2", event: "text_delta", timestampMs: 2, data: { delta: "Done." } },
        { id: "evt-3", event: "interaction_complete", timestampMs: 3, data: { result: "Done." } },
      ],
      serverPhase: "generating",
    }),
    null,
  );

  assert.equal(
    resolvePanelResponsePhase({
      frames: [
        { id: "evt-1", event: "run_started", timestampMs: 1, data: { prompt: "Work" } },
        { id: "evt-2", event: "tool_call_requested", timestampMs: 2, data: { name: "index_search" } },
        { id: "evt-3", event: "tool_execution_completed", timestampMs: 3, data: {} },
      ],
      serverPhase: "tool-executing",
    }),
    "waiting",
  );

  assert.equal(
    resolvePanelResponsePhase({
      frames: [
        { id: "evt-1", event: "interaction_started", timestampMs: 1, data: { content: "Work" } },
      ],
      localPhase: null,
      hasLocalPhase: true,
      serverPhase: "generating",
    }),
    null,
  );
});

test("mapFramesToTimelineEntries renders terminal completion without streamed deltas", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "api-investigator",
      member_id: "api-investigator",
      label: "API Investigator",
      kind: "identity",
    },
    [
      { id: "evt-1", event: "turn_completed", data: { stop_reason: "end_turn" } },
      {
        id: "evt-2",
        event: "interaction_complete",
        data: { result: "The uploaded badge says ALL CLEAR." },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.role, "assistant");
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : "",
    "The uploaded badge says ALL CLEAR.",
  );
});

test("mapFramesToTimelineEntries hides steer delivery terminal control frames", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "review",
      member_id: "review",
      label: "Review Agent",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "user_input",
        interactionId: "interaction-1",
        data: { text: "Steer this", handling_mode: "steer" },
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        interactionId: "interaction-1",
        data: { reason: "steer_delivered", handling_mode: "steer" },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.notEqual(entries[0]?.identity.role, "assistant");
  assert.match("text" in entries[0] ? entries[0].text : "", /Steer this/);
  assert.doesNotMatch("text" in entries[0] ? entries[0].text : "", /steer_delivered/);
});

test("mapFramesToTimelineEntries renders image-tool turns without duplicating final text", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "console-evt-2",
        event: "interaction_started",
        interactionId: "turn-1",
        timestampMs: 10,
        data: { content: "Generate an ALL CLEAR badge." },
      },
      {
        id: "evt-tool-call",
        event: "tool_call_requested",
        interactionId: "turn-1",
        timestampMs: 20,
        data: { id: "tool-1", name: "generate_image", args: { prompt: "ALL CLEAR" } },
      },
      {
        id: "evt-tool-done",
        event: "tool_execution_completed",
        interactionId: "turn-1",
        timestampMs: 30,
        data: {
          id: "tool-1",
          name: "generate_image",
          result: JSON.stringify({
            images: [
              {
                blob_ref: { blob_id: "sha256:badge", media_type: "image/png" },
                height: 1024,
                image_id: "image-1",
                media_type: "image/png",
                width: 1024,
              },
            ],
          }),
        },
      },
      {
        id: "evt-image",
        event: "assistant_image",
        interactionId: "turn-1",
        timestampMs: 30,
        data: {
          blob_id: "sha256:badge",
          media_type: "image/png",
          width: 1024,
          height: 1024,
        },
      },
      {
        id: "evt-tool-result",
        event: "tool_result_received",
        interactionId: "turn-1",
        timestampMs: 30,
        data: {
          id: "tool-1",
          name: "generate_image",
          result: JSON.stringify({
            images: [
              {
                blob_ref: { blob_id: "sha256:badge", media_type: "image/png" },
                height: 1024,
                image_id: "image-1",
                media_type: "image/png",
                width: 1024,
              },
            ],
          }),
        },
      },
      {
        id: "evt-turn-started-2",
        event: "turn_started",
        interactionId: "turn-1",
        timestampMs: 30,
        data: {},
      },
      {
        id: "evt-delta-1",
        event: "text_delta",
        interactionId: "turn-1",
        timestampMs: 40,
        data: { delta: "Generated" },
      },
      {
        id: "evt-delta-2",
        event: "text_delta",
        interactionId: "turn-1",
        timestampMs: 40,
        data: { delta: " the square ALL CLEAR incident badge image." },
      },
      {
        id: "evt-text-complete",
        event: "text_complete",
        interactionId: "turn-1",
        timestampMs: 50,
        data: { content: "Generated the square ALL CLEAR incident badge image." },
      },
      {
        id: "evt-turn-complete",
        event: "turn_completed",
        interactionId: "turn-1",
        timestampMs: 50,
        data: { stop_reason: "end_turn" },
      },
      {
        id: "evt-complete",
        event: "interaction_complete",
        interactionId: "turn-1",
        timestampMs: 50,
        data: { result: "Generated the square ALL CLEAR incident badge image." },
      },
    ],
    {
      renderInteractionStartsAsUser: true,
      blobBaseUrl: "http://127.0.0.1:49551",
    },
  );

  const finalTextEntries = entries.filter((entry) => {
    if (entry.kind !== "message") return false;
    if ("text" in entry && entry.text === "Generated the square ALL CLEAR incident badge image.") return true;
    return "blocks" in entry
      && Array.isArray(entry.blocks)
      && entry.blocks.some(
        (block) => (block.type === "paragraph" || block.type === "markdown")
          && conversationRichBlockCopyText(block) === "Generated the square ALL CLEAR incident badge image.",
      );
  });
  const imageEntries = entries.filter(
    (entry) => entry.kind === "message"
      && "blocks" in entry
      && Array.isArray(entry.blocks)
      && entry.blocks.some((block) => block.type === "image"),
  );

  assert.equal(finalTextEntries.length, 1);
  assert.equal(imageEntries.length, 1);
});

test("mapFramesToTimelineEntries renders generate_image result images immediately", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-tool-call",
        event: "tool_call_requested",
        interactionId: "turn-1",
        timestampMs: 20,
        data: { id: "tool-1", name: "generate_image", args: { prompt: "ALL CLEAR" } },
      },
      // The execution fact carries no body; the result event carries the
      // generated images.
      {
        id: "evt-tool-done",
        event: "tool_execution_completed",
        interactionId: "turn-1",
        timestampMs: 30,
        data: { id: "tool-1", name: "generate_image", is_error: false, duration_ms: 900 },
      },
      {
        id: "evt-tool-result",
        event: "tool_result_received",
        interactionId: "turn-1",
        timestampMs: 30,
        data: {
          id: "tool-1",
          name: "generate_image",
          is_error: false,
          result: JSON.stringify({
            images: [
              {
                blob_ref: { blob_id: "sha256:badge", media_type: "image/png" },
                height: 1024,
                image_id: "image-1",
                media_type: "image/png",
                width: 1024,
              },
            ],
          }),
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:49551",
    },
  );

  const imageEntries = entries.filter(
    (entry) => entry.kind === "message"
      && "blocks" in entry
      && Array.isArray(entry.blocks)
      && entry.blocks.some((block) => block.type === "image"),
  );

  assert.equal(imageEntries.length, 1);
  const imageBlock = imageEntries[0]?.kind === "message"
    && "blocks" in imageEntries[0]
    && Array.isArray(imageEntries[0].blocks)
    ? imageEntries[0].blocks.find((block) => block.type === "image")
    : null;
  assert.equal(imageBlock?.type, "image");
  assert.equal(
    imageBlock?.type === "image" ? imageBlock.src : "",
    "http://127.0.0.1:49551/blobs/sha256%3Abadge",
  );
});

test("sortConversationTimelineEntries keeps optimistic user messages after older assistant replies", () => {
  const entries = sortConversationTimelineEntries([
    {
      kind: "message",
      id: "assistant-1",
      identity: { id: "agent-1", label: "Agent", role: "assistant" },
      variant: "plain",
      text: "Assistant 1",
      createdAt: "2026-04-04T10:00:01.000Z",
    },
    {
      kind: "message",
      id: "user-1",
      identity: { id: "user", label: "You", role: "user" },
      variant: "plain",
      text: "User 1",
      createdAt: "2026-04-04T10:00:00.000Z",
    },
    {
      kind: "message",
      id: "user-2",
      identity: { id: "user", label: "You", role: "user" },
      variant: "plain",
      text: "User 2",
      createdAt: "2026-04-04T10:00:02.000Z",
    },
  ]);

  assert.deepEqual(entries.map((entry) => entry.id), ["user-1", "assistant-1", "user-2"]);
});

test("optimisticUserMessageForPanel shares the latest identity prompt across split panes", () => {
  const older = {
    interactionId: "",
    sentAtMs: 10,
    entry: {
      kind: "message" as const,
      id: "older",
      identity: { id: "user", label: "You", role: "user" as const },
      variant: "plain" as const,
      text: "Older prompt",
      createdAt: "2026-05-21T14:00:00.000Z",
    },
  };
  const latest = {
    interactionId: "",
    sentAtMs: 20,
    entry: {
      kind: "message" as const,
      id: "latest",
      identity: { id: "user", label: "You", role: "user" as const },
      variant: "plain" as const,
      text: "Latest prompt",
      createdAt: "2026-05-21T14:01:00.000Z",
    },
  };
  const direct = {
    interactionId: "",
    sentAtMs: 1,
    entry: {
      kind: "message" as const,
      id: "direct",
      identity: { id: "user", label: "You", role: "user" as const },
      variant: "plain" as const,
      text: "Direct prompt",
      createdAt: "2026-05-21T14:02:00.000Z",
    },
  };
  const optimistic = {
    "panel:left:agent-chat:deep-investigator:singleton": older,
    "panel:middle:agent-chat:other-agent": direct,
    "panel:right:agent-chat:deep-investigator:singleton": latest,
  };

  assert.equal(
    optimisticUserMessageForPanel(
      optimistic,
      "panel:new:agent-chat:deep-investigator:singleton",
      "deep-investigator:singleton",
    )?.entry.id,
    "latest",
  );
  assert.equal(
    optimisticUserMessageForPanel(
      optimistic,
      "panel:middle:agent-chat:other-agent",
      "other-agent",
    )?.entry.id,
    "direct",
  );
});

test("sortConversationTimelineEntries orders accepted user frames before later assistant replies regardless of entry arrival", () => {
  const entries = sortConversationTimelineEntries([
    {
      kind: "message",
      id: "assistant-1",
      identity: { id: "agent-1", label: "Agent", role: "assistant" },
      variant: "plain",
      text: "ORDER_OK",
      createdAt: "2026-05-07T15:21:50.000Z",
    },
    {
      kind: "message",
      id: "user-1",
      identity: { id: "user", label: "You", role: "user" },
      variant: "plain",
      text: "Ordering smoke: reply with exactly ORDER_OK and no extra text.",
      createdAt: "2026-05-07T15:21:48.000Z",
    },
  ]);

  assert.deepEqual(entries.map((entry) => entry.id), ["user-1", "assistant-1"]);
});

test("appendOptimisticConversationEntry preserves timestamp transcript order", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "deep-investigator",
      member_id: "deep-investigator",
      label: "Deep Investigator",
      kind: "identity",
    },
    [
      {
        id: "tool-1",
        cursor: "console:4",
        event: "tool_call_requested",
        timestampMs: 1_779_405_464_000,
        interactionId: "turn-1",
        data: { id: "call-1", name: "get_all_initiatives", args: {} },
      },
      {
        id: "tool-1-done",
        cursor: "console:5",
        event: "tool_execution_completed",
        timestampMs: 1_779_405_464_500,
        interactionId: "turn-1",
        data: { id: "call-1", name: "get_all_initiatives", result: "[]" },
      },
      {
        id: "final",
        cursor: "console:6",
        event: "interaction_complete",
        timestampMs: 1_779_405_463_000,
        interactionId: "turn-1",
        data: { result: "I checked the initiative list." },
      },
    ],
    { renderInteractionStartsAsUser: true, renderTextDeltas: true },
  );
  const optimistic = {
    kind: "message" as const,
    id: "optimistic",
    identity: { id: "user", label: "You", role: "user" as const },
    variant: "plain" as const,
    text: "Queued follow-up",
    createdAt: "2026-05-22T00:00:00.000Z",
  };

  assert.deepEqual(
    appendOptimisticConversationEntry(entries, optimistic).map((entry) => {
      if (entry.id === "optimistic") return "optimistic";
      if (entry.kind !== "message") return entry.kind;
      if (entry.variant === "rich" && entry.blocks?.[0]?.type === "tool-call") return "tool";
      if (entry.variant === "rich" && (entry.blocks?.[0]?.type === "paragraph" || entry.blocks?.[0]?.type === "markdown")) {
        return conversationRichBlockCopyText(entry.blocks[0]);
      }
      return "text" in entry ? entry.text : "rich";
    }),
    ["I checked the initiative list.", "tool", "optimistic"],
  );
});

test("mapFramesToTimelineEntries renders tool turns without raw tool lifecycle system noise", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "tool_call_requested",
        data: { id: "call-1", name: "send", args: { to: "payments-sre", body: "Check status" } },
      },
      {
        id: "evt-2",
        event: "tool_execution_started",
        data: { id: "call-1", name: "send" },
      },
      {
        id: "evt-3",
        event: "tool_execution_completed",
        data: { id: "call-1", name: "send", result: "{\"status\":\"sent\"}" },
      },
      {
        id: "evt-4",
        event: "text_delta",
        data: { delta: "Sent the status check." },
      },
      {
        id: "evt-5",
        event: "interaction_complete",
        data: { text: "Sent the status check." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.kind, "message");
  assert.equal(entries[0]?.identity.role, "assistant");
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? entries[0].blocks[0]?.type
      : "",
    "tool-call",
  );
  assert.equal(entries[1]?.identity.role, "assistant");
  assert.equal(
    entries[1] && "blocks" in entries[1] && Array.isArray(entries[1].blocks)
      ? (entries[1].blocks[0]?.type === "paragraph" || entries[1].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[1].blocks[0]) : ""
      : "",
    "Sent the status check.",
  );
});

test("mapFramesToTimelineEntries renders server tool content as tool activity without raw JSON", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-search-1",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            item_id: "ws-1",
            name: "web_search",
            type: "response.web_search_call.in_progress",
          },
        },
      },
      {
        id: "evt-search-2",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            item_id: "ws-1",
            name: "web_search",
            type: "response.web_search_call.completed",
          },
        },
      },
      {
        id: "evt-annotations",
        event: "server_tool_content",
        data: {
          content: {
            id: "msg-1",
            name: "web_search_annotations",
            type: "message_annotations",
            annotations: [
              { title: "Example", url: "https://example.com" },
            ],
          },
        },
      },
      {
        id: "evt-answer",
        event: "interaction_complete",
        data: { result: "I found one source." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const tool = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(tool?.type, "tool-call");
  if (tool?.type === "tool-call") {
    assert.equal(tool.name, "web_search");
    assert.equal(tool.status, "success");
    assert.match(tool.result || "", /Example/);
    assert.match(tool.result || "", /https:\/\/example\.com/);
  }
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "I found one source.");
});

test("mapFramesToTimelineEntries renders completed web_search_call status and query context", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-search-1",
        event: "server_tool_content",
        data: {
          content: {
            id: "ws-1",
            item_id: "ws-1",
            name: "web_search_call",
            type: "web_search_call",
            status: "completed",
            action: {
              queries: ["stockholm continuity risk", "muskö naval base"],
            },
          },
        },
      },
    ],
  );

  assert.equal(entries.length, 1);
  const tool = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(tool?.type, "tool-call");
  if (tool?.type === "tool-call") {
    assert.equal(tool.status, "success");
    assert.equal(tool.arguments, "stockholm continuity risk\nmuskö naval base");
  }
});

test("mapFramesToTimelineEntries hides orphan server tool annotations", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-annotations",
        event: "server_tool_content",
        data: {
          content: {
            id: "msg-1",
            name: "web_search_annotations",
            type: "message_annotations",
            annotations: [
              { title: "Example", url: "https://example.com" },
            ],
          },
        },
      },
    ],
  );

  assert.equal(entries.length, 0);
});

test("mapFramesToTimelineEntries preserves text and tool interleaving inside one interaction", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "deep-investigator",
      member_id: "deep-investigator",
      label: "Deep Investigator",
      kind: "identity",
    },
    [
      {
        id: "user",
        cursor: "console:1",
        event: "user_input",
        timestampMs: 100,
        interactionId: "turn-1",
        data: { content: "Investigate Minecraft" },
      },
      {
        id: "text-1",
        cursor: "console:2",
        event: "text_delta",
        timestampMs: 200,
        interactionId: "turn-1",
        data: { delta: "I will look it up." },
      },
      {
        id: "complete-1",
        cursor: "console:3",
        event: "text_complete",
        timestampMs: 300,
        interactionId: "turn-1",
        data: { content: "I will look it up." },
      },
      {
        id: "tool-1",
        cursor: "console:4",
        event: "tool_call_requested",
        timestampMs: 400,
        interactionId: "turn-1",
        data: { id: "call-1", name: "get_all_initiatives", args: {} },
      },
      {
        id: "tool-1-done",
        cursor: "console:5",
        event: "tool_execution_completed",
        timestampMs: 500,
        interactionId: "turn-1",
        data: { id: "call-1", name: "get_all_initiatives", result: "[]" },
      },
      {
        id: "text-2",
        cursor: "console:6",
        event: "text_delta",
        timestampMs: 600,
        interactionId: "turn-1",
        data: { delta: "Matched the initiative." },
      },
      {
        id: "done",
        cursor: "console:7",
        event: "interaction_complete",
        timestampMs: 700,
        interactionId: "turn-1",
        data: { result: "I will look it up.\nMatched the initiative." },
      },
    ],
    { renderInteractionStartsAsUser: true, renderTextDeltas: true },
  );

  assert.equal(entries.length, 4);
  assert.deepEqual(
    entries.map((entry) => {
      if (entry.kind !== "message") return entry.kind;
      if (entry.variant === "rich") {
        const block = entry.blocks?.[0];
        if (block?.type === "tool-call") return "tool";
        if (block?.type === "paragraph" || block?.type === "markdown") return conversationRichBlockCopyText(block);
        return "rich";
      }
      return entry.text;
    }),
    ["Investigate Minecraft", "I will look it up.", "tool", "Matched the initiative."],
  );
});

test("mapFramesToTimelineEntries hides technical peer request intents and previews human params", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-peers-done",
        event: "tool_execution_completed",
        data: {
          id: "peers-1",
          name: "peers",
          result: JSON.stringify({
            peers: [{ peer_id: "peer-scribe-1", name: "incident-command-center/scribe/scribe" }],
          }),
        },
      },
      {
        id: "evt-request",
        event: "tool_call_requested",
        data: {
          id: "call-1",
          name: "send_request",
          args: {
            peer_id: "peer-scribe-1",
            intent: "checksum_token",
            params: { subject: "Reply exactly: peer smoke ok" },
          },
        },
      },
      {
        id: "evt-request-done",
        event: "tool_execution_completed",
        data: { id: "call-1", name: "send_request", result: "{\"status\":\"sent\"}" },
      },
    ],
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(block?.type, "tool-call");
  if (block?.type === "tool-call") {
    assert.equal(block.peerTarget, "scribe");
    assert.equal(block.peerIntent, undefined);
    assert.equal(block.peerBody, "Reply exactly: peer smoke ok");
  }
});

test("mapFramesToTimelineEntries previews checksum peer responses by token instead of raw JSON", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "identity",
    },
    [
      {
        id: "evt-response",
        event: "tool_call_requested",
        data: {
          id: "call-1",
          name: "send_response",
          args: {
            peer_id: "peer-commander-1",
            display_name: "incident-command-center/commander/incident-commander",
            in_reply_to: "req-1",
            result: {
              request_intent: "checksum_token",
              request_subject: "peer smoke ok",
              token: "peer smoke ok",
            },
          },
        },
      },
      {
        id: "evt-response-done",
        event: "tool_execution_completed",
        data: { id: "call-1", name: "send_response", result: "{\"status\":\"sent\"}" },
      },
    ],
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(block?.type, "tool-call");
  if (block?.type === "tool-call") {
    assert.equal(block.peerTarget, "incident-commander");
    assert.equal(block.peerBody, "peer smoke ok");
  }
});

test("mapFramesToTimelineEntries can render historical interaction_started frames as user messages", () => {
  const entries = mapFramesToTimelineEntries(
    null,
    [
      {
        id: "evt-1",
        event: "interaction_started",
        data: { content: "Run a status sweep." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0] && "text" in entries[0] ? entries[0].text : "", "Run a status sweep.");
});

test("mapFramesToTimelineEntries renders aggregate user_input frames as user messages", () => {
  const entries = mapFramesToTimelineEntries(
    null,
    [
      {
        id: "console-frame-1",
        cursor: "console:1",
        event: "user_input",
        data: { content: "Run a status sweep." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0] && "text" in entries[0] ? entries[0].text : "", "Run a status sweep.");
});

test("mapFramesToTimelineEntries renders session-history assistant text_complete frames", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "full-tool-worker",
      member_id: "full-tool-worker",
      label: "full-tool-worker",
      kind: "mob_agent",
    },
    [
      {
        id: "history-assistant",
        event: "text_complete",
        identity: "full-tool-worker",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "text",
                data: { text: "Ready and standing by. Readiness reported." },
              },
            ],
          },
        },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "full-tool-worker");
  const renderedText = entries[0] && "text" in entries[0]
    ? entries[0].text
    : entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? entries[0].blocks.map((block) => (block.type === "paragraph" || block.type === "markdown") ? conversationRichBlockCopyText(block) : "").join("")
      : "";
  assert.equal(
    renderedText,
    "Ready and standing by. Readiness reported.",
  );
});

test("mapFramesToTimelineEntries renders session-history reasoning blocks outside answer text", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "history-reasoning-only",
        event: "interaction_complete",
        identity: "incident-worker-1",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "reasoning",
                data: { text: "**Deciding on communication approach**\n\nI should think this through." },
              },
              {
                block_type: "tool_use",
                data: { name: "peers", args: {}, id: "call-peers" },
              },
            ],
          },
          text: "**Deciding on communication approach**\n\nI should think this through.",
          result: "**Deciding on communication approach**\n\nI should think this through.",
        },
      },
      {
        id: "history-final",
        event: "interaction_complete",
        identity: "incident-worker-1",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "text",
                data: {
                  text: "Ready as the incident investigation worker and standing by for follow-up tasks.",
                },
              },
              {
                block_type: "reasoning",
                data: { text: "**Considering event response**\n\nI should not be rendered as an answer." },
              },
            ],
          },
          text: "**Considering event response**\n\nI should not be rendered as an answer.Ready as the incident investigation worker and standing by for follow-up tasks.",
          result: "**Considering event response**\n\nI should not be rendered as an answer.Ready as the incident investigation worker and standing by for follow-up tasks.",
        },
      },
    ],
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "incident-worker-1");
  const reasoningOnlyBlocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks : [];
  assert.equal(reasoningOnlyBlocks?.[0]?.type, "thinking");
  assert.equal(reasoningOnlyBlocks?.[0]?.text, "**Deciding on communication approach**\n\nI should think this through.");
  assert.equal(reasoningOnlyBlocks?.[0]?.label, "");
  assert.equal(reasoningOnlyBlocks?.[0]?.final, true);
  assert.equal(reasoningOnlyBlocks?.[0]?.persisted, true);
  const toolBlock = reasoningOnlyBlocks?.[1];
  assert.equal(toolBlock?.type, "tool-call");
  assert.equal(toolBlock?.name, "peers");
  assert.equal(entries[1]?.identity.id, "incident-worker-1");
  const finalBlocks = entries[1] && "blocks" in entries[1] ? entries[1].blocks : [];
  assert.equal(finalBlocks?.[0]?.type, "thinking");
  assert.equal(finalBlocks?.[0]?.text, "**Considering event response**\n\nI should not be rendered as an answer.");
  assert.equal(finalBlocks?.[0]?.label, "");
  assert.equal(finalBlocks?.[1]?.type, "markdown");
  assert.equal(finalBlocks?.[1]?.source, "Ready as the incident investigation worker and standing by for follow-up tasks.");
  const renderedText = entries[1] && "text" in entries[1]
    ? entries[1].text
    : entries[1] && "blocks" in entries[1] && Array.isArray(entries[1].blocks)
      ? entries[1].blocks.map((block) => (block.type === "paragraph" || block.type === "markdown") ? conversationRichBlockCopyText(block) : "").join("")
      : "";
  assert.equal(
    renderedText,
    "Ready as the incident investigation worker and standing by for follow-up tasks.",
  );
});

test("mapFramesToTimelineEntries renders live reasoning deltas as thought blocks", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        identity: "incident-worker-1",
        data: { delta: "Planning " },
      },
      {
        id: "reasoning-2",
        event: "reasoning_delta",
        identity: "incident-worker-1",
        data: { delta: "the next move." },
      },
      {
        id: "text-1",
        event: "text_delta",
        identity: "incident-worker-1",
        data: { delta: "Ready." },
      },
      {
        id: "done-1",
        event: "interaction_complete",
        identity: "incident-worker-1",
        data: { result: "Ready." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const thought = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(thought?.type, "thinking");
  assert.equal(thought?.text, "Planning the next move.");
  assert.equal(thought?.label, "");
  assert.equal(thought?.final, true);
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "Ready.");
});

test("mapFramesToTimelineEntries suppresses history reasoning replay after a streamed answer", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_complete",
        identity: "incident-worker-1",
        runId: "run-history-replay",
        data: { content: "Planning before answering." },
      },
      {
        id: "text-1",
        event: "text_delta",
        identity: "incident-worker-1",
        runId: "run-history-replay",
        data: { delta: "Ready." },
      },
      {
        id: "history-final",
        event: "interaction_complete",
        identity: "incident-worker-1",
        runId: "run-history-replay",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "text",
                data: { text: "Ready." },
              },
              {
                block_type: "reasoning",
                data: { text: "Planning before answering." },
              },
            ],
          },
          text: "Ready.",
          result: "Ready.",
        },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const thought = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(thought?.type, "thinking");
  assert.equal(thought?.text, "Planning before answering.");
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "Ready.");
});

test("mapFramesToTimelineEntries replaces unfinished reasoning with the late authoritative block", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { delta: "Searching for context.\n\nEvaluating choices." },
      },
      {
        id: "text-1",
        event: "text_delta",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { delta: "Ready." },
      },
      {
        id: "done-1",
        event: "interaction_complete",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { result: "Ready." },
      },
      {
        id: "reasoning-complete-late",
        event: "reasoning_complete",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { text: "Evaluating choices." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const thought = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(thought?.type, "thinking");
  assert.equal(thought?.text, "Evaluating choices.");
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "Ready.");
});

test("mapFramesToTimelineEntries replaces pending reasoning with the authoritative complete block", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "risk-red-team",
      member_id: "risk-red-team",
      label: "risk-red-team",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        interactionId: "turn-1",
        identity: "risk-red-team",
        data: { delta: "Searching for context.\n\nEvaluating relocation choices." },
      },
      {
        id: "reasoning-complete-slice",
        event: "reasoning_complete",
        interactionId: "turn-1",
        identity: "risk-red-team",
        data: { text: "Evaluating relocation choices." },
      },
      {
        id: "text-1",
        event: "text_delta",
        interactionId: "turn-1",
        identity: "risk-red-team",
        data: { delta: "Answer." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const thinking = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(thinking?.type, "thinking");
  assert.equal(thinking?.text, "Evaluating relocation choices.");
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "Answer.");
});

test("mapFramesToTimelineEntries upgrades prior reasoning when a late complete is fuller", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { delta: "Searching for context." },
      },
      {
        id: "text-1",
        event: "text_delta",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { delta: "Ready." },
      },
      {
        id: "done-1",
        event: "interaction_complete",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { result: "Ready." },
      },
      {
        id: "reasoning-complete-late",
        event: "reasoning_complete",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { text: "Searching for context.\n\nEvaluating choices." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const thought = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(thought?.type, "thinking");
  assert.equal(thought?.text, "Searching for context.\n\nEvaluating choices.");
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "Ready.");
});

test("mapFramesToTimelineEntries flushes pending reasoning before another interaction completes", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        interactionId: "turn-1",
        identity: "incident-worker-1",
        data: { delta: "First turn reasoning." },
      },
      {
        id: "reasoning-complete-2",
        event: "reasoning_complete",
        interactionId: "turn-2",
        identity: "incident-worker-1",
        data: { text: "Second turn reasoning." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  const first = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  const second = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(first?.type, "thinking");
  assert.equal(first?.text, "First turn reasoning.");
  assert.equal(second?.type, "thinking");
  assert.equal(second?.text, "Second turn reasoning.");
});

test("mapFramesToTimelineEntries preserves late unscoped reasoning separately from prior traces", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        identity: "incident-worker-1",
        data: { delta: "Searching for context." },
      },
      {
        id: "text-1",
        event: "text_delta",
        identity: "incident-worker-1",
        data: { delta: "Ready." },
      },
      {
        id: "reasoning-2",
        event: "reasoning_complete",
        identity: "incident-worker-1",
        data: { text: "Searching for context.\n\nEvaluating a later independent turn." },
      },
    ],
  );

  assert.equal(entries.length, 3);
  const first = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  const answer = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(first?.type, "thinking");
  assert.equal(first?.text, "Searching for context.");
  assert.equal(answer?.type, "markdown");
  assert.equal(answer?.source, "Ready.");
  const late = entries[2] && "blocks" in entries[2] ? entries[2].blocks?.[0] : null;
  assert.equal(late?.text, "Searching for context.\n\nEvaluating a later independent turn.");
});

test("mapFramesToTimelineEntries marks in-flight reasoning deltas as non-final", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-worker-1",
      member_id: "incident-worker-1",
      label: "incident-worker-1",
      kind: "mob_agent",
    },
    [
      {
        id: "reasoning-1",
        event: "reasoning_delta",
        identity: "incident-worker-1",
        data: { delta: "Still thinking" },
      },
    ],
  );

  assert.equal(entries.length, 1);
  const thought = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(thought?.type, "thinking");
  assert.equal(thought?.text, "Still thinking");
  assert.equal(thought?.final, undefined);
});

// AgentEvent::ReasoningComplete contains the assembler's entire current block.
// Exercise both public adapters so the reusable surface follows the same owner contract.
for (const [surface, mapper] of [
  ["stock", mapFramesToTimelineEntries],
  ["shared", mapFramesToTimelineEntriesShared],
] as const) {
  const reasoningFrame = (id: string, event: string, text: string, interactionId = "turn-a") => ({
    id, event, interactionId,
    data: event === "reasoning_delta" ? { delta: text } : { content: text },
  });
  const thoughts = (frames: Parameters<typeof mapper>[1]) => mapper(null, frames)
    .flatMap((entry) => entry.kind === "message" ? (entry.blocks || [])
      .filter((block) => block.type === "thinking")
      .map((block) => ({ id: entry.id, text: block.text, final: block.final })) : []);

  test(`${surface}: reasoning completion replaces damaged fragments exactly with a stable entry id`, () => {
    const frames = [
      reasoningFrame("reasoning-open", "reasoning_delta", "Check "),
      reasoningFrame("reasoning-fragment", "reasoning_delta", "again and finish."),
    ];
    assert.deepEqual(thoughts(frames), [{ id: "reasoning-open", text: "Check again and finish.", final: undefined }]);
    const completed = "  Check again again and finish.\r\n";
    assert.deepEqual(thoughts([...frames, reasoningFrame("reasoning-done", "reasoning_complete", completed)]), [
      { id: "reasoning-open", text: completed, final: true },
    ]);
  });

  test(`${surface}: distinct completed reasoning blocks retain identical words in one interaction`, () => {
    assert.deepEqual(thoughts([
      reasoningFrame("first-open", "reasoning_delta", "Check again."),
      reasoningFrame("first-done", "reasoning_complete", "Check again."),
      reasoningFrame("second-open", "reasoning_delta", "Check again."),
      reasoningFrame("second-done", "reasoning_complete", "Check again."),
      reasoningFrame("third-complete-only", "reasoning_complete", "Check again."),
    ]), [
      { id: "first-open", text: "Check again.", final: true },
      { id: "second-open", text: "Check again.", final: true },
      { id: "third-complete-only", text: "Check again.", final: true },
    ]);
  });

  test(`${surface}: late reasoning completion updates only its unfinished block after a visual flush`, () => {
    assert.deepEqual(thoughts([
      reasoningFrame("first-open", "reasoning_delta", "Keep this earlier block."),
      reasoningFrame("first-done", "reasoning_complete", "Keep this earlier block."),
      reasoningFrame("second-open", "reasoning_delta", "Damaged second block."),
      { id: "answer", event: "text_delta", interactionId: "turn-a", data: { delta: "Answer." } },
      reasoningFrame("second-done", "reasoning_complete", "Exact completed second block."),
    ]), [
      { id: "first-open", text: "Keep this earlier block.", final: true },
      { id: "second-open", text: "Exact completed second block.", final: true },
    ]);
  });

  test(`${surface}: reasoning deltas continue the open block across interleaved text`, () => {
    assert.deepEqual(thoughts([
      reasoningFrame("reasoning-open", "reasoning_delta", "Check "),
      { id: "answer", event: "text_delta", interactionId: "turn-a", data: { delta: "Answer." } },
      reasoningFrame("reasoning-next", "reasoning_delta", "again."),
      reasoningFrame("reasoning-done", "reasoning_complete", "Check again."),
    ]), [{ id: "reasoning-open", text: "Check again.", final: true }]);
  });

  test(`${surface}: foreign reasoning completion cannot rewrite another interaction`, () => {
    assert.deepEqual(thoughts([
      reasoningFrame("first-open", "reasoning_delta", "First provisional."),
      { id: "answer", event: "text_delta", interactionId: "turn-a", data: { delta: "Answer." } },
      reasoningFrame("foreign-complete", "reasoning_complete", "First provisional. Foreign complete.", "turn-b"),
      reasoningFrame("first-done", "reasoning_complete", "First canonical."),
    ]), [
      { id: "first-open", text: "First canonical.", final: true },
      { id: "foreign-complete", text: "First provisional. Foreign complete.", final: true },
    ]);
  });

  test(`${surface}: explicit run identity prevents reasoning completion from claiming another run`, () => {
    assert.deepEqual(thoughts([
      { ...reasoningFrame("first-open", "reasoning_delta", "First run."), runId: "run-a" },
      { ...reasoningFrame("other-complete", "reasoning_complete", "Other run."), runId: "run-b" },
      { ...reasoningFrame("first-complete", "reasoning_complete", "First run completed."), runId: "run-a" },
    ]), [
      { id: "first-open", text: "First run completed.", final: true },
      { id: "other-complete", text: "Other run.", final: true },
    ]);
  });
}

test("mapFramesToTimelineEntries renders session-history tool-use only assistant turns", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "nested-worker-coordinator",
      member_id: "nested-worker-coordinator",
      label: "nested-worker-coordinator",
      kind: "mob_agent",
    },
    [
      {
        id: "history-tool-use",
        event: "interaction_complete",
        identity: "nested-worker-coordinator",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "tool_use",
                data: {
                  id: "call-delegate",
                  name: "delegate",
                  args: {
                    member_id: "nested-ack-worker",
                    task: "acknowledge",
                  },
                },
              },
            ],
            stop_reason: "tool_use",
          },
          text: "",
          result: "",
        },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.variant, "rich");
  const firstBlock = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(firstBlock?.type, "tool-call");
  assert.equal(firstBlock?.name, "delegate");
  assert.match(firstBlock?.arguments || "", /nested-ack-worker/);
});

test("mapFramesToTimelineEntries attaches session-history tool results to tool-use turns", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "stress-worker",
      member_id: "stress-worker",
      label: "stress-worker",
      kind: "mob_agent",
    },
    [
      {
        id: "history-tool-use",
        event: "interaction_complete",
        identity: "stress-worker",
        sourceKind: "session_history",
        timestampMs: 10,
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "tool_use",
                data: { id: "call-peers", name: "peers", args: {} },
              },
            ],
            stop_reason: "tool_use",
          },
          text: "",
          result: "",
        },
      },
      {
        id: "history-tool-result",
        event: "tool_execution_completed",
        identity: "stress-worker",
        sourceKind: "session_history",
        timestampMs: 11,
        data: {
          id: "call-peers",
          tool_call_id: "call-peers",
          result: JSON.stringify("{\"peers\":[{\"peer_id\":\"peer-1\",\"name\":\"mob/worker/peer-1\"}]}"),
          is_error: false,
        },
      },
    ],
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.name, "peers");
  assert.equal(
    block?.type === "tool-call" ? block.result : "",
    "1 peers · worker 1\nFirst peers: peer-1",
  );
  assert.equal(block?.type === "tool-call" ? block.status : "", "success");
});

test("mapFramesToTimelineEntries drops session-history tool-use blocks already rendered live", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "browser-parent-worker",
      member_id: "browser-parent-worker",
      label: "browser-parent-worker",
      kind: "mob_agent",
    },
    [
      {
        id: "live-mob-list-requested",
        event: "tool_call_requested",
        identity: "browser-parent-worker",
        interactionId: "turn-1",
        data: { id: "call-live-mob-list", name: "mob_list", args: {} },
      },
      {
        id: "live-mob-list-completed",
        event: "tool_execution_completed",
        identity: "browser-parent-worker",
        interactionId: "turn-1",
        data: { id: "call-live-mob-list", name: "mob_list", result: "{\"mobs\":[]}" },
      },
      {
        id: "history-mob-list-tool-use",
        event: "interaction_complete",
        identity: "browser-parent-worker",
        interactionId: "turn-1",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "tool_use",
                data: {
                  id: "call-live-mob-list",
                  name: "mob_list",
                  args: {},
                },
              },
            ],
            stop_reason: "tool_use",
          },
          text: "",
          result: "",
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const toolBlocks = entries.flatMap((entry) => entry && "blocks" in entry && Array.isArray(entry.blocks)
    ? entry.blocks.filter((block) => block.type === "tool-call")
    : []);
  assert.equal(toolBlocks.length, 1);
  assert.equal(toolBlocks[0]?.name, "mob_list");
});

test("mapFramesToTimelineEntries preserves extra history tool-use repeats beyond live count", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "browser-parent-worker",
      member_id: "browser-parent-worker",
      label: "browser-parent-worker",
      kind: "mob_agent",
    },
    [
      {
        id: "live-peer-requested",
        event: "tool_call_requested",
        identity: "browser-parent-worker",
        data: { id: "call-live-peer", name: "peers", args: {} },
      },
      {
        id: "history-peer-tool-use",
        event: "interaction_complete",
        identity: "browser-parent-worker",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              { block_type: "tool_use", data: { id: "call-history-peer-1", name: "peers", args: {} } },
              { block_type: "tool_use", data: { id: "call-history-peer-2", name: "peers", args: {} } },
            ],
            stop_reason: "tool_use",
          },
          text: "",
          result: "",
        },
      },
    ],
  );

  const toolBlocks = entries.flatMap((entry) => entry && "blocks" in entry && Array.isArray(entry.blocks)
    ? entry.blocks.filter((block) => block.type === "tool-call")
    : []);
  assert.equal(toolBlocks.length, 2);
  assert.equal(toolBlocks.every((block) => block.name === "peers"), true);
});

test("mapFramesToTimelineEntries deduplicates paired user_input and interaction_started frames", () => {
  const entries = mapFramesToTimelineEntries(
    null,
    [
      {
        id: "console-frame-1",
        event: "user_input",
        interactionId: "turn-1",
        timestampMs: 10,
        data: { content: "Retire rollback-risk-worker please" },
      },
      {
        id: "evt-1",
        event: "interaction_started",
        interactionId: "turn-1",
        timestampMs: 10,
        data: { content: "Retire rollback-risk-worker please" },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(
    entries[0] && "text" in entries[0] ? entries[0].text : "",
    "Retire rollback-risk-worker please",
  );
});

test("mapFramesToTimelineEntries renders user image content blocks inline", () => {
  const entries = mapFramesToTimelineEntries(
    null,
    [
      {
        id: "evt-1",
        event: "interaction_started",
        data: {
          content: [
            { type: "text", text: "Describe this badge." },
            {
              type: "image",
              media_type: "image/jpeg",
              source: "blob",
              blob_id: "sha256:abc/def",
              alt: "incident badge",
            },
          ],
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:7000/",
      renderInteractionStartsAsUser: true,
    },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0]?.variant, "rich");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks || [] : [];
  assert.equal(blocks[0]?.type, "markdown");
  assert.equal((blocks[0]?.type === "paragraph" || blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(blocks[0]) : "", "Describe this badge.");
  assert.equal(blocks[1]?.type, "image");
  assert.equal(
    blocks[1]?.type === "image" ? blocks[1].src : "",
    "http://127.0.0.1:7000/blobs/sha256%3Aabc%2Fdef",
  );
  assert.equal(blocks[1]?.type === "image" ? blocks[1].alt : "", "incident badge");
});

test("mapFramesToTimelineEntries renders user image_ref content blocks inline", () => {
  const entries = mapFramesToTimelineEntries(
    null,
    [
      {
        id: "evt-1",
        event: "interaction_started",
        data: {
          content: [
            { type: "text", text: "Please inspect the forwarded image." },
            {
              type: "image_ref",
              media_type: "image/png",
              source: "blob",
              blob_id: "sha256:forwarded/operator",
            },
          ],
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:7000",
      renderInteractionStartsAsUser: true,
    },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0]?.variant, "rich");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks || [] : [];
  assert.equal(blocks[0]?.type, "markdown");
  assert.equal((blocks[0]?.type === "paragraph" || blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(blocks[0]) : "", "Please inspect the forwarded image.");
  assert.equal(blocks[1]?.type, "image");
  assert.equal(
    blocks[1]?.type === "image" ? blocks[1].src : "",
    "http://127.0.0.1:7000/blobs/sha256%3Aforwarded%2Foperator",
  );
  assert.equal(blocks[1]?.type === "image" ? blocks[1].alt : "", "referenced image");
});

test("mapFramesToTimelineEntries renders assistant_image frames with API blob URLs", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "artist",
      member_id: "artist",
      label: "Artist",
      kind: "identity",
    },
    [
      {
        id: "evt-image",
        event: "assistant_image",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          blob_id: "sha256:abc/def",
          media_type: "image/png",
          width: 512,
          height: 512,
        },
      },
    ],
    { blobBaseUrl: "http://127.0.0.1:7000/" },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "image");
  assert.equal(block?.src, "http://127.0.0.1:7000/blobs/sha256%3Aabc%2Fdef");
});

test("mapFramesToTimelineEntries renders assistant_image_appended frames without raw metadata", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-image-appended",
        event: "assistant_image_appended",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          image: {
            blob_ref: { blob_id: "sha256:generated", media_type: "image/png" },
            height: 1024,
            image_id: "image-1",
            media_type: "image/png",
            meta: { response_id: "resp_123" },
            width: 1024,
          },
          source_event_type: "assistant_image_appended",
          type: "assistant_image_appended",
        },
      },
    ],
    { blobBaseUrl: "http://127.0.0.1:7000/" },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "image");
  assert.equal(block?.src, "http://127.0.0.1:7000/blobs/sha256%3Agenerated");
});

test("mapFramesToTimelineEntries deduplicates assistant image and appended image events for the same blob", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-image",
        event: "assistant_image",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          blob_id: "sha256:generated",
          media_type: "image/png",
          width: 1024,
          height: 1024,
        },
      },
      {
        id: "evt-image-appended",
        event: "assistant_image_appended",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          image: {
            blob_ref: { blob_id: "sha256:generated", media_type: "image/png" },
            height: 1024,
            image_id: "image-1",
            media_type: "image/png",
            width: 1024,
          },
        },
      },
    ],
    { blobBaseUrl: "http://127.0.0.1:7000/" },
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "image");
  assert.equal(block?.src, "http://127.0.0.1:7000/blobs/sha256%3Agenerated");
});

test("mapFramesToTimelineEntries treats spawn-looking interaction prompts as user text", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-spawn",
        event: "interaction_started",
        interactionId: "spawn-turn",
        timestampMs: 10,
        data: {
          content: "You have been spawned as 'incident-commander' (role: commander) in mob 'incident-command-center'.",
        },
      },
      {
        id: "evt-delta",
        event: "text_delta",
        interactionId: "spawn-turn",
        timestampMs: 20,
        data: { delta: "Acknowledged." },
      },
      {
        id: "evt-complete",
        event: "interaction_complete",
        interactionId: "spawn-turn",
        timestampMs: 30,
        data: { result: "Acknowledged." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(
    "text" in (entries[0] || {}) ? entries[0]?.text : "",
    "You have been spawned as 'incident-commander' (role: commander) in mob 'incident-command-center'.",
  );
  assert.equal(entries[1]?.identity.role, "assistant");
  assert.equal(
    entries[1] && "blocks" in entries[1] && Array.isArray(entries[1].blocks)
      ? (entries[1].blocks[0]?.type === "paragraph" || entries[1].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[1].blocks[0]) : ""
      : "",
    "Acknowledged.",
  );
});

test("mapFramesToTimelineEntries renders inbound comms run_started prompts as user work", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "run_started",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          prompt: "[COMMS REQUEST from incident-command-center/incident_commander/incident-commander]\nIntent: request_summary\nBody: Summarize the incident.",
        },
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        timestampMs: Date.parse("2026-04-06T23:00:02.000Z"),
        data: { result: "Summary sent back to commander." },
      },
    ],
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(
    entries[0] && "text" in entries[0] ? entries[0].text : "",
    "[COMMS REQUEST from incident-command-center/incident_commander/incident-commander]\nIntent: request_summary\nBody: Summarize the incident.",
  );
  assert.equal(entries[1]?.identity.id, "scribe");
});

test("mapFramesToTimelineEntries renders inbound one-line peer run_started prompts as user work", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "run_started",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          prompt: "[COMMS MESSAGE from incident-command-center/commander/incident-commander] Please describe what you see in the attached image.",
        },
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        timestampMs: Date.parse("2026-04-06T23:00:02.000Z"),
        data: { result: "The image shows a winged fox commander." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(
    entries[0] && "text" in entries[0] ? entries[0].text : "",
    "[COMMS MESSAGE from incident-command-center/commander/incident-commander] Please describe what you see in the attached image.",
  );
  assert.equal(entries[1]?.identity.id, "scribe");
});

test("mapFramesToTimelineEntries renders inbound comms-looking user-input frames as operator chat", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "tutti-profile-worker",
      member_id: "tutti-profile-worker",
      label: "tutti-profile-worker",
      kind: "identity",
    },
    [
      {
        id: "evt-comms-user-input",
        event: "user_input",
        timestampMs: Date.parse("2026-05-11T15:48:20.000Z"),
        data: {
          content:
            "[COMMS MESSAGE from incident-command-center/worker/tutti-profile-worker]\n"
            + "Ping: please confirm you are still online and reachable for CardinalPay incident support.",
        },
      },
      {
        id: "evt-complete",
        event: "interaction_complete",
        timestampMs: Date.parse("2026-05-11T15:48:22.000Z"),
        data: { result: "Confirmed online and reachable via comms." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(
    "text" in (entries[0] || {}) ? entries[0]?.text : "",
    "[COMMS MESSAGE from incident-command-center/worker/tutti-profile-worker]\nPing: please confirm you are still online and reachable for CardinalPay incident support.",
  );
  assert.equal(entries[1]?.identity.id, "tutti-profile-worker");
});

test("mapFramesToTimelineEntries renders session-history typed comms frames as comms metadata", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "child-worker",
      member_id: "child-worker",
      label: "child-worker",
      kind: "identity",
    },
    [
      {
        id: "history-comms-user-input",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-11T20:45:57.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "implicit-019e18c9-cd01-7ad0-8a78-ce86f780706b/delegate/grandchild-worker",
            body: "Peer message from implicit-019e18c9-cd01-7ad0-8a78-ce86f780706b/delegate/grandchild-worker:\ngrandchild-worker ping acknowledgement.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerIncoming, true);
  assert.equal(block?.peerTarget, "grandchild-worker");
  assert.equal(block?.peerBody, "Peer message from implicit-019e18c9-cd01-7ad0-8a78-ce86f780706b/delegate/grandchild-worker:\ngrandchild-worker ping acknowledgement.");
});

test("mapFramesToTimelineEntries preserves outgoing typed comms direction", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "outgoing-comms",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-b",
            body: "Please review this patch.",
            direction: "outgoing",
          }),
        },
      },
    ],
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.type === "tool-call" ? block.peerIncoming : true, false);
  assert.equal(block?.type === "tool-call" ? block.peerTarget : "", "peer-b");
});

test("mapFramesToTimelineEntries suppresses repeated assistant history after an inbound comms notice", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "qa-parent-worker",
      member_id: "qa-parent-worker",
      label: "qa-parent-worker",
      kind: "identity",
    },
    [
      {
        id: "history-final-1",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-13T18:57:45.401Z"),
        data: {
          message: {
            role: "block_assistant",
            blocks: [{
              block_type: "text",
              data: { text: "qa-child-worker said: “Ping acknowledged.”" },
            }],
          },
          result: "qa-child-worker said: “Ping acknowledged.”",
          source_event_type: "session_history",
          type: "session_history",
        },
      },
      {
        id: "history-comms",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-13T18:57:45.406Z"),
        data: {
          message: typedCommsNotice({
            peer: "implicit/delegate/qa-child-worker",
            body: "Ping acknowledged.",
          }),
        },
      },
      {
        id: "history-final-2",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-13T18:57:47.816Z"),
        data: {
          message: {
            role: "block_assistant",
            blocks: [{
              block_type: "text",
              data: { text: "qa-child-worker said: “Ping acknowledged.”" },
            }],
          },
          result: "qa-child-worker said: “Ping acknowledged.”",
          source_event_type: "session_history",
          type: "session_history",
        },
      },
    ],
  );

  const assistantMessages = entries.filter(
    (entry) => entry.kind === "message" && entry.identity.id === "qa-parent-worker",
  );
  assert.equal(assistantMessages.length, 1);
  const assistantText = "text" in assistantMessages[0]
    ? assistantMessages[0].text
    : "blocks" in assistantMessages[0] && (assistantMessages[0].blocks?.[0]?.type === "paragraph" || assistantMessages[0].blocks?.[0]?.type === "markdown") ? conversationRichBlockCopyText(assistantMessages[0].blocks[0]) : "";
  assert.equal(assistantText, "qa-child-worker said: “Ping acknowledged.”");
  const commsMessages = entries.filter(
    (entry) => entry.kind === "message" && entry.identity.id === "comms",
  );
  assert.equal(commsMessages.length, 1);
});

test("mapFramesToTimelineEntries renders live typed comms system notices as comms metadata", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "child-worker",
      member_id: "child-worker",
      label: "child-worker",
      kind: "identity",
    },
    [
      {
        id: "live-comms-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-11T20:45:57.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "implicit-019e18c9-cd01-7ad0-8a78-ce86f780706b/delegate/grandchild-worker",
            body: "grandchild-worker ping acknowledgement.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerIncoming, true);
  assert.equal(block?.peerTarget, "grandchild-worker");
  assert.equal(block?.peerBody, "grandchild-worker ping acknowledgement.");
});

test("mapFramesToTimelineEntries suppresses raw run_started peer envelopes when structured comms notice exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer message\n"
            + "Peer message from fugue/issue_lead/LUC-642/issue_lead:\n"
            + "Peer message from fugue/issue_lead/LUC-642/issue_lead:\n"
            + "Focused RED-review replan is complete and Linear is back in Implementing...\n"
            + "Peer message",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            body: "Focused RED-review replan is complete and Linear is back in Implementing...",
          }),
        },
      },
      {
        id: "evt-complete",
        event: "interaction_complete",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: { result: "Acknowledged." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  assert.equal(entries.some((entry) => (
    entry.identity.id === "user"
    && "text" in entry
    && entry.text.includes("Peer message from fugue/issue_lead")
  )), false);
  assert.equal(entries[1]?.identity.id, "planner");
});

test("mapFramesToTimelineEntries keeps raw peer prompts when structured comms body is only partial", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer message from fugue/peer-a:\n"
            + "Done.\n"
            + "Please also validate the retry plan.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(
    entries[0] && "text" in entries[0] ? entries[0].text : "",
    "Peer message from fugue/peer-a:\nDone.\nPlease also validate the retry plan.",
  );
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses colon-delimited peer identity envelopes", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer message\n"
            + "Peer message from review:singleton:\n"
            + "Done.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries matches raw peer envelopes against structured display names", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer message\n"
            + "Peer message from fugue/issue_lead/LUC-642/issue_lead:\n"
            + "Focused RED-review replan is complete and Linear is back in Implementing...",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            peerId: "peer-uuid-1",
            peerDisplayName: "fugue/issue_lead/LUC-642/issue_lead",
            body: "Focused RED-review replan is complete and Linear is back in Implementing...",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries.some((entry) => (
    entry.identity.id === "user"
    && "text" in entry
    && entry.text.includes("Peer message from fugue/issue_lead")
  )), false);
});

test("mapFramesToTimelineEntries suppresses raw peer response envelopes when structured comms notice exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer response\n"
            + "Peer response from fugue/issue_lead/LUC-642/issue_lead:\n"
            + "Done.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            kind: "response",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries.some((entry) => (
    entry.identity.id === "user"
    && "text" in entry
    && entry.text.includes("Peer response from fugue/issue_lead")
  )), false);
});

test("mapFramesToTimelineEntries suppresses raw peer request envelopes when structured comms notice exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer request\n"
            + "Peer request from fugue/issue_lead/LUC-642/issue_lead:\n"
            + "Can you validate the current plan?",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            kind: "request",
            body: "Can you validate the current plan?",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries.some((entry) => (
    entry.identity.id === "user"
    && "text" in entry
    && entry.text.includes("Peer request from fugue/issue_lead")
  )), false);
});

test("mapFramesToTimelineEntries suppresses plain intent/body peer envelopes when structured comms notice exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer request from fugue/peer-a:\n"
            + "Intent: request_summary\n"
            + "Body: Summarize the incident.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            kind: "request",
            body: "Summarize the incident.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries.some((entry) => (
    entry.identity.id === "user"
    && "text" in entry
    && entry.text.includes("Intent: request_summary")
  )), false);
});

test("mapFramesToTimelineEntries suppresses duplicated intent/body peer envelopes when structured comms notice exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer request\n"
            + "Peer request from fugue/peer-a:\n"
            + "Peer request from fugue/peer-a:\n"
            + "Intent: request_summary\n"
            + "Body: Summarize the incident.\n"
            + "Peer request",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            kind: "request",
            body: "Summarize the incident.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses duplicated bracketed intent/body comms envelopes", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "[COMMS REQUEST from fugue/peer-a]\n"
            + "[COMMS REQUEST from fugue/peer-a]\n"
            + "Intent: request_summary\n"
            + "Body: Summarize the incident.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            kind: "request",
            body: "Summarize the incident.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries does not suppress peer requests with structured peer responses", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-request",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt: "Peer request from fugue/peer-a:\nDone.",
        },
      },
      {
        id: "structured-response",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
            kind: "response",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries consumes one raw peer prompt per structured duplicate", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nSame update.",
        },
      },
      {
        id: "raw-two",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nSame update.",
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Same update.",
          }),
        },
      },
      {
        id: "structured-two",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Same update.",
            requestId: "second",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries.every((entry) => entry.identity.id === "comms"), true);
});

test("mapFramesToTimelineEntries does not overconsume raw prompts for one legacy structured notice", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nSame update.",
        },
      },
      {
        id: "raw-two",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nSame update.",
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: {
            kind: "comms",
            body: "Peer message from fugue/peer-a:\nSame update.",
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 1);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
});

test("mapFramesToTimelineEntries dedupes alias-equivalent typed comms notice signatures", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-display:\nSame update.",
        },
      },
      {
        id: "raw-two",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-display:\nSame update.",
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-display",
            peerId: "fugue/peer-id",
            peerDisplayName: "fugue/peer-display",
            body: "Peer message from fugue/peer-display:\nSame update.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 1);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
});

test("mapFramesToTimelineEntries consumes duplicate raw prompts for duplicate typed comms blocks", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nSame update.",
        },
      },
      {
        id: "raw-two",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nSame update.",
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: {
            role: "system_notice",
            kind: "comms",
            blocks: [
              {
                type: "comms",
                kind: "message",
                direction: "incoming",
                peer: {
                  id: "fugue/peer-a",
                  display_name: "fugue/peer-a",
                },
                content: [{ type: "text", text: "Same update." }],
              },
              {
                type: "comms",
                kind: "message",
                direction: "incoming",
                peer: {
                  id: "fugue/peer-a",
                  display_name: "fugue/peer-a",
                },
                content: [{ type: "text", text: "Same update." }],
              },
            ],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 0);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
});

test("mapFramesToTimelineEntries suppresses raw prompts for body-only typed comms notices", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: "Peer message from fugue/peer-a:\nClean body.",
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: {
            role: "system_notice",
            kind: "comms",
            body: "Clean body.",
            blocks: [{
              type: "comms",
              kind: "message",
              direction: "incoming",
              peer: {
                id: "fugue/peer-a",
                display_name: "fugue/peer-a",
              },
              request_id: "body-only",
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 0);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(block?.type === "tool-call" ? block.peerBody : "", "Clean body.");
});

test("mapFramesToTimelineEntries suppresses bracketed intent/body comms prompts", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: "[COMMS REQUEST from fugue/peer-a]\nIntent: request_summary\nBody: Summarize the incident.",
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            kind: "request",
            intent: "request_summary",
            body: "Summarize the incident.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 0);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
});

test("mapFramesToTimelineEntries preserves literal Body lines in clean comms bodies", () => {
  const body = "Here is the report\nBody: section one";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: `[COMMS REQUEST from fugue/peer-a]\nIntent: request_summary\nBody: ${body}`,
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            kind: "request",
            intent: "request_summary",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 0);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "Here is the report\nBody: section one",
  );
});

test("mapFramesToTimelineEntries preserves clean comms bodies that look like intent wrappers", () => {
  const body = "Intent: preserve this line\nBody: preserve this too";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-one",
        event: "run_started",
        data: {
          prompt: `[COMMS REQUEST from fugue/peer-a]\nIntent: request_summary\nBody: ${body}`,
        },
      },
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            kind: "request",
            intent: "request_summary",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.filter((entry) => entry.identity.id === "user").length, 0);
  assert.equal(entries.filter((entry) => entry.identity.id === "comms").length, 1);
  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "Intent: preserve this line\nBody: preserve this too",
  );
});

test("mapFramesToTimelineEntries preserves quoted peer envelope lines in clean comms bodies", () => {
  const body = "Peer message from incident-lead:\nPlease keep this quote.";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            kind: "message",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "Peer message from incident-lead:\nPlease keep this quote.",
  );
});

test("mapFramesToTimelineEntries preserves same-peer envelope lines quoted inside clean typed comms bodies", () => {
  const body = "Quoted wrapper:\nPeer message from review:singleton:\nPlease keep this quote.";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            kind: "message",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "Quoted wrapper:\nPeer message from review:singleton:\nPlease keep this quote.",
  );
});

test("mapFramesToTimelineEntries preserves bracketed COMMS note literals in clean comms bodies", () => {
  const body = "[COMMS NOTE]\nIntent: preserve this\nBody: preserve this too";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            kind: "message",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "[COMMS NOTE]\nIntent: preserve this\nBody: preserve this too",
  );
});

test("mapFramesToTimelineEntries preserves quoted bracketed COMMS envelopes in clean comms bodies", () => {
  const body = "[COMMS MESSAGE from incident-lead]\nIntent: preserve this\nBody: preserve this too";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            kind: "message",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "[COMMS MESSAGE from incident-lead]\nIntent: preserve this\nBody: preserve this too",
  );
});

test("mapFramesToTimelineEntries preserves standalone peer scaffold words in clean comms bodies", () => {
  const body = "Peer message\nPlease keep this heading.";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "structured-one",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            kind: "message",
            body,
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const block = entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
    ? entries[0].blocks[0]
    : null;
  assert.equal(
    block?.type === "tool-call" ? block.peerBody : "",
    "Peer message\nPlease keep this heading.",
  );
});

// Meerkat 0.7.1 canonical model-facing peer-request projection: transport
// envelope (peer_spec address + pubkey bytes) plus protocol coaching. This is
// what the runtime stuffs into the typed comms block content for mob kickoff
// requests; none of it may surface in user-facing chat.
const MEERKAT_071_PEER_REQUEST_PROJECTION =
  "Peer request from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c"
  + " (display_name: incident-command-center/commander/incident-commander)"
  + " (id: 964020b4-c9b6-4c31-ba6c-30598279b388)\n"
  + "Intent: mob.kickoff_started\n"
  + "Params: {\n"
  + "  \"peer\": \"incident-commander\",\n"
  + "  \"peer_spec\": {\n"
  + "    \"address\": \"inproc://incident-command-center/commander/incident-commander\",\n"
  + "    \"peer_id\": \"6f6114cd-2cf7-590f-a172-0e36feacd12c\",\n"
  + "    \"pubkey\": [20, 129, 97, 58, 74, 93, 150, 7]\n"
  + "  },\n"
  + "  \"role\": \"commander\"\n"
  + "}\n"
  + "Request ID: 964020b4-c9b6-4c31-ba6c-30598279b388\n"
  + "\n"
  + "This is a correlated peer request. Reply with send_response with arguments"
  + " {\"in_reply_to\":\"964020b4-c9b6-4c31-ba6c-30598279b388\","
  + "\"peer_id\":\"6f6114cd-2cf7-590f-a172-0e36feacd12c\",\"status\":\"completed\"}."
  + " Use status=\"failed\" instead of \"completed\" when the request cannot be"
  + " fulfilled, and include result only when the request contract provides a"
  + " typed result payload. Do not answer this request with send_message.";

function meerkat071KickoffNotice() {
  return {
    role: "system_notice",
    kind: "comms",
    body: "Peer request: mob.kickoff_started",
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
      payload: {
        peer: "incident-commander",
        peer_spec: {
          address: "inproc://incident-command-center/commander/incident-commander",
          peer_id: "6f6114cd-2cf7-590f-a172-0e36feacd12c",
          pubkey: [20, 129, 97, 58, 74, 93, 150, 7],
        },
        role: "commander",
      },
      content: [{ type: "text", text: MEERKAT_071_PEER_REQUEST_PROJECTION }],
    }],
  };
}

// Meerkat 0.8.52 sends member-kickoff status as a one-way typed lifecycle
// notice: comms `kind: "lifecycle"`, the lifecycle kind as `intent`, no
// request id, and model-facing notice text as content.
const MEERKAT_LIFECYCLE_NOTICE_PROJECTION =
  "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c"
  + " (display_name: incident-command-center/commander/incident-commander)\n"
  + "Kind: mob.kickoff_started\n"
  + "Params: {\n"
  + "  \"peer\": \"incident-commander\",\n"
  + "  \"role\": \"commander\"\n"
  + "}\n"
  + "\n"
  + "This is a one-way status notice, not a request. There is nothing to answer:"
  + " do not call send_response or send_message for it.";

function kickoffNoticeFrame(form: "lifecycle" | "request", intent: string, index: number) {
  const summary = `${form === "lifecycle" ? "Peer lifecycle" : "Peer request"}: ${intent}`;
  return {
    id: `kickoff-${form}-${index}`,
    event: "system_notice",
    timestampMs: Date.parse("2026-10-03T18:48:01.519Z") + index,
    sourceKind: "session_history" as const,
    data: {
      message: {
        role: "system_notice",
        kind: "comms",
        body: summary,
        blocks: [{
          type: "comms",
          kind: form,
          direction: "incoming",
          peer: {
            id: "6f6114cd-2cf7-590f-a172-0e36feacd12c",
            display_name: "incident-command-center/commander/incident-commander",
          },
          ...(form === "request" ? { request_id: `964020b4-c9b6-4c31-ba6c-3059827${index}` } : {}),
          intent,
          summary,
          payload: { peer: "incident-commander", role: "commander" },
          content: [{
            type: "text",
            text: form === "lifecycle"
              ? MEERKAT_LIFECYCLE_NOTICE_PROJECTION.replace("mob.kickoff_started", intent)
              : MEERKAT_071_PEER_REQUEST_PROJECTION.replace("mob.kickoff_started", intent),
          }],
        }],
      },
    },
  };
}

test("mapFramesToTimelineEntries renders every kickoff phase, lifecycle or request form, as its typed status", () => {
  const phases = ["pending", "starting", "started", "callback_pending", "failed", "cancelled"] as const;
  const labels = ["pending", "starting", "started", "waiting for callback", "failed", "cancelled"];
  const agent = { agent_id: "scribe", member_id: "scribe", label: "Scribe", kind: "mob_agent" } as const;
  const check = (label: string, frames: Parameters<typeof mapFramesToTimelineEntries>[1], expected: Record<string, unknown>) => {
    const entries = mapFramesToTimelineEntries(agent, frames, { renderInteractionStartsAsUser: true });
    const blocks = entries.flatMap((entry) => "blocks" in entry && Array.isArray(entry.blocks) ? entry.blocks : []);
    assert.deepEqual(blocks.find((block) => block.type === "member-kickoff"), expected, label);
    assert.ok(!blocks.some((block) => block.type === "tool-call"), `${label}: no peer card`);
    const rendered = JSON.stringify(entries);
    for (const marker of ["Peer lifecycle notice from", "Peer request from", "pubkey", "send_response", "nothing to answer"]) {
      assert.ok(!rendered.includes(marker), `${label} must not surface model-facing text "${marker}"`);
    }
  };
  phases.forEach((phase, index) => {
    // Meerkat #1608's serialized lifecycle notice, as is and with the
    // peer_spec live mob payloads also carry.
    const message = structuredClone(MEERKAT_1608_KICKOFF_NOTICES[index]) as unknown as Record<string, unknown>;
    assert.equal((message.blocks as Array<Record<string, unknown>>)[0].intent, `mob.kickoff_${phase}`);
    const lifecycle = {
      type: "member-kickoff", phase, member: "delivery-lead", role: "delivery",
      copyText: `Kickoff ${labels[index]}: delivery-lead`,
    };
    const frame = (data: Record<string, unknown>) => [{
      id: `kickoff-lifecycle-${index}`, event: "system_notice", timestampMs: Date.parse("2026-10-03T18:48:01.519Z") + index,
      sourceKind: "session_history" as const, data: { message: data },
    }];
    check(`lifecycle mob.kickoff_${phase}`, frame(message), lifecycle);
    const withPeerSpec = structuredClone(message);
    const block = (withPeerSpec.blocks as Array<Record<string, unknown>>)[0];
    block.payload = { ...(block.payload as Record<string, unknown>), peer_spec: {
      address: "inproc://incident-command-center/delivery/delivery-lead",
      peer_id: "6f6114cd-2cf7-590f-a172-0e36feacd12c", pubkey: [20, 129, 97, 58],
    } };
    check(`lifecycle mob.kickoff_${phase} with peer_spec`, frame(withPeerSpec), lifecycle);
    // The request form older sessions keep.
    check(`request mob.kickoff_${phase}`, [kickoffNoticeFrame("request", `mob.kickoff_${phase}`, index)], {
      type: "member-kickoff", phase, member: "incident-commander", role: "commander",
      copyText: `Kickoff ${labels[index]}: incident-commander`,
    });
  });
});

test("mapFramesToTimelineEntries renders any other typed lifecycle notice by its summary, never its content", () => {
  const frame = kickoffNoticeFrame("lifecycle", "mob.member_paused", 0);
  const block = frame.data.message.blocks[0];
  block.content = [{ type: "text", text: MEERKAT_LIFECYCLE_NOTICE_PROJECTION.replace("mob.kickoff_started", "mob.member_paused") }];
  const entries = mapFramesToTimelineEntries(
    { agent_id: "scribe", member_id: "scribe", label: "Scribe", kind: "mob_agent" },
    [frame],
    { renderInteractionStartsAsUser: true },
  );
  const card = entries.flatMap((entry) => "blocks" in entry && Array.isArray(entry.blocks) ? entry.blocks : [])
    .find((candidate) => candidate.type === "tool-call");
  assert.equal(card?.type === "tool-call" ? card.peerBody : undefined, "Peer lifecycle: mob.member_paused");
  assert.ok(!JSON.stringify(entries).includes("Peer lifecycle notice from"));
});

test("mapFramesToTimelineEntries never renders the meerkat 0.7.1 peer transport projection as the comms body", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "mob_agent",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-06-12T18:48:01.000Z"),
        data: {
          prompt:
            "Peer request: mob.kickoff_started\n"
            + MEERKAT_071_PEER_REQUEST_PROJECTION,
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-06-12T18:48:01.519Z"),
        sourceKind: "session_history",
        data: { message: meerkat071KickoffNotice() },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const commsEntry = entries.find((entry) => entry.identity.id === "comms");
  assert.ok(commsEntry, "typed comms notice should render a comms entry");
  const block = commsEntry && "blocks" in commsEntry && Array.isArray(commsEntry.blocks)
    ? commsEntry.blocks[0]
    : null;
  // A kickoff request renders as its typed status, not a peer message.
  assert.equal(block?.type, "member-kickoff");
  const statusText = block?.type === "member-kickoff" ? `${block.member} ${block.role ?? ""} ${block.copyText}` : "";
  assert.equal(block?.type === "member-kickoff" ? block.copyText : "", "Kickoff started: incident-commander");
  for (const marker of ["pubkey", "peer_spec", "send_response", "Do not answer this request"]) {
    assert.ok(
      !statusText.includes(marker),
      `kickoff status must not leak transport scaffold marker "${marker}": ${statusText}`,
    );
  }

  for (const entry of entries) {
    if (entry.identity.id !== "user" || entry.kind !== "message" || !("text" in entry)) continue;
    assert.ok(
      !/pubkey|peer_spec|Do not answer this request/.test(entry.text),
      `user entries must not leak transport scaffold: ${entry.text}`,
    );
  }
});

test("mapFramesToTimelineEntries drops pure peer transport scaffold run prompts even without a matching comms notice", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "mob_agent",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-06-12T18:48:01.000Z"),
        data: { prompt: MEERKAT_071_PEER_REQUEST_PROJECTION },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(
    entries.some((entry) => entry.identity.id === "user"),
    false,
    `pure transport scaffold must not render as a user prompt: ${JSON.stringify(entries)}`,
  );
});

test("stripPeerTransportScaffold keeps human-authored remainder and drops the envelope", () => {
  assert.equal(stripPeerTransportScaffold(MEERKAT_071_PEER_REQUEST_PROJECTION), "");
  assert.equal(
    stripPeerTransportScaffold(
      `${MEERKAT_071_PEER_REQUEST_PROJECTION}\nPlease summarize the incident.`,
    ),
    "Please summarize the incident.",
  );
  // Single-line `prompt_text` form terminates with the send_message coaching.
  assert.equal(
    stripPeerTransportScaffold(
      "Peer request from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c."
      + " Intent: review. Request ID: req-1. Params: {\"x\":1}."
      + " This is not a normal user request and not a prompt for direct"
      + " user-facing output. Reply with send_response."
      + " Do not use send_message for this reply.",
    ),
    "",
  );
  // Truncated envelope (no coaching terminator) still never leaks.
  assert.equal(
    stripPeerTransportScaffold(
      "Peer request from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c\nParams: { \"pubkey\": [1, 2, 3",
    ),
    "",
  );
  // Ordinary peer text is untouched.
  assert.equal(
    stripPeerTransportScaffold("Peer request from fugue/peer-a:\nDone."),
    "Peer request from fugue/peer-a:\nDone.",
  );
});

test("mapFramesToTimelineEntries does not suppress explicit peer request with generic structured comms", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-request",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt: "Peer request from fugue/peer-a:\nDone.",
        },
      },
      {
        id: "generic-structured",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses raw bracketed comms response envelopes when structured comms notice exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "[COMMS RESPONSE from fugue/issue_lead/LUC-642/issue_lead]\n"
            + "Done.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            kind: "response",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries.some((entry) => (
    entry.identity.id === "user"
    && "text" in entry
    && entry.text.includes("[COMMS RESPONSE from fugue/issue_lead")
  )), false);
});

test("mapFramesToTimelineEntries suppresses one-line peer envelopes whose body contains colons", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt: "Peer message from review:singleton: Error: failed",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Error: failed",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses bannered peer envelopes whose body contains colons", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt:
            "Peer message\n"
            + "Peer message from review:singleton: Error: failed\n"
            + "Peer message",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Error: failed",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses only the nearest duplicate raw peer prompt", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "early-raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { prompt: "Peer message from review:singleton: Done." },
      },
      {
        id: "nearest-raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:09.000Z"),
        data: { prompt: "Peer message from review:singleton: Done." },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:10.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries does not suppress later envelope-looking prompts", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "actual-duplicate-before-structured",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { prompt: "Peer message from review:singleton: Done." },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:10.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Done.",
          }),
        },
      },
      {
        id: "later-operator-looking-prompt",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:11.000Z"),
        data: { prompt: "Peer message from review:singleton: Done." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[1]?.identity.id, "user");
  assert.equal(entries[1]?.id, "later-operator-looking-prompt");
});

test("mapFramesToTimelineEntries does not suppress quoted peer envelopes inside operator prompts", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "operator-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt: "Please investigate this:\nPeer message from review:singleton: Done.",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[0] && "text" in entries[0] ? entries[0].text : "", "Please investigate this:\nPeer message from review:singleton: Done.");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses nearest duplicate by frame order without timestamps", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "early-raw-run-started",
        event: "run_started",
        data: { prompt: "Peer message from review:singleton: Done." },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Done.",
          }),
        },
      },
      {
        id: "later-raw-run-started",
        event: "run_started",
        data: { prompt: "Peer message from review:singleton: Done." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[1]?.identity.id, "user");
  assert.equal(entries[1]?.id, "later-raw-run-started");
});

test("mapFramesToTimelineEntries does not suppress colon-prefixed peer aliases", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { prompt: "Peer message from review:singleton: Error" },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review",
            body: "singleton: Error",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses runtime-id peer envelopes without truncating colon bodies", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          prompt: "Peer message from rt:review:singleton:0: Error: failed",
        },
      },
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "rt:review:singleton:0",
            body: "Error: failed",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries preserves typed one-line comms owner content", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "structured-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Peer message from review:singleton: Error: failed",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : undefined;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.type === "tool-call" ? block.peerBody : "", "Peer message from review:singleton: Error: failed");
});

test("mapFramesToTimelineEntries does not suppress unrelated same-body peer prompts", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "notice-peer-a",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
          }),
        },
      },
      {
        id: "run-peer-b",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:01.000Z"),
        data: {
          prompt:
            "Peer message\n"
            + "Peer message from fugue/peer-b:\n"
            + "Done.\n"
            + "Peer message",
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[1]?.identity.id, "user");
  assert.equal(
    entries[1] && "text" in entries[1] ? entries[1].text : "",
    "Peer message\nPeer message from fugue/peer-b:\nDone.\nPeer message",
  );
});

test("mapFramesToTimelineEntries deduplicates live and history copies of the same comms notice", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            body: "Focused RED-review replan is complete.",
          }),
        },
      },
      {
        id: "history-notice",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            body: "Focused RED-review replan is complete.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries deduplicates live comms and session-history terminal system notices", () => {
  const notice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { message: notice },
      },
      {
        id: "history-text-complete",
        event: "text_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: { message: notice },
      },
      {
        id: "history-interaction-complete",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:03.000Z"),
        data: { message: notice },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries deduplicates comms notices when volatile status drifts", () => {
  const liveNotice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  liveNotice.blocks[0] = {
    ...liveNotice.blocks[0],
    status: "sent",
    state: "pending",
  };
  const historyNotice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  historyNotice.blocks[0] = {
    ...historyNotice.blocks[0],
    status: "delivered",
    state: "complete",
  };

  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { message: liveNotice },
      },
      {
        id: "history-interaction-complete",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:03.000Z"),
        data: { message: historyNotice },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerBody, "Done.");
});

test("mapFramesToTimelineEntries deduplicates body-only comms notices when volatile status drifts", () => {
  const liveNotice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  liveNotice.blocks[0] = {
    ...liveNotice.blocks[0],
    content: undefined,
    status: "sent",
    state: "pending",
  };
  const historyNotice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  historyNotice.blocks[0] = {
    ...historyNotice.blocks[0],
    content: undefined,
    status: "delivered",
    state: "complete",
  };

  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { message: liveNotice },
      },
      {
        id: "history-interaction-complete",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:03.000Z"),
        data: { message: historyNotice },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerBody, "Done.");
});

test("mapFramesToTimelineEntries suppresses raw peer prompts using session-history terminal system notices", () => {
  const notice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-peer-prompt",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { prompt: "Peer message from fugue/peer-a:\nDone." },
      },
      {
        id: "history-text-complete",
        event: "text_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: { message: notice },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries suppresses raw peer prompts using body-only status-drift comms notices", () => {
  const notice = typedCommsNotice({
    peer: "fugue/peer-a",
    body: "Done.",
  });
  notice.blocks[0] = {
    ...notice.blocks[0],
    content: undefined,
    status: "delivered",
    state: "complete",
  };
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "raw-peer-prompt",
        event: "run_started",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: { prompt: "Peer message from fugue/peer-a:\nDone." },
      },
      {
        id: "history-text-complete",
        event: "text_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: { message: notice },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerBody, "Done.");
});

test("mapFramesToTimelineEntries keeps opposite-direction comms with the same peer request and body", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-outgoing",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
            direction: "outgoing",
            requestId: "req-shared",
          }),
        },
      },
      {
        id: "history-incoming",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
            direction: "incoming",
            requestId: "req-shared",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  const blocks = entries.map((entry) => (
    entry && "blocks" in entry ? entry.blocks?.[0] : null
  ));
  assert.equal(blocks[0]?.type, "tool-call");
  assert.equal(blocks[0]?.peerIncoming, false);
  assert.equal(blocks[1]?.type, "tool-call");
  assert.equal(blocks[1]?.peerIncoming, true);
});

test("mapFramesToTimelineEntries preserves leading peer-envelope-looking text in structured comms content", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "notice-quoted-envelope",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "review:singleton",
            body: "Peer message from review:singleton:\nPlease keep this quote.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(
    block?.peerBody,
    "Peer message from review:singleton:\nPlease keep this quote.",
  );
});

test("mapFramesToTimelineEntries preserves leading peer-envelope-looking text in body-only structured comms", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "notice-body-only-quote",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: {
            role: "system_notice",
            kind: "comms",
            body: "Peer message from review:singleton:\nPlease keep this quote.",
            blocks: [{
              type: "comms",
              kind: "message",
              direction: "incoming",
              peer: { id: "review:singleton", display_name: "review:singleton" },
              request_id: "req-body-only",
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(
    block?.peerBody,
    "Peer message from review:singleton:\nPlease keep this quote.",
  );
});

test("mapFramesToTimelineEntries preserves leading slash peer envelopes in structured comms content", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "notice-slash-envelope",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/issue_lead/LUC-642/issue_lead",
            body: "Peer message from fugue/issue_lead/LUC-642/issue_lead:\nPlease keep this quote.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(
    block?.peerBody,
    "Peer message from fugue/issue_lead/LUC-642/issue_lead:\nPlease keep this quote.",
  );
});

test("mapFramesToTimelineEntries keeps mixed comms notice frames when later blocks differ", () => {
  const multiNotice = (
    id: string,
    sourceKind: "session_history" | undefined,
    secondPeer: string,
    secondBody: string,
  ) => ({
    id,
    event: "system_notice",
    sourceKind,
    timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
    data: {
      message: {
        role: "system_notice",
        kind: "comms",
        blocks: [
          {
            type: "comms",
            kind: "message",
            direction: "incoming",
            peer: { id: "peer-a", display_name: "peer-a" },
            request_id: "shared-a",
            content: [{ type: "text", text: "same" }],
          },
          {
            type: "comms",
            kind: "message",
            direction: "incoming",
            peer: { id: secondPeer, display_name: secondPeer },
            request_id: `unique-${secondPeer}`,
            content: [{ type: "text", text: secondBody }],
          },
        ],
      },
    },
  });
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      multiNotice("live-notice", undefined, "peer-b", "unique live"),
      multiNotice("history-notice", "session_history", "peer-c", "unique history"),
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  const firstBlocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks ?? [] : [];
  const secondBlocks = entries[1] && "blocks" in entries[1] ? entries[1].blocks ?? [] : [];
  assert.equal(firstBlocks.length, 2);
  assert.equal(secondBlocks.length, 1);
  assert.equal(
    secondBlocks[0]?.type === "tool-call" ? secondBlocks[0].peerTarget : "",
    "peer-c",
  );
  assert.equal(
    secondBlocks[0]?.type === "tool-call" ? secondBlocks[0].peerBody : "",
    "unique history",
  );
});

test("mapFramesToTimelineEntries filters duplicate legacy comms blocks in mixed notices", () => {
  const legacyNotice = (
    id: string,
    sourceKind: "session_history" | undefined,
    secondPeer: string,
    secondBody: string,
  ) => ({
    id,
    event: "system_notice",
    sourceKind,
    timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
    data: {
      message: {
        role: "system_notice",
        kind: "generic",
        blocks: [
          {
            type: "text",
            body: "Peer message from peer-a:\nsame",
          },
          {
            type: "text",
            body: `Peer message from ${secondPeer}:\n${secondBody}`,
          },
        ],
      },
    },
  });
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      legacyNotice("live-notice", undefined, "peer-b", "unique live"),
      legacyNotice("history-notice", "session_history", "peer-c", "unique history"),
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  const secondBlocks = entries[1] && "blocks" in entries[1] ? entries[1].blocks ?? [] : [];
  assert.equal(secondBlocks.length, 1);
  assert.equal(secondBlocks[0]?.type === "divider" ? secondBlocks[0].text : "", "Peer message from peer-c:\nunique history");
});

test("mapFramesToTimelineEntries keeps same-peer same-body comms with different request ids", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
            requestId: "request-a",
          }),
        },
      },
      {
        id: "history-notice",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
            requestId: "request-b",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries keeps same-body history comms from different peers", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "planner",
      member_id: "planner",
      label: "Planner",
      kind: "identity",
    },
    [
      {
        id: "live-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-27T08:00:00.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-a",
            body: "Done.",
          }),
        },
      },
      {
        id: "history-notice",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-27T08:00:02.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "fugue/peer-b",
            body: "Done.",
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[1]?.identity.id, "comms");
});

test("mapFramesToTimelineEntries renders live untyped peer system notices", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "deep-investigator",
      member_id: "deep-investigator",
      label: "Deep Investigator",
      kind: "identity",
    },
    [
      {
        id: "live-untyped-peer-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-23T21:10:00.000Z"),
        data: {
          blocks: [{
            content: [{
              type: "text",
              text: "Peer message from ops/investigation-worker/investigation-worker-live-proof:\nLIVE_PEER_NOTICE landed in the parent chat.",
            }],
          }],
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  const text = entries[0] && "blocks" in entries[0] && (entries[0].blocks?.[0]?.type === "paragraph" || entries[0].blocks?.[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : "";
  assert.match(text, /LIVE_PEER_NOTICE landed in the parent chat/);
});

test("mapFramesToTimelineEntries renders live tool-config system notices as metadata", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "catalog-worker",
      member_id: "catalog-worker",
      label: "catalog-worker",
      kind: "identity",
    },
    [
      {
        id: "live-tool-config-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-15T09:00:00.000Z"),
        data: {
          role: "system_notice",
          kind: "tool_scope",
          render_class: "tool_scope_notice",
          body: "Deferred catalog changed at turn boundary: new deferred tools available: docs_search",
          blocks: [{
            type: "tool_config",
            payload: {
              operation: "reload",
              target: "deferred_catalog",
              status: "deferred_catalog_delta(added_hidden=1,removed_hidden=0,pending_sources=0)",
              status_info: {
                kind: "deferred_catalog_delta",
                added_hidden_count: 1,
                removed_hidden_count: 0,
                pending_source_count: 0,
              },
              persisted: false,
              domain: "deferred_catalog",
            },
          }],
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "divider");
  assert.equal(
    block?.type === "divider" ? block.text : "",
    "Deferred catalog changed at turn boundary: new deferred tools available: docs_search",
  );
});

test("mapFramesToTimelineEntries renders live non-comms system notices without tool or image blocks", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "ops-worker",
      member_id: "ops-worker",
      label: "ops-worker",
      kind: "identity",
    },
    [
      {
        id: "live-mcp-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-15T09:01:00.000Z"),
        data: {
          message: {
            role: "system_notice",
            kind: "mcp",
            body: "MCP server docs connected",
            blocks: [{
              type: "mcp",
              server_id: "docs",
              detail: "MCP server docs connected",
              persisted: true,
            }],
          },
        },
      },
      {
        id: "live-runtime-notice",
        event: "system_notice",
        timestampMs: Date.parse("2026-05-15T09:02:00.000Z"),
        data: {
          message: {
            role: "system_notice",
            kind: "generic",
            body: "Runtime recovered from transient stream lag",
            blocks: [{
              type: "runtime_notice",
              category: "stream",
              detail: "Runtime recovered from transient stream lag",
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const mcpBlock = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(mcpBlock?.type, "divider");
  assert.equal(mcpBlock?.type === "divider" ? mcpBlock.text : "", "MCP server docs connected");

  const runtimeBlock = entries[1] && "blocks" in entries[1] ? entries[1].blocks?.[0] : null;
  assert.equal(runtimeBlock?.type, "paragraph");
  assert.equal(
    (runtimeBlock?.type === "paragraph" || runtimeBlock?.type === "markdown") ? conversationRichBlockCopyText(runtimeBlock) : "",
    "Runtime recovered from transient stream lag",
  );
});

test("systemNoticeClearsBusyState only treats peer/comms notices as terminal", () => {
  const activeToolFrames = [
    {
      id: "tool-start",
      event: "tool_execution_started" as const,
      timestampMs: 1,
      data: { name: "index_search" },
    },
    {
      id: "tool-done",
      event: "tool_execution_completed" as const,
      timestampMs: 2,
      data: { name: "index_search", result: "ok" },
    },
  ];
  const runtimeNotice = {
    id: "runtime-notice",
    event: "system_notice" as const,
    timestampMs: 3,
    data: {
      message: {
        role: "system_notice",
        kind: "generic",
        body: "Received from runtime control plane: recovered from transient stream lag",
        blocks: [{
          type: "runtime_notice",
          category: "stream",
          detail: "Received from runtime control plane: recovered from transient stream lag",
        }],
      },
    },
  };
  const toolConfigNotice = {
    id: "tool-config-notice",
    event: "system_notice" as const,
    timestampMs: 4,
    data: {
      role: "system_notice",
      kind: "tool_scope",
      body: "Peer message from tool catalog is not real comms; deferred tools available: docs_search",
      blocks: [{ type: "tool_config", payload: { operation: "reload" } }],
    },
  };
  const nestedRuntimePeerPhraseNotice = {
    id: "nested-runtime-peer-phrase-notice",
    event: "system_notice" as const,
    timestampMs: 4.5,
    data: {
      message: {
        role: "system_notice",
        kind: "generic",
        body: "Runtime metadata update",
        blocks: [{
          type: "runtime_notice",
          category: "stream",
          detail: "runtime payload mentioned Peer message from docs-worker but it is not a comms notice",
          payload: {
            text: "Peer message from docs-worker should not count when nested in runtime metadata",
          },
        }],
      },
    },
  };
  const commsNotice = {
    id: "comms-notice",
    event: "system_notice" as const,
    timestampMs: 5,
    data: {
      message: typedCommsNotice({
        peer: "ops/delegate/worker",
        body: "Peer result landed.",
      }),
    },
  };
  const untypedPeerNotice = {
    id: "untyped-peer-notice",
    event: "system_notice" as const,
    timestampMs: 6,
    data: {
      blocks: [{
        content: [{
          type: "text",
          text: "Peer message from ops/worker:\nFinished.",
        }],
      }],
    },
  };
  const runtimeReceivedNotice = {
    id: "runtime-received-notice",
    event: "system_notice" as const,
    timestampMs: 7,
    data: {
      message: {
        role: "system_notice",
        kind: "generic",
        body: "Received from runtime metadata channel: lease renewed",
        blocks: [{
          type: "runtime_notice",
          category: "lease",
          detail: "Received from runtime metadata channel: lease renewed",
        }],
      },
    },
  };

  assert.equal(systemNoticeClearsBusyState(runtimeNotice), false);
  assert.equal(systemNoticeClearsBusyState(toolConfigNotice), false);
  assert.equal(systemNoticeClearsBusyState(runtimeReceivedNotice), false);
  assert.equal(systemNoticeClearsBusyState(nestedRuntimePeerPhraseNotice), false);
  assert.equal(systemNoticeClearsBusyState(commsNotice), true);
  assert.equal(systemNoticeClearsBusyState(untypedPeerNotice), true);
  assert.equal(
    inferResponsePhaseFromFrames([...activeToolFrames, runtimeNotice], null),
    "waiting",
    "runtime notices must not make an active tool turn look idle",
  );
  assert.equal(
    inferResponsePhaseFromFrames([...activeToolFrames, toolConfigNotice], null),
    "waiting",
    "tool-config notices must not make an active tool turn look idle",
  );
  assert.equal(
    inferResponsePhaseFromFrames([...activeToolFrames, commsNotice], "waiting"),
    null,
    "peer/comms notices still clear completed peer-send busy state",
  );
});

test("mapFramesToTimelineEntries hides external-event system notices that duplicate user input", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "user-input",
        event: "user_input",
        timestampMs: 1,
        data: { content: "Create a worker" },
      },
      {
        id: "external-event-history",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: 2,
        data: {
          message: {
            role: "system_notice",
            kind: "external_event",
            body: "Create a worker",
            blocks: [{
              type: "external_event",
              source: "rpc",
              event_type: "rpc",
              summary: "Create a worker",
              body: "Create a worker",
              payload: { body: "Create a worker" },
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal("text" in (entries[0] || {}) ? entries[0]?.text : "", "Create a worker");
});

test("mapFramesToTimelineEntries hides image external-event notices after rich user input", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "api-investigator",
      member_id: "api-investigator",
      label: "API Investigator",
      kind: "identity",
    },
    [
      {
        id: "user-input",
        event: "user_input",
        timestampMs: 1,
        data: {
          content: [
            { type: "text", text: "Describe the attached image." },
            {
              type: "image",
              source: "blob",
              blob_id: "sha256:badge",
              media_type: "image/png",
            },
          ],
        },
      },
      {
        id: "external-event-history",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: 2,
        data: {
          message: {
            role: "system_notice",
            kind: "external_event",
            body: "Describe the attached image.\n[image: image/png]",
            blocks: [{
              type: "external_event",
              source: "rpc",
              event_type: "rpc",
              summary: "Describe the attached image.\n[image: image/png]",
              body: "Describe the attached image.\n[image: image/png]",
              content: [
                { type: "text", text: "Describe the attached image." },
                {
                  type: "image",
                  source: "blob",
                  blob_id: "sha256:badge",
                  media_type: "image/png",
                },
              ],
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true, blobBaseUrl: "http://localhost:63212" },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "user");
  assert.equal(entries[0]?.variant, "rich");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks : [];
  assert.equal(blocks?.filter((block) => block.type === "paragraph" || block.type === "markdown").length, 1);
  assert.equal(blocks?.filter((block) => block.type === "image").length, 1);
});

test("mapFramesToTimelineEntries renders session-history typed peer system notices as comms metadata", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "final-grandchild-worker",
      member_id: "final-grandchild-worker",
      label: "final-grandchild-worker",
      kind: "identity",
    },
    [
      {
        id: "history-peer-system-notice",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-11T21:10:23.000Z"),
        data: {
          message: typedCommsNotice({
            peer: "incident-command-center/commander/final-child-worker",
            body: "ping acknowledgement to final-child-worker",
            kind: "request",
            intent: "checksum_token",
            requestId: "8d04e806-3316-40c4-816f-345ded237fbc",
            payload: { subject: "ping acknowledgement to final-child-worker" },
          }),
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerIncoming, true);
  assert.equal(block?.peerTarget, "final-child-worker");
  assert.equal(block?.peerIntent, "checksum_token");
  assert.equal(block?.peerBody, "ping acknowledgement to final-child-worker");
});

test("mapFramesToTimelineEntries renders session-history peer image notices as comms plus image", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "history-peer-image",
        event: "system_notice",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-11T21:12:00.000Z"),
        data: {
          message: {
            role: "system_notice",
            blocks: [{
              type: "comms",
              kind: "message",
              peer: { display_name: "incident-command-center/scribe/scribe" },
              request_id: "peer-image-1",
              content: [
                { type: "text", text: "Generated incident badge forwarded." },
                {
                  type: "image_ref",
                  blob_ref: {
                    blob_id: "sha256:badge/1",
                    media_type: "image/png",
                  },
                  media_type: "image/png",
                  alt: "incident badge",
                },
              ],
            }],
          },
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:7000",
      renderInteractionStartsAsUser: true,
    },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  assert.equal(entries[0]?.variant, "rich");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks || [] : [];
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0]?.type, "tool-call");
  assert.equal(blocks[0]?.type === "tool-call" ? blocks[0].peerIncoming : false, true);
  assert.equal(blocks[0]?.type === "tool-call" ? blocks[0].peerTarget : "", "scribe");
  assert.equal(blocks[0]?.type === "tool-call" ? blocks[0].peerBody : "", "Generated incident badge forwarded.");
  const peerImages = blocks[0]?.type === "tool-call" ? blocks[0].peerImages || [] : [];
  assert.equal(peerImages.length, 1);
  assert.equal(
    peerImages[0]?.src || "",
    "http://127.0.0.1:7000/blobs/sha256%3Abadge%2F1",
  );
  assert.equal(peerImages[0]?.alt || "", "incident badge");
});

test("mapFramesToTimelineEntries renders live typed peer blob-ref image notices as comms plus image", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "image-recipient",
      member_id: "image-recipient",
      label: "Image Recipient",
      kind: "identity",
    },
    [
      {
        id: "live-peer-image",
        event: "system_notice",
        timestampMs: Date.parse("2026-06-03T21:12:00.000Z"),
        data: {
          message: {
            role: "system_notice",
            blocks: [{
              type: "comms",
              kind: "message",
              peer: { display_name: "ops/delegate/image-artist-2" },
              request_id: "peer-image-live-1",
              content: [
                { type: "text", text: "Saved to generated_images/ops_admin_role_v2.png." },
                {
                  type: "image",
                  image: {
                    blob_ref: {
                      blob_id: "sha256:ops-admin-role",
                      media_type: "image/png",
                    },
                    width: 1024,
                    height: 1024,
                    image_id: "image-ops-admin-role",
                  },
                },
              ],
            }],
          },
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:7000",
      renderInteractionStartsAsUser: true,
    },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks || [] : [];
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0]?.type, "tool-call");
  assert.equal(blocks[0]?.type === "tool-call" ? blocks[0].peerTarget : "", "image-artist-2");
  assert.equal(blocks[0]?.type === "tool-call" ? blocks[0].peerBody : "", "Saved to generated_images/ops_admin_role_v2.png.");
  const peerImages = blocks[0]?.type === "tool-call" ? blocks[0].peerImages || [] : [];
  assert.equal(peerImages.length, 1);
  assert.equal(
    peerImages[0]?.src || "",
    "http://127.0.0.1:7000/blobs/sha256%3Aops-admin-role",
  );
  assert.equal(peerImages[0]?.width || 0, 1024);
  assert.equal(peerImages[0]?.imageId || "", "image-ops-admin-role");
});

test("mapFramesToTimelineEntries suppresses raw peer image prompts when structured comms image exists", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "identity",
    },
    [
      {
        id: "raw-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-06-03T22:06:43.000Z"),
        data: {
          prompt:
            "Peer message\n"
            + "Peer message from incident-command-center/commander/incident-commander:\n"
            + "Peer message from incident-command-center/commander/incident-commander:\n"
            + "Generated fictional CardinalPay payments-api outage dashboard image with required labels and rollback 64%.\n"
            + "Generated fictional CardinalPay payments-api outage dashboard image with required labels and rollback 64%.\n"
            + "[image: image/png]\n"
            + "Peer message",
        },
      },
      {
        id: "structured-comms-image",
        event: "system_notice",
        timestampMs: Date.parse("2026-06-03T22:06:42.000Z"),
        data: {
          message: {
            role: "system_notice",
            blocks: [{
              type: "comms",
              kind: "message",
              peer: { display_name: "incident-command-center/commander/incident-commander" },
              request_id: "peer-image-duplicate",
              content: [
                {
                  type: "text",
                  text:
                    "Generated fictional CardinalPay payments-api outage dashboard image with required labels and rollback 64%.\n"
                    + "Generated fictional CardinalPay payments-api outage dashboard image with required labels and rollback 64%.",
                },
                {
                  type: "image_ref",
                  blob_ref: {
                    blob_id: "sha256:cardinalpay-dashboard",
                    media_type: "image/png",
                  },
                  media_type: "image/png",
                },
              ],
            }],
          },
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:7000",
      renderInteractionStartsAsUser: true,
    },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0]?.identity.id, "comms");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks || [] : [];
  assert.equal(blocks.length, 1);
  const block = blocks[0];
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.type === "tool-call" ? block.peerTarget : "", "incident-commander");
  assert.equal(block?.type === "tool-call" ? (block.peerImages || []).length : 0, 1);
});

test("mapFramesToTimelineEntries does not render raw peer image run-started prompts", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "identity",
    },
    [
      {
        id: "raw-peer-image-run-started",
        event: "run_started",
        timestampMs: Date.parse("2026-06-03T22:30:01.655Z"),
        data: {
          prompt: [
            {
              type: "text",
              text:
                "Peer message\n"
                + "Peer message from incident-command-center/commander/incident-commander:\n"
                + "Peer message from incident-command-center/commander/incident-commander:\n"
                + "codex-comms-image-mpyn1bac Generated fictional CardinalPay payments-api outage dashboard image with required labels and rollback 64%.\n"
                + "codex-comms-image-mpyn1bac Generated fictional CardinalPay payments-api outage dashboard image with required labels and rollback 64%.\n"
                + "[image: image/png]\n"
                + "Peer message",
            },
            {
              type: "image",
              source: "inline",
              media_type: "image/png",
              data: "iVBORw0KGgo=",
            },
          ],
        },
      },
    ],
    {
      blobBaseUrl: "http://127.0.0.1:7000",
      renderInteractionStartsAsUser: true,
    },
  );

  assert.equal(entries.length, 0);
});

test("mapFramesToTimelineEntries resolves session-history peer tool targets from live peers results", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "peers-result",
        event: "tool_execution_completed",
        timestampMs: Date.parse("2026-05-11T21:29:47.000Z"),
        data: {
          id: "call-peers",
          name: "peers",
          result: JSON.stringify({
            peers: [{
              peer_id: "16e23049-513c-5ec1-94f6-892a5daf2f89",
              name: "incident-command-center/scribe/scribe",
            }],
          }),
        },
      },
      {
        id: "history-send-message",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-11T21:29:50.000Z"),
        data: {
          message: {
            role: "block_assistant",
            stop_reason: "tool_use",
            blocks: [{
              block_type: "tool_use",
              data: {
                id: "call-send",
                name: "send_message",
                args: {
                  peer_id: "16e23049-513c-5ec1-94f6-892a5daf2f89",
                  body: "peer label check",
                },
              },
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const toolEntry = entries.find((entry) =>
    entry.kind === "message"
    && "blocks" in entry
    && entry.blocks?.[0]?.type === "tool-call"
  );
  const block = toolEntry && "blocks" in toolEntry ? toolEntry.blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.peerTarget, "scribe");
});

test("mapFramesToTimelineEntries summarizes image refs in peer send_message tool bodies", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "image-worker",
      member_id: "image-worker",
      label: "image-worker",
      kind: "identity",
    },
    [
      {
        id: "history-send-image",
        event: "interaction_complete",
        sourceKind: "session_history",
        timestampMs: Date.parse("2026-05-11T21:13:00.000Z"),
        data: {
          message: {
            role: "block_assistant",
            stop_reason: "tool_use",
            blocks: [{
              block_type: "tool_use",
              data: {
                id: "call-send-image",
                name: "send_message",
                args: {
                  peer_id: "peer-image-target",
                  body: [
                    { type: "text", text: "Forwarding generated result." },
                    {
                      type: "image_ref",
                      source: "blob",
                      blob_id: "sha256:generated/1",
                      media_type: "image/png",
                      alt: "generated incident badge",
                    },
                  ],
                },
              },
            }],
          },
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  const block = entries[0] && "blocks" in entries[0] ? entries[0].blocks?.[0] : null;
  assert.equal(block?.type, "tool-call");
  assert.equal(block?.type === "tool-call" ? block.peerBody : "", "Forwarding generated result. generated incident badge");
});

test("mapFramesToTimelineEntries deduplicates repeated session-history kickoff prompts", () => {
  const prompt = "You are the child worker in a worker-chain test. Create a worker named grandchild-worker.";
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "child-worker",
      member_id: "child-worker",
      label: "child-worker",
      kind: "identity",
    },
    [
      {
        id: "history-kickoff-1",
        event: "user_input",
        sourceKind: "session_history",
        timestampMs: 1,
        data: { content: prompt },
      },
      {
        id: "history-kickoff-2",
        event: "user_input",
        sourceKind: "session_history",
        timestampMs: 2,
        data: { content: prompt },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 1);
  assert.equal(entries[0] && "text" in entries[0] ? entries[0].text : "", prompt);
});

test("mapFramesToTimelineEntries renders inbound content-block run_started prompts as user work", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "scribe",
      member_id: "scribe",
      label: "Scribe",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "run_started",
        timestampMs: Date.parse("2026-04-06T23:00:00.000Z"),
        data: {
          prompt: [
            { type: "text", text: "[COMMS MESSAGE from incident-command-center/commander/incident-commander]" },
            { type: "text", text: "Please describe this generated self-portrait image." },
            { type: "image", data: "base64-image-data", media_type: "image/png" },
          ],
        },
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        timestampMs: Date.parse("2026-04-06T23:00:02.000Z"),
        data: { result: "The image shows a winged fox commander." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0]?.variant, "rich");
  const blocks = entries[0] && "blocks" in entries[0] ? entries[0].blocks || [] : [];
  assert.equal(blocks.length, 3);
  assert.equal(
    blocks
      .filter((block) => block.type === "paragraph" || block.type === "markdown")
      .map((block) => (block.type === "paragraph" || block.type === "markdown") ? conversationRichBlockCopyText(block) : "")
      .join("\n"),
    "[COMMS MESSAGE from incident-command-center/commander/incident-commander]\nPlease describe this generated self-portrait image.",
  );
  assert.equal(blocks[2]?.type, "image");
  assert.equal(blocks[2]?.type === "image" ? blocks[2].src : "", "data:image/png;base64,base64-image-data");
  assert.equal(entries[1]?.identity.id, "scribe");
});

test("mapFramesToTimelineEntries orders persisted interaction history by interaction semantics, not raw arrival order", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "evt-2",
        event: "text_delta",
        interactionId: "turn-1",
        timestampMs: 10,
        data: { delta: "Working on it." },
      },
      {
        id: "evt-1",
        event: "interaction_started",
        interactionId: "turn-1",
        timestampMs: 11,
        data: { content: "Run a status sweep." },
      },
      {
        id: "evt-3",
        event: "interaction_complete",
        interactionId: "turn-1",
        timestampMs: 12,
        data: { text: "Working on it." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0] && "text" in entries[0] ? entries[0].text : "", "Run a status sweep.");
  assert.equal(entries[1]?.identity.role, "assistant");
  assert.equal(
    entries[1] && "blocks" in entries[1] && Array.isArray(entries[1].blocks)
      ? (entries[1].blocks[0]?.type === "paragraph" || entries[1].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[1].blocks[0]) : ""
      : "",
    "Working on it.",
  );
});

test("mapFramesToTimelineEntries keeps accepted user input before the later assistant response when frames arrive reversed", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "incident-commander",
      member_id: "incident-commander",
      label: "Incident Commander",
      kind: "identity",
    },
    [
      {
        id: "assistant-frame",
        cursor: "console:10",
        event: "interaction_complete",
        interactionId: "runtime-turn-1",
        timestampMs: Date.parse("2026-05-07T15:16:28.000Z"),
        data: { text: "OK" },
      },
      {
        id: "user-frame",
        cursor: "console:11",
        event: "user_input",
        interactionId: "console-interaction-1",
        timestampMs: Date.parse("2026-05-07T15:16:26.000Z"),
        data: { content: "Reply with exactly OK." },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  assert.equal(entries.length, 2);
  assert.equal(entries[0]?.identity.role, "user");
  assert.equal(entries[0] && "text" in entries[0] ? entries[0].text : "", "Reply with exactly OK.");
  assert.equal(entries[1]?.identity.role, "assistant");
  assert.equal(
    entries[1] && "blocks" in entries[1] && Array.isArray(entries[1].blocks)
      ? (entries[1].blocks[0]?.type === "paragraph" || entries[1].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[1].blocks[0]) : ""
      : "",
    "OK",
  );
});

test("mapFramesToTimelineEntries keeps no-interaction peer turns after the originating user turn", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "review",
      member_id: "review:singleton",
      label: "Review Agent",
      kind: "identity",
    },
    [
      {
        id: "peer-result-started",
        cursor: "console:1141",
        event: "run_started",
        timestampMs: Date.parse("2026-05-25T10:43:26.000Z"),
        data: {
          prompt:
            "Peer message from ops/review-worker/review-worker-vibe-forward-chat-review-fix: { result: true }",
        },
      },
      {
        id: "peer-result-complete",
        cursor: "console:3106",
        event: "interaction_complete",
        timestampMs: Date.parse("2026-05-25T10:43:49.000Z"),
        data: {
          result:
            "Tool progress: Full review result forwarded to the Vibe Forward initiative agent.",
        },
      },
      {
        id: "operator-prompt",
        cursor: "console:425",
        event: "user_input",
        interactionId: "console-interaction-review",
        timestampMs: Date.parse("2026-05-25T10:42:04.000Z"),
        data: {
          content:
            "Console chat smoke chat-review-fix: run a fresh OSIR review for initiative Vibe Forward.",
        },
      },
      {
        id: "operator-handoff",
        cursor: "console:923",
        event: "interaction_complete",
        interactionId: "console-interaction-review",
        timestampMs: Date.parse("2026-05-25T10:42:23.000Z"),
        data: {
          result:
            "Tool progress: Spawned exactly one review-worker. Worker handoff.",
        },
      },
    ],
    { renderInteractionStartsAsUser: true },
  );

  const texts = entries.map((entry) =>
    "text" in entry
      ? entry.text
      : "blocks" in entry
        ? JSON.stringify(entry.blocks)
        : "",
  );
  const promptIndex = texts.findIndex((text) => text.includes("Console chat smoke"));
  const handoffIndex = texts.findIndex((text) => text.includes("Worker handoff"));
  const peerIndex = texts.findIndex((text) => text.includes("Peer message from ops/review-worker"));
  const finalIndex = texts.findIndex((text) => text.includes("Full review result forwarded"));

  assert(promptIndex >= 0, `missing operator prompt: ${texts.join("\n---\n")}`);
  assert(handoffIndex > promptIndex, `handoff must follow prompt: ${texts.join("\n---\n")}`);
  assert(peerIndex > handoffIndex, `peer turn must follow originating turn: ${texts.join("\n---\n")}`);
  assert(finalIndex > peerIndex, `final result must follow peer turn: ${texts.join("\n---\n")}`);
});

test("mapFramesToTimelineEntries decodes stringified delta payloads from persisted history", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "merchant-success",
      member_id: "merchant-success",
      label: "Merchant Success",
      kind: "identity",
    },
    [
      {
        id: "evt-1",
        event: "text_delta",
        timestampMs: 1,
        data: "{\"delta\":\"Enterprise merchants are experiencing significant payment failures.\",\"source_event_type\":\"text_delta\",\"type\":\"text_delta\"}",
      },
      {
        id: "evt-2",
        event: "interaction_complete",
        timestampMs: 2,
        data: { text: "Enterprise merchants are experiencing significant payment failures." },
      },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : "",
    "Enterprise merchants are experiencing significant payment failures.",
  );
});

test("mapFramesToTimelineEntries preserves whitespace-only text deltas instead of stringifying the payload", () => {
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "payments-sre",
      member_id: "payments-sre",
      label: "Payments SRE",
      kind: "identity",
    },
    [
      { id: "evt-1", event: "text_delta", data: { delta: "Payments-API remains degraded at" } },
      { id: "evt-2", event: "text_delta", data: { delta: " " } },
      { id: "evt-3", event: "text_delta", data: { delta: "38%" } },
      { id: "evt-4", event: "interaction_complete", data: { text: "Payments-API remains degraded at 38%" } },
    ],
  );

  assert.equal(entries.length, 1);
  assert.equal(
    entries[0] && "blocks" in entries[0] && Array.isArray(entries[0].blocks)
      ? (entries[0].blocks[0]?.type === "paragraph" || entries[0].blocks[0]?.type === "markdown") ? conversationRichBlockCopyText(entries[0].blocks[0]) : ""
      : "",
    "Payments-API remains degraded at 38%",
  );
});

test("mergeConversationFrames deduplicates history and live copies of the same event", () => {
  const merged = mergeConversationFrames(
    [
      {
        id: "evt-1",
        event: "interaction_started",
        interactionId: "turn-1",
        data: { content: "Run a status sweep." },
      },
      {
        id: "evt-2",
        event: "text_delta",
        interactionId: "turn-1",
        data: { delta: "Working" },
      },
    ],
    [
      {
        id: "evt-2",
        event: "text_delta",
        interactionId: "turn-1",
        data: { delta: "Working" },
      },
      {
        id: "evt-3",
        event: "interaction_complete",
        interactionId: "turn-1",
        data: { text: "Working" },
      },
    ],
  );

  assert.deepEqual(merged.map((frame) => frame.id), ["evt-1", "evt-2", "evt-3"]);
});

test("buildActivityRailViewState hides text deltas and internal config churn", () => {
  const view = buildActivityRailViewState({
    agents: [
      {
        agent_id: "incident-commander",
        member_id: "incident-commander",
        identity: "incident-commander",
        label: "Incident Commander",
        kind: "identity",
      },
    ],
    eventFrames: [
      {
        id: "evt-1",
        event: "text_delta",
        identity: "incident-commander",
        data: { delta: "hello" },
      },
      {
        id: "evt-2",
        event: "tool_config_changed",
        identity: "incident-commander",
        data: { target: "tool_scope" },
      },
      {
        id: "evt-2a",
        event: "snapshot_started",
        identity: "_system",
        data: { type: "snapshot_started", after: "console:1" },
      },
      {
        id: "evt-2b",
        event: "snapshot_complete",
        identity: "_system",
        data: { type: "snapshot_complete", cursor: "console:2" },
      },
      {
        id: "evt-3",
        event: "tool_call_requested",
        identity: "incident-commander",
        data: { id: "call-1", name: "send" },
      },
      {
        id: "evt-4",
        event: "tool_execution_started",
        identity: "incident-commander",
        data: { id: "call-1", name: "send" },
      },
      {
        id: "evt-5",
        event: "interaction_complete",
        identity: "incident-commander",
        data: { text: "done" },
      },
      {
        id: "evt-6",
        event: "interaction_complete",
        identity: "incident-commander",
        sourceKind: "session_history",
        data: {
          message: {
            role: "block_assistant",
            blocks: [
              {
                block_type: "reasoning",
                data: { text: "**Thinking**\n\nDo not show this in the rail." },
              },
            ],
          },
          text: "**Thinking**\n\nDo not show this in the rail.",
        },
      },
    ],
  });

  assert.deepEqual(view.panels[0]?.items.map((item) => item.id), ["event:evt-5"]);
});

test("buildQuickPromptSuggestions projects stock prompt labels into runnable suggestions", () => {
  const suggestions = buildQuickPromptSuggestions({
    agent_id: "incident-commander",
    member_id: "incident-commander",
    label: "Incident Commander",
    kind: "identity",
    labels: {
      console_prompt_1_label: "Status sweep",
      console_prompt_1_value: "Run a status sweep.",
      console_prompt_2_label: "Merchant impact",
      console_prompt_2_value: "Summarize merchant impact.",
    },
  });

  assert.deepEqual(
    suggestions.map((suggestion) => ({ label: suggestion.label, value: suggestion.value })),
    [
      { label: "Status sweep", value: "Run a status sweep." },
      { label: "Merchant impact", value: "Summarize merchant impact." },
    ],
  );
});

test("memory timeline events render as clean meta entries, never raw JSON", () => {
  const frames = [
    {
      id: "m1",
      event: "memory.dream.completed",
      timestampMs: 1000,
      data: { realm: "default", run_id: "run-9", ops_committed: 3, detail: "distilled prefs" },
    },
    {
      id: "m2",
      event: "memory.write.quarantined",
      timestampMs: 2000,
      data: { realm: "default", author: "agent", reason: "low trust" },
    },
    {
      id: "m3",
      event: "memory.taint.transition",
      timestampMs: 3000,
      data: { session_key: "sess-1", kind: "tainted", source: "reset" },
    },
    {
      id: "m4",
      event: "memory.future.event",
      timestampMs: 4000,
      data: { reason: "something new" },
    },
  ];

  const entries = mapFramesToTimelineEntries(null, frames);
  assert.equal(entries.length, 4);
  for (const entry of entries) {
    assert.equal(entry.variant, "meta");
    const text = entry.kind === "message" && "text" in entry ? entry.text || "" : "";
    assert.ok(text.length > 0, "memory entry must have text");
    assert.ok(!text.includes("{"), `memory entry must not leak JSON: ${text}`);
    assert.ok(!text.includes("}"), `memory entry must not leak JSON: ${text}`);
  }

  const dream = entries[0];
  assert.ok(
    dream.kind === "message" && "text" in dream && dream.text?.includes("3 ops committed"),
    "dream.completed summarizes committed ops",
  );

  const unknown = entries[3];
  assert.ok(
    unknown.kind === "message" && "text" in unknown && unknown.text?.startsWith("Memory future event"),
    "unknown memory.* subtypes humanize the event name",
  );
});

test("describeMemoryTimelineEvent produces clean lines for every documented subtype", () => {
  const cases: Array<[string, Record<string, unknown>, RegExp]> = [
    ["memory.dream.started", { run_id: "run-1" }, /Dream started/],
    ["memory.dream.skipped", { reason: "no changes" }, /Dream skipped — no changes/],
    ["memory.record.promoted", { scope_kind: "identity", scope_key: "luka", gated: true }, /promoted to identity:luka \(gated\)/],
    ["memory.quarantine.verdict", { verdict: "reject", rationale: "duplicate" }, /Quarantine verdict: reject — duplicate/],
    ["memory.quarantine.release_blocked", { record_id: "rec-9", verdict: "release", class: "private-key" }, /Quarantine release blocked for rec-9 — matches secret pattern private-key/],
    ["memory.conflict.signal", { entity: "user", topic: "tz", reason: "mismatch" }, /Conflict signal on user \/ tz — mismatch/],
    ["memory.budget.denied", { stage: "harvest", reason: "over cap" }, /Budget denied at harvest — over cap/],
    ["memory.promotion.pending_gate", { scope_kind: "mob", scope_key: "main" }, /awaiting gate for mob:main/],
    ["memory.harvest.completed", { promoted: 2, tombstoned: 1 }, /Harvest completed — 2 promoted, 1 tombstoned/],
    ["memory.distill.timed_out", { cause: "slow" }, /Distill timed out — slow/],
    ["memory.hygiene.applied", { cause: "reset", ops: 4 }, /Hygiene applied — 4 ops/],
    ["memory.hygiene.blocked", { reason: "revision drift" }, /Hygiene blocked — revision drift/],
  ];
  for (const [event, data, expected] of cases) {
    const line = describeMemoryTimelineEvent(event, data);
    assert.match(line, expected, `${event}: ${line}`);
    assert.ok(!line.includes("{") && !line.includes("}"), `${event} leaked JSON: ${line}`);
  }
});

test("describeMemoryTimelineEvent renders exact copy for quarantine release_blocked", () => {
  assert.equal(
    describeMemoryTimelineEvent("memory.quarantine.release_blocked", {
      realm: "main",
      record_id: "rec-9",
      verdict: "release",
      class: "private-key",
    }),
    "Quarantine release blocked for rec-9 — matches secret pattern private-key",
  );
  assert.equal(
    describeMemoryTimelineEvent("memory.quarantine.release_blocked", {
      realm: "main",
      record_id: "rec-12",
      verdict: "promote_pending_gate",
      class: "credential-assignment",
    }),
    "Quarantine promotion blocked for rec-12 — matches secret pattern credential-assignment",
  );
  // Degraded payload still yields a clean line, never the humanized fallback.
  assert.equal(
    describeMemoryTimelineEvent("memory.quarantine.release_blocked", {}),
    "Quarantine release blocked",
  );
});

test("describeMemoryTimelineEvent stays in sync with the console-core mirror", () => {
  // Every payload-dependent branch of the formatter gets its own variant so a
  // divergence in EITHER copy trips the comparison, not just the happy path.
  const cases: Record<string, Array<Record<string, unknown>>> = {
    "memory.dream.started": [{ run_id: "run-1" }, {}],
    "memory.dream.completed": [
      { run_id: "run-1", ops_committed: 3, detail: "2 promoted" },
      { run_id: "run-1", ops_committed: 1 },
      { detail: "nothing to do" },
      {},
    ],
    "memory.dream.skipped": [{ reason: "no changes" }, {}],
    "memory.record.promoted": [
      { record_id: "rec-1", scope_kind: "identity", scope_key: "luka", gated: true },
      { record_id: "rec-1", scope_kind: "identity", gated: false },
      { record_id: "rec-1", scope_key: "main" },
      {},
    ],
    "memory.quarantine.verdict": [
      { record_id: "rec-2", verdict: "reject", rationale: "duplicate" },
      { record_id: "rec-2", verdict: "release" },
      {},
    ],
    "memory.quarantine.release_blocked": [
      { record_id: "rec-9", verdict: "release", class: "private-key" },
      { record_id: "rec-12", verdict: "promote_pending_gate", class: "credential-assignment" },
      { record_id: "rec-13", verdict: "future_verdict", class: "github-token" },
      { verdict: "release", class: "aws-access-key-id" },
      { record_id: "rec-14", verdict: "release" },
      {},
    ],
    "memory.conflict.signal": [
      { entity: "user", topic: "tz", reason: "mismatch" },
      { entity: "user" },
      { topic: "tz" },
      {},
    ],
    "memory.write.quarantined": [
      { author: "distiller", reason: "tainted session" },
      { reason: "tainted session" },
      { author: "distiller" },
      {},
    ],
    "memory.taint.transition": [
      { session_key: "s-1", kind: "tainted", source: "web_fetch" },
      { session_key: "s-1", kind: "reset_boundary" },
      { kind: "rotated_clean", source: "steward" },
      { kind: "future_kind" },
      {},
    ],
    "memory.budget.denied": [
      { stage: "harvest", reason: "over cap" },
      { stage: "harvest" },
      { reason: "over cap" },
      {},
    ],
    "memory.promotion.pending_gate": [
      { pending_id: "p-1", record_id: "rec-3", scope_kind: "mob", scope_key: "main" },
      { scope_kind: "mob" },
      {},
    ],
    "memory.harvest.completed": [
      { identity: "scout", promoted: 2, tombstoned: 1 },
      { promoted: 0 },
      { tombstoned: 3 },
      {},
    ],
    "memory.distill.timed_out": [
      { session_key: "s-2", cause: "slow" },
      { session_key: "s-2" },
      { cause: "slow" },
      {},
    ],
    "memory.hygiene.proposed": [
      { session_key: "s-3", cause: "reset", ops: 2 },
      { cause: "reset" },
      {},
    ],
    "memory.hygiene.applied": [
      { session_key: "s-3", cause: "reset", ops: 4 },
      { ops: 1 },
    ],
    "memory.hygiene.blocked": [
      { session_key: "s-3", cause: "reset", reason: "revision drift" },
      { cause: "reset", ops: 2 },
    ],
    "memory.hygiene.skipped": [
      { session_key: "s-3", cause: "reset", reason: "mid-turn" },
      {},
    ],
    // Unknown-subtype fallback: each best-effort reason source in turn.
    "memory.future.event": [
      { reason: "unknown subtype" },
      { detail: "detail fallback" },
      { cause: "cause fallback" },
      { verdict: "verdict fallback" },
      {},
    ],
    // Non-memory-prefixed event exercises the humanizer's other branch.
    "console.mystery": [{}],
  };
  for (const [event, variants] of Object.entries(cases)) {
    for (const data of variants) {
      assert.equal(
        describeMemoryTimelineEvent(event, data),
        describeMemoryTimelineEventCore(event, data),
        `copies diverge for ${event} with ${JSON.stringify(data)}`,
      );
    }
  }
});

// ── WorkGraph inline card aggregation ───────────────────────────────────────

const WORKGRAPH_AGENT = {
  agent_id: "planner",
  member_id: "planner",
  label: "Planner",
  kind: "identity",
};

function workGraphItem(args: {
  id: string;
  title?: string;
  status?: string;
  revision?: number;
  priority?: string;
  description?: string;
  owner?: { kind: string; id: string; display_name?: string };
  createdAt?: string;
}): Record<string, unknown> {
  return {
    id: args.id,
    realm_id: "realm-1",
    namespace: "default",
    title: args.title || args.id,
    ...(args.description ? { description: args.description } : {}),
    status: args.status || "open",
    priority: args.priority || "medium",
    revision: args.revision ?? 1,
    ...(args.owner
      ? {
          owner: {
            key: { kind: args.owner.kind, id: args.owner.id },
            ...(args.owner.display_name ? { display_name: args.owner.display_name } : {}),
          },
        }
      : {}),
    created_at: args.createdAt || "2026-07-08T09:00:00Z",
    updated_at: "2026-07-08T09:00:00Z",
  };
}

function workGraphToolFrames(args: {
  idPrefix: string;
  name: string;
  callArgs: Record<string, unknown>;
  result: Record<string, unknown>;
  interactionId?: string;
  timestampMs?: number;
}) {
  const interactionId = args.interactionId || "turn-wg";
  const timestampMs = args.timestampMs ?? 1_779_405_464_000;
  return [
    {
      id: `${args.idPrefix}-call`,
      event: "tool_call_requested",
      interactionId,
      timestampMs,
      data: { id: `${args.idPrefix}-tc`, name: args.name, args: args.callArgs },
    },
    {
      id: `${args.idPrefix}-done`,
      event: "tool_execution_completed",
      interactionId,
      timestampMs: timestampMs + 200,
      data: { id: `${args.idPrefix}-tc`, name: args.name, result: JSON.stringify(args.result) },
    },
  ];
}

function workGraphFallbackToolBlocks(entries: ReturnType<typeof mapFramesToTimelineEntries>) {
  return entries.flatMap((entry) => entry.kind === "message"
    ? (entry.blocks || []).filter((block) => block.type === "tool-call")
    : []);
}

for (const name of ["workgraph_ready", "workgraph_list"]) {
  test(`workgraph fallback preserves empty ${name} result and exact copied bytes`, () => {
    const frames = workGraphToolFrames({ idPrefix: name, name, callArgs: { labels: [] }, result: { items: [] } });
    const exactResult = " \n{ \"items\" : [] }\n\t";
    const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [frames[0], {
      ...frames[1], data: { ...frames[1].data, result: exactResult, is_error: false },
    }]);
    const blocks = workGraphFallbackToolBlocks(entries);
    assert.equal(entries.filter((entry) => entry.kind === "workgraph").length, 0);
    assert.equal(blocks.length, 1);
    assert.equal(blocks[0].name, name);
    assert.equal(blocks[0].result, exactResult);
    assert.equal(blocks[0].completionEvidence?.outcome, "success");
    assert.equal(conversationRichBlockCopyText(blocks[0]), `$ ${name}\nInput: {"labels":[]}\nResult: ${exactResult}`);
  });
}

test("workgraph fallback preserves pending and unknown completion before empty success", () => {
  const frames = workGraphToolFrames({ idPrefix: "pending-ready", name: "workgraph_ready", callArgs: {}, result: { items: [] } });
  const pending = workGraphFallbackToolBlocks(mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames.slice(0, 1)));
  assert.equal(pending.length, 1);
  assert.equal(pending[0].status, "pending");
  assert.equal(pending[0].completionEvidence?.outcome, "running");
  assert.equal(pending[0].result, undefined);
  const unknown = workGraphFallbackToolBlocks(mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames));
  assert.equal(unknown.length, 1);
  assert.equal(unknown[0].status, "pending");
  assert.equal(unknown[0].completionEvidence?.outcome, "unknown");
  const completed = workGraphFallbackToolBlocks(mapFramesToTimelineEntries(WORKGRAPH_AGENT, [frames[0], {
    ...frames[1], data: { ...frames[1].data, is_error: false },
  }]));
  assert.equal(completed.length, 1);
  assert.equal(completed[0].toolCallId, pending[0].toolCallId);
  assert.equal(completed[0].status, "success");
  assert.equal(completed[0].completionEvidence?.outcome, "success");
});

test("workgraph fallback pairs a nameless history result with its exact call", () => {
  const frames = workGraphToolFrames({ idPrefix: "history-empty", name: "workgraph_ready", callArgs: {}, result: { items: [] } });
  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [
    { ...frames[0], sourceKind: "session_history" },
    { ...frames[1], sourceKind: "session_history", data: { tool_call_id: "history-empty-tc", content: "{\"items\":[]}", is_error: false } },
  ]);
  const blocks = workGraphFallbackToolBlocks(entries);
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0].toolCallId, "history-empty-tc");
  assert.equal(blocks[0].name, "workgraph_ready");
  assert.equal(blocks[0].result, "{\"items\":[]}");
  assert.equal(blocks[0].completionEvidence?.source, "session-history");
  assert.equal(blocks[0].completionEvidence?.outcome, "success");
});

test("workgraph fallback restores canonical assistant tool blocks with empty results", () => {
  const frames = [
    { id: "canonical-tool", event: "text_complete", sourceKind: "session_history", timestampMs: 1000,
      data: { message: { role: "block_assistant", stop_reason: "tool_use", blocks: [{
        block_type: "tool_use", data: { id: "canonical-empty-call", name: "workgraph_ready", args: {} },
      }] } } },
    { id: "canonical-result", event: "tool_execution_completed", sourceKind: "session_history", timestampMs: 1001,
      data: { tool_call_id: "canonical-empty-call", content: " \n{\"items\":[]}\n", is_error: false } },
  ];
  const blocks = workGraphFallbackToolBlocks(mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames));
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0].name, "workgraph_ready");
  assert.equal(blocks[0].result, " \n{\"items\":[]}\n");
  assert.equal(blocks[0].completionEvidence?.outcome, "success");
});

test("workgraph fallback does not duplicate item cards or hide another empty call", () => {
  const itemFrames = workGraphToolFrames({ idPrefix: "card", name: "workgraph_list", callArgs: {}, result: { items: [workGraphItem({ id: "visible-item" })] } });
  const emptyFrames = workGraphToolFrames({ idPrefix: "empty-next", name: "workgraph_ready", callArgs: {}, result: { items: [] }, timestampMs: 1_779_405_465_000 });
  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...itemFrames, ...emptyFrames]);
  assert.equal(entries.filter((entry) => entry.kind === "workgraph").length, 1);
  assert.deepEqual(workGraphFallbackToolBlocks(entries).map((block) => block.toolCallId), ["empty-next-tc"]);
});

test("workgraph fallback keeps unrooted errors in their existing card without duplicate rows", () => {
  const frames = workGraphToolFrames({ idPrefix: "unrooted-error", name: "workgraph_ready", callArgs: {}, result: {} });
  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [frames[0], {
    ...frames[1], data: { ...frames[1].data, is_error: true, result: "Unable to read ready set" },
  }]);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  assert.equal(cards[0].lastActionFailed, true);
  assert.match((cards[0].recentEvents || []).join("\n"), /Unable to read ready set/);
  assert.deepEqual(workGraphFallbackToolBlocks(entries), []);
});

test("workgraph create→claim→close folds into one evolving card with correct progress and revisions", () => {
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-1",
      name: "workgraph_create",
      callArgs: { title: "Ship the fix" },
      result: { item: workGraphItem({ id: "item-1", title: "Ship the fix", status: "open", revision: 1 }) },
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-2",
      name: "workgraph_claim",
      callArgs: { id: "item-1", expected_revision: 1, owner: { kind: "agent", id: "planner" } },
      result: {
        item: workGraphItem({
          id: "item-1",
          title: "Ship the fix",
          status: "in_progress",
          revision: 2,
          owner: { kind: "agent", id: "planner", display_name: "Planner" },
        }),
      },
      timestampMs: 1_779_405_465_000,
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-3",
      name: "workgraph_close",
      callArgs: { id: "item-1", expected_revision: 2 },
      result: {
        item: workGraphItem({
          id: "item-1",
          title: "Ship the fix",
          status: "completed",
          revision: 3,
          owner: { kind: "agent", id: "planner", display_name: "Planner" },
        }),
      },
      timestampMs: 1_779_405_466_000,
    }),
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  assert.equal(card.kind === "workgraph" && card.id, "workgraph:interaction:turn-wg");
  if (card.kind !== "workgraph") return;
  assert.equal(card.title, "Ship the fix");
  assert.equal(card.status, "completed");
  assert.deepEqual(card.progress, { completed: 1, total: 1 });
  assert.equal(card.items.length, 1);
  assert.equal(card.items[0].revision, 3);
  assert.equal(card.items[0].status, "completed");
  assert.equal(card.items[0].ownerLabel, "Planner");

  // No generic tool rows for workgraph calls.
  const toolBlockNames = entries.flatMap((entry) => (
    entry.kind === "message" && Array.isArray(entry.blocks)
      ? entry.blocks.filter((block) => block.type === "tool-call").map((block) => block.name)
      : []
  ));
  assert.deepEqual(toolBlockNames.filter((name) => String(name).startsWith("workgraph_")), []);
});

test("workgraph card id and identity stay stable across live passes so updates land in place", () => {
  const createFrames = workGraphToolFrames({
    idPrefix: "wg-1",
    name: "workgraph_create",
    callArgs: { title: "Ship the fix" },
    result: { item: workGraphItem({ id: "item-1", title: "Ship the fix", status: "open", revision: 1 }) },
  });
  const claimFrames = workGraphToolFrames({
    idPrefix: "wg-2",
    name: "workgraph_claim",
    callArgs: { id: "item-1", expected_revision: 1 },
    result: { item: workGraphItem({ id: "item-1", title: "Ship the fix", status: "in_progress", revision: 2 }) },
    timestampMs: 1_779_405_465_000,
  });

  const firstPass = mapFramesToTimelineEntries(WORKGRAPH_AGENT, createFrames)
    .filter((entry) => entry.kind === "workgraph");
  const secondPass = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...createFrames, ...claimFrames])
    .filter((entry) => entry.kind === "workgraph");

  assert.equal(firstPass.length, 1);
  assert.equal(secondPass.length, 1);
  assert.equal(firstPass[0].id, secondPass[0].id);
  assert.equal(firstPass[0].kind === "workgraph" ? firstPass[0].status : "", "active");
  assert.equal(secondPass[0].kind === "workgraph" ? secondPass[0].items[0].revision : 0, 2);
  assert.equal(secondPass[0].identity.role, "assistant");
});

test("workgraph snapshot results hydrate the goal tree, parent depths, and attention bindings", () => {
  const frames = workGraphToolFrames({
    idPrefix: "wg-snap",
    name: "workgraph_snapshot",
    callArgs: {},
    result: {
      snapshot: {
        realm_id: "realm-1",
        all_namespaces: false,
        captured_at: "2026-07-08T09:10:00Z",
        items: [
          workGraphItem({
            id: "goal-1",
            title: "Release 0.7.30",
            description: "Ship WorkGraph end to end",
            status: "in_progress",
            revision: 4,
            createdAt: "2026-07-08T08:00:00Z",
          }),
          workGraphItem({
            id: "child-1",
            title: "Console card",
            status: "completed",
            revision: 2,
            createdAt: "2026-07-08T08:10:00Z",
          }),
          workGraphItem({
            id: "child-2",
            title: "SDK parity",
            status: "open",
            revision: 1,
            priority: "high",
            createdAt: "2026-07-08T08:20:00Z",
          }),
        ],
        edges: [
          { realm_id: "realm-1", namespace: "default", kind: "parent", from_id: "child-1", to_id: "goal-1", created_at: "2026-07-08T08:10:00Z" },
          { realm_id: "realm-1", namespace: "default", kind: "parent", from_id: "child-2", to_id: "goal-1", created_at: "2026-07-08T08:20:00Z" },
        ],
        attention: [
          {
            binding_id: "attention-1",
            work_ref: { realm_id: "realm-1", namespace: "default", item_id: "goal-1" },
            target: { kind: "session", session_id: "sess-42" },
            mode: "pursue",
            status: { state: "active" },
            machine_state: { lifecycle_phase: "active", revision: 7 },
            created_at: "2026-07-08T08:00:00Z",
            updated_at: "2026-07-08T09:00:00Z",
          },
        ],
        ready_item_ids: ["child-2"],
      },
    },
  });

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.id, "workgraph:goal-1");
  assert.equal(card.rootId, "goal-1");
  assert.equal(card.title, "Release 0.7.30");
  assert.equal(card.objective, "Ship WorkGraph end to end");
  assert.equal(card.status, "active");
  assert.deepEqual(card.progress, { completed: 1, total: 3 });
  assert.deepEqual(
    card.items.map((item) => [item.itemId, item.depth]),
    [["goal-1", 0], ["child-1", 1], ["child-2", 1]],
  );
  assert.equal(card.items[2].priority, "high");
  assert.equal(card.attention.length, 1);
  assert.equal(card.attention[0].bindingId, "attention-1");
  assert.equal(card.attention[0].mode, "pursue");
  assert.equal(card.attention[0].statusLabel, "active");
  assert.equal(card.attention[0].revision, 7);
  assert.equal(card.attention[0].targetLabel, "sess-42");
});

test("workgraph unrooted items in one interaction group into a single catch-all card", () => {
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-a",
      name: "workgraph_create",
      callArgs: { title: "Item A" },
      result: { item: workGraphItem({ id: "item-a", title: "Item A", status: "open", revision: 1 }) },
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-b",
      name: "workgraph_create",
      callArgs: { title: "Item B" },
      result: { item: workGraphItem({ id: "item-b", title: "Item B", status: "blocked", revision: 1 }) },
      timestampMs: 1_779_405_465_000,
    }),
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.title, "Work items");
  assert.equal(card.items.length, 2);
  assert.equal(card.status, "active");
});

test("workgraph failed tool results do not poison the card fold", () => {
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-ok",
      name: "workgraph_create",
      callArgs: { title: "Item A" },
      result: { item: workGraphItem({ id: "item-a", title: "Item A", status: "open", revision: 1 }) },
    }),
    {
      id: "wg-err-call",
      event: "tool_call_requested",
      interactionId: "turn-wg",
      timestampMs: 1_779_405_465_000,
      data: { id: "wg-err-tc", name: "workgraph_claim", args: { id: "item-a", expected_revision: 9 } },
    },
    {
      id: "wg-err-done",
      event: "tool_execution_completed",
      interactionId: "turn-wg",
      timestampMs: 1_779_405_465_200,
      data: { id: "wg-err-tc", name: "workgraph_claim", is_error: true, result: "revision conflict" },
    },
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.items[0].status, "open");
  assert.equal(card.items[0].revision, 1);
});

test("workgraph history backfill result frames without a name still fold into the card and never leak generic rows", () => {
  // Backfill (session_history) tool_execution_completed frames carry no
  // `name` — the tool_call_id pairing with the requested frame must resolve
  // it so fold-inclusion and generic-row-exclusion agree.
  const frames = [
    {
      id: "wg-h1-call",
      event: "tool_call_requested",
      interactionId: "turn-history",
      timestampMs: 1_779_405_464_000,
      sourceKind: "session_history",
      data: { id: "wg-h1-tc", name: "workgraph_create", args: { title: "Restored goal" } },
    },
    {
      id: "wg-h1-done",
      event: "tool_execution_completed",
      interactionId: "turn-history",
      timestampMs: 1_779_405_464_200,
      sourceKind: "session_history",
      // No `name` field — exactly what session-history backfill emits.
      data: {
        id: "wg-h1-tc",
        result: JSON.stringify({
          item: workGraphItem({ id: "item-h1", title: "Restored goal", status: "open", revision: 1 }),
        }),
      },
    },
    {
      id: "wg-h2-call",
      event: "tool_call_requested",
      interactionId: "turn-history",
      timestampMs: 1_779_405_465_000,
      sourceKind: "session_history",
      data: { id: "wg-h2-tc", name: "workgraph_close", args: { id: "item-h1", expected_revision: 1 } },
    },
    {
      id: "wg-h2-done",
      event: "tool_execution_completed",
      interactionId: "turn-history",
      timestampMs: 1_779_405_465_200,
      sourceKind: "session_history",
      data: {
        id: "wg-h2-tc",
        result: JSON.stringify({
          item: workGraphItem({ id: "item-h1", title: "Restored goal", status: "completed", revision: 2 }),
        }),
      },
    },
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1, "the card survives a reload-shaped replay");
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.items.length, 1);
  assert.equal(card.items[0].status, "completed", "result payloads without a name still fold");
  assert.equal(card.items[0].revision, 2);
  assert.equal(card.status, "completed");

  // The same resolution excludes the frames from generic rows.
  const toolBlocks = entries.flatMap((entry) => (
    entry.kind === "message" && Array.isArray(entry.blocks)
      ? entry.blocks.filter((block) => block.type === "tool-call")
      : []
  ));
  assert.deepEqual(toolBlocks, [], "no generic rows for workgraph activity after reload");
});

test("workgraph failed calls fold a failure note into recent events and flag the card", () => {
  const failedClaim = [
    {
      id: "wg-f1-call",
      event: "tool_call_requested",
      interactionId: "turn-wg",
      timestampMs: 1_779_405_465_000,
      data: { id: "wg-f1-tc", name: "workgraph_claim", args: { id: "item-a", expected_revision: 9 } },
    },
    {
      id: "wg-f1-done",
      event: "tool_execution_completed",
      interactionId: "turn-wg",
      timestampMs: 1_779_405_465_200,
      data: {
        id: "wg-f1-tc",
        name: "workgraph_claim",
        is_error: true,
        result: "revision conflict: expected 9, found 1, and this overlong upstream detail keeps going past the truncation budget",
      },
    },
  ];
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-ok",
      name: "workgraph_create",
      callArgs: { title: "Item A" },
      result: { item: workGraphItem({ id: "item-a", title: "Item A", status: "open", revision: 1 }) },
    }),
    ...failedClaim,
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.lastActionFailed, true, "latest action failed → card carries the flag");
  const failureLines = (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_claim failed:"));
  assert.equal(failureLines.length, 1, `expected a failure note, got ${JSON.stringify(card.recentEvents)}`);
  assert.ok(failureLines[0].length <= "✗ workgraph_claim failed: ".length + 80, "message truncates to ~80 chars");
  assert.match(failureLines[0], /revision conflict/);
  // Failures still never resurrect generic tool rows.
  const toolBlockNames = entries.flatMap((entry) => (
    entry.kind === "message" && Array.isArray(entry.blocks)
      ? entry.blocks.filter((block) => block.type === "tool-call").map((block) => block.name)
      : []
  ));
  assert.deepEqual(toolBlockNames.filter((name) => String(name).startsWith("workgraph_")), []);

  // A subsequent successful call clears the failed flag.
  const recovered = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [
    ...frames,
    ...workGraphToolFrames({
      idPrefix: "wg-f2",
      name: "workgraph_claim",
      callArgs: { id: "item-a", expected_revision: 1 },
      result: { item: workGraphItem({ id: "item-a", title: "Item A", status: "in_progress", revision: 2 }) },
      timestampMs: 1_779_405_466_000,
    }),
  ]).filter((entry) => entry.kind === "workgraph");
  assert.equal(recovered.length, 1);
  assert.equal(recovered[0].kind === "workgraph" ? recovered[0].lastActionFailed : true, undefined);
});

test("workgraph root items missing from the fold render a friendly title, never the bare ULID", () => {
  // The parent goal was created in an earlier conversation: only the child
  // and the parent edge are visible in this session's frames.
  const rootUlid = "01JZX3B7Q0R9TKVMH2E8S4WGXA";
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-c1",
      name: "workgraph_create",
      callArgs: { title: "Child task" },
      result: { item: workGraphItem({ id: "child-1", title: "Child task", status: "open", revision: 1 }) },
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-l1",
      name: "workgraph_link",
      callArgs: { kind: "parent", from_id: "child-1", to_id: rootUlid },
      result: { edge: { kind: "parent", from_id: "child-1", to_id: rootUlid } },
      timestampMs: 1_779_405_465_000,
    }),
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.rootId, rootUlid);
  assert.equal(card.title, "Goal from an earlier conversation");
  assert.notEqual(card.title, rootUlid);
  assert.ok(
    (card.objective || "").includes(rootUlid.slice(-6)),
    `short id lands in the objective slot: ${card.objective}`,
  );
  assert.deepEqual(card.items.map((item) => item.itemId), ["child-1"]);
});

test("workgraph operator-result frames fold like tool results so consecutive card actions use fresh revisions", () => {
  const base = workGraphToolFrames({
    idPrefix: "wg-op-0",
    name: "workgraph_create",
    callArgs: { title: "Operator target" },
    result: { item: workGraphItem({ id: "item-op", title: "Operator target", status: "open", revision: 1 }) },
  });
  const claimEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/claim",
    params: { id: "item-op", expected_revision: 1, owner: { kind: "principal", id: "ops@example.com" } },
    result: {
      item: workGraphItem({
        id: "item-op",
        title: "Operator target",
        status: "in_progress",
        revision: 2,
        owner: { kind: "principal", id: "ops@example.com", display_name: "Ops" },
      }),
    },
    identity: "planner",
    timestampMs: 1_779_405_465_000,
    frameId: "local-wg-claim",
  });

  const afterClaim = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, claimEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(afterClaim.length, 1);
  const claimed = afterClaim[0];
  if (claimed.kind !== "workgraph") return;
  assert.equal(claimed.items[0].status, "in_progress", "operator result updates status");
  assert.equal(claimed.items[0].revision, 2, "operator result carries the fresh CAS token");
  assert.equal(claimed.lastActionFailed, undefined);

  // Consecutive action: the close the card would now send uses revision 2,
  // and its echoed result advances the card again without any refresh.
  const closeEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/close",
    params: { id: "item-op", expected_revision: 2 },
    result: {
      item: workGraphItem({ id: "item-op", title: "Operator target", status: "completed", revision: 3 }),
    },
    identity: "planner",
    timestampMs: 1_779_405_466_000,
    frameId: "local-wg-close",
  });
  const afterClose = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, claimEcho, closeEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(afterClose.length, 1);
  const closed = afterClose[0];
  if (closed.kind !== "workgraph") return;
  assert.equal(closed.id, claimed.id, "card id stays stable across operator echoes");
  assert.equal(closed.items[0].status, "completed");
  assert.equal(closed.items[0].revision, 3);

  // A stale echo (out-of-order delivery) never rolls the revision back.
  const staleEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/claim",
    params: { id: "item-op", expected_revision: 1 },
    result: {
      item: workGraphItem({ id: "item-op", title: "Operator target", status: "open", revision: 1 }),
    },
    identity: "planner",
    timestampMs: 1_779_405_467_000,
    frameId: "local-wg-stale",
  });
  const afterStale = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, claimEcho, closeEcho, staleEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(afterStale[0].kind === "workgraph" ? afterStale[0].items[0].revision : 0, 3);

  // Operator frames never leak generic tool rows.
  const toolBlocks = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, claimEcho, closeEcho])
    .flatMap((entry) => (
      entry.kind === "message" && Array.isArray(entry.blocks)
        ? entry.blocks.filter((block) => block.type === "tool-call")
        : []
    ));
  assert.deepEqual(toolBlocks, []);
});

test("workgraph operator failures fold a failure note and flag through the same synthetic-frame path", () => {
  const base = workGraphToolFrames({
    idPrefix: "wg-opf-0",
    name: "workgraph_create",
    callArgs: { title: "Operator target" },
    result: { item: workGraphItem({ id: "item-op", title: "Operator target", status: "open", revision: 1 }) },
  });
  const failedEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/claim",
    params: { id: "item-op", expected_revision: 9 },
    errorMessage: "workgraph conflict: stale revision 9, item is at 1",
    identity: "planner",
    timestampMs: 1_779_405_465_000,
    frameId: "local-wg-fail",
  });

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, failedEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.lastActionFailed, true);
  assert.equal(card.items[0].revision, 1, "a failed mutation never moves the revision");
  const failureLines = (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_claim failed:"));
  assert.equal(failureLines.length, 1, `expected a failure note, got ${JSON.stringify(card.recentEvents)}`);
  assert.match(failureLines[0], /stale revision/);
});

test("workgraph unroutable failures land on the interaction's catch-all card instead of vanishing", () => {
  // A claim against an id the fold never observed: previously no card, no
  // note, no generic row — completely invisible.
  const frames = [
    {
      id: "wg-ghost-call",
      event: "tool_call_requested",
      interactionId: "turn-ghost",
      timestampMs: 1_779_405_464_000,
      data: { id: "wg-ghost-tc", name: "workgraph_claim", args: { id: "item-ghost", expected_revision: 4 } },
    },
    {
      id: "wg-ghost-done",
      event: "tool_execution_completed",
      interactionId: "turn-ghost",
      timestampMs: 1_779_405_464_200,
      data: { id: "wg-ghost-tc", name: "workgraph_claim", is_error: true, result: "work item item-ghost not found" },
    },
  ];

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1, "the failure materializes a catch-all card");
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.id, "workgraph:interaction:turn-ghost");
  assert.equal(card.title, "WorkGraph activity");
  assert.deepEqual(card.items, []);
  assert.equal(card.lastActionFailed, true);
  const failureLines = (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_claim failed:"));
  assert.equal(failureLines.length, 1, `expected the failure note, got ${JSON.stringify(card.recentEvents)}`);
  assert.match(failureLines[0], /not found/);
});

test("workgraph unroutable failures join an existing catch-all card for the same interaction", () => {
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-mix-0",
      name: "workgraph_create",
      callArgs: { title: "Known item" },
      result: { item: workGraphItem({ id: "item-known", title: "Known item", status: "open", revision: 1 }) },
    }),
    {
      id: "wg-mix-call",
      event: "tool_call_requested",
      interactionId: "turn-wg",
      timestampMs: 1_779_405_465_000,
      data: { id: "wg-mix-tc", name: "workgraph_close", args: { id: "item-ghost", expected_revision: 2 } },
    },
    {
      id: "wg-mix-done",
      event: "tool_execution_completed",
      interactionId: "turn-wg",
      timestampMs: 1_779_405_465_200,
      data: { id: "wg-mix-tc", name: "workgraph_close", is_error: true, result: "work item item-ghost not found" },
    },
  ];

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1, "the unroutable failure folds into the interaction's existing card");
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.id, "workgraph:interaction:turn-wg");
  assert.deepEqual(card.items.map((item) => item.itemId), ["item-known"]);
  assert.equal(card.lastActionFailed, true);
  assert.equal(
    (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_close failed:")).length,
    1,
  );
});

test("workgraph unroutable operator failures fold to the unscoped catch-all card", () => {
  const failedEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/attention/pause",
    params: { binding_id: "b-ghost", expected_revision: 3 },
    errorMessage: "attention binding b-ghost not found",
    timestampMs: 1_779_405_464_000,
    frameId: "local-wg-ghost",
  });
  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [failedEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.id, "workgraph:interaction:unscoped");
  assert.equal(card.title, "WorkGraph activity");
  assert.equal(card.lastActionFailed, true);
  assert.equal(
    (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_attention_pause failed:")).length,
    1,
  );
});

test("workgraph absent revisions never overwrite known ones (items and bindings)", () => {
  const itemWithoutRevision = workGraphItem({ id: "item-r", title: "Rev target", status: "open" }) as Record<string, unknown>;
  delete itemWithoutRevision.revision;
  const binding = (machineState: Record<string, unknown> | undefined) => ({
    binding_id: "b-1",
    mode: "pursue",
    status: { state: "active" },
    target: { session_id: "sess-1" },
    work_ref: { item_id: "item-r" },
    ...(machineState ? { machine_state: machineState } : {}),
    updated_at: "2026-07-08T09:00:00Z",
  });
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-r1",
      name: "workgraph_update",
      callArgs: { id: "item-r", expected_revision: 2 },
      result: {
        item: workGraphItem({ id: "item-r", title: "Rev target", status: "in_progress", revision: 3 }),
        attention: binding({ revision: 5 }),
      },
    }),
    // A later, non-authoritative echo: no item revision, no binding machine
    // state. It must not displace the known revisions or regress status.
    ...workGraphToolFrames({
      idPrefix: "wg-r2",
      name: "workgraph_get",
      callArgs: { id: "item-r" },
      result: { item: itemWithoutRevision, attention: binding(undefined) },
      timestampMs: 1_779_405_465_000,
    }),
  ];

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.items[0].revision, 3, "absent item revision never overwrites a present one");
  assert.equal(card.items[0].status, "in_progress", "the revision-less sighting is non-authoritative");
  assert.equal(card.attention.length, 1);
  assert.equal(card.attention[0].revision, 5, "absent binding revision never overwrites a present one");
});

test("workgraph card uiStateKey stays anchored across the catch-all→rooted id migration", () => {
  const looseFrames = workGraphToolFrames({
    idPrefix: "wg-mig-0",
    name: "workgraph_create",
    callArgs: { title: "Migrating goal" },
    result: { item: workGraphItem({ id: "item-mig", title: "Migrating goal", status: "open", revision: 1 }) },
  });
  const beforeCards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, looseFrames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(beforeCards.length, 1);
  const before = beforeCards[0];
  if (before.kind !== "workgraph") return;
  assert.equal(before.id, "workgraph:interaction:turn-wg");
  assert.equal(before.uiStateKey, "workgraph:interaction:turn-wg:item-mig");

  const hierarchyFrames = [
    ...looseFrames,
    ...workGraphToolFrames({
      idPrefix: "wg-mig-1",
      name: "workgraph_create",
      callArgs: { title: "Child task" },
      result: { item: workGraphItem({ id: "item-mig-child", title: "Child task", status: "open", revision: 1 }) },
      timestampMs: 1_779_405_465_000,
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-mig-2",
      name: "workgraph_link",
      callArgs: { kind: "parent", from_id: "item-mig-child", to_id: "item-mig" },
      result: { edge: { kind: "parent", from_id: "item-mig-child", to_id: "item-mig" } },
      timestampMs: 1_779_405_466_000,
    }),
  ];
  const afterCards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, hierarchyFrames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(afterCards.length, 1);
  const after = afterCards[0];
  if (after.kind !== "workgraph") return;
  assert.equal(after.id, "workgraph:item-mig", "hierarchy promotes the entry to its rooted id");
  assert.equal(
    after.uiStateKey,
    "workgraph:interaction:turn-wg:item-mig",
    "the UI-state anchor survives the rekey so expansion state can be carried forward",
  );
});

test("workgraph cards born in one interaction get distinct per-card uiStateKeys", () => {
  // Two independent goals, each with a child hierarchy, created in the same
  // interaction: collapse state must not bleed between their cards.
  const goal = (goalId: string, childId: string, baseTs: number) => [
    ...workGraphToolFrames({
      idPrefix: `wg-${goalId}`,
      name: "workgraph_create",
      callArgs: { title: `Goal ${goalId}` },
      result: { item: workGraphItem({ id: goalId, title: `Goal ${goalId}`, status: "open", revision: 1 }) },
      timestampMs: baseTs,
    }),
    ...workGraphToolFrames({
      idPrefix: `wg-${childId}`,
      name: "workgraph_create",
      callArgs: { title: `Child of ${goalId}` },
      result: { item: workGraphItem({ id: childId, title: `Child of ${goalId}`, status: "open", revision: 1 }) },
      timestampMs: baseTs + 1_000,
    }),
    ...workGraphToolFrames({
      idPrefix: `wg-${goalId}-link`,
      name: "workgraph_link",
      callArgs: { kind: "parent", from_id: childId, to_id: goalId },
      result: { edge: { kind: "parent", from_id: childId, to_id: goalId } },
      timestampMs: baseTs + 2_000,
    }),
  ];
  const frames = [
    ...goal("goal-a", "child-a", 1_779_405_464_000),
    ...goal("goal-b", "child-b", 1_779_405_470_000),
  ];

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 2);
  const keys = cards.map((card) => (card.kind === "workgraph" ? card.uiStateKey : undefined));
  assert.equal(keys[0], "workgraph:interaction:turn-wg:goal-a");
  assert.equal(keys[1], "workgraph:interaction:turn-wg:goal-b");
  assert.notEqual(keys[0], keys[1], "collapse state must not bleed between same-interaction cards");
});

test("workgraph item-less catch-all cards anchor UI state on the unrooted placeholder", () => {
  // An unroutable failure materializes a catch-all with no items: it still
  // needs a deterministic anchor distinct from item-bearing cards.
  const frames = [
    {
      id: "wg-ui-ghost-call",
      event: "tool_call_requested",
      interactionId: "turn-ghost",
      timestampMs: 1_779_405_464_000,
      data: { id: "wg-ui-ghost-tc", name: "workgraph_claim", args: { id: "item-ghost", expected_revision: 4 } },
    },
    {
      id: "wg-ui-ghost-done",
      event: "tool_execution_completed",
      interactionId: "turn-ghost",
      timestampMs: 1_779_405_464_200,
      data: { id: "wg-ui-ghost-tc", name: "workgraph_claim", is_error: true, result: "work item item-ghost not found" },
    },
  ];
  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  assert.equal(
    cards[0].kind === "workgraph" ? cards[0].uiStateKey : "",
    "workgraph:interaction:turn-ghost:unrooted",
  );

  // Operator-echo-only activity (no interaction) keys per target id instead
  // of one shared "unscoped" anchor.
  const echo = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/claim",
    params: { id: "item-echo", expected_revision: 1 },
    result: { item: workGraphItem({ id: "item-echo", title: "Echo target", status: "in_progress", revision: 2 }) },
    timestampMs: 1_779_405_465_000,
    frameId: "local-wg-ui-echo",
  });
  const echoCards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [echo])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(echoCards.length, 1);
  assert.equal(
    echoCards[0].kind === "workgraph" ? echoCards[0].uiStateKey : "",
    "workgraph:interaction:unscoped:item-echo",
  );
});

test("workgraph post-conflict refresh echoes heal the folded revision without clearing the failure flag", () => {
  const base = workGraphToolFrames({
    idPrefix: "wg-heal-0",
    name: "workgraph_create",
    callArgs: { title: "Wedged target" },
    result: { item: workGraphItem({ id: "item-heal", title: "Wedged target", status: "open", revision: 1 }) },
  });
  // The inline card sent its folded revision 1, but the item moved to 5
  // underneath: the mutation conflicts and echoes only the error…
  const conflictEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/claim",
    params: { id: "item-heal", expected_revision: 1, owner: { kind: "principal", id: "ops@example.com" } },
    errorMessage: "workgraph conflict: stale revision 1, item is at 5",
    identity: "planner",
    timestampMs: 1_779_405_465_000,
    frameId: "local-wg-heal-fail",
  });
  // …then the console re-reads the entity and folds the live state through
  // the same echo path, marked `refresh`.
  const refreshEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/get",
    params: { id: "item-heal" },
    result: { item: workGraphItem({ id: "item-heal", title: "Wedged target", status: "in_progress", revision: 5 }) },
    identity: "planner",
    timestampMs: 1_779_405_465_100,
    frameId: "local-wg-heal-refresh",
    refresh: true,
  });

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, conflictEcho, refreshEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.items[0].revision, 5, "the refresh echo heals the CAS token the next action will send");
  assert.equal(card.items[0].status, "in_progress", "the refresh echo carries the live status too");
  assert.equal(card.lastActionFailed, true, "a refresh is a read, not an action: the failure flag stays");
  assert.equal(
    (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_claim failed:")).length,
    1,
    "the conflict note stays visible alongside the healed state",
  );

  // The same refresh for a binding conflict: goal/status carries the goal
  // item AND the binding machine state, healing both CAS classes.
  const bindingBase = workGraphToolFrames({
    idPrefix: "wg-heal-b0",
    name: "workgraph_get",
    callArgs: { id: "goal-heal" },
    result: {
      item: workGraphItem({ id: "goal-heal", title: "Bound goal", status: "in_progress", revision: 2 }),
      attention: {
        binding_id: "b-heal",
        mode: "pursue",
        status: { state: "active" },
        target: { session_id: "sess-1" },
        work_ref: { item_id: "goal-heal" },
        machine_state: { revision: 3 },
        updated_at: "2026-07-08T09:00:00Z",
      },
    },
  });
  const bindingConflict = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/attention/pause",
    params: { binding_id: "b-heal", expected_revision: 3 },
    errorMessage: "workgraph conflict: stale machine revision 3, binding is at 8",
    identity: "planner",
    timestampMs: 1_779_405_466_000,
    frameId: "local-wg-heal-bfail",
  });
  const bindingRefresh = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/goal/status",
    params: { binding_id: "b-heal" },
    result: {
      item: workGraphItem({ id: "goal-heal", title: "Bound goal", status: "in_progress", revision: 6 }),
      attention: {
        binding_id: "b-heal",
        mode: "pursue",
        status: { state: "active" },
        target: { session_id: "sess-1" },
        work_ref: { item_id: "goal-heal" },
        machine_state: { revision: 8 },
        updated_at: "2026-07-08T09:05:00Z",
      },
    },
    identity: "planner",
    timestampMs: 1_779_405_466_100,
    frameId: "local-wg-heal-brefresh",
    refresh: true,
  });
  const bindingCards = mapFramesToTimelineEntries(
    WORKGRAPH_AGENT,
    [...bindingBase, bindingConflict, bindingRefresh],
  ).filter((entry) => entry.kind === "workgraph");
  assert.equal(bindingCards.length, 1);
  const bindingCard = bindingCards[0];
  if (bindingCard.kind !== "workgraph") return;
  assert.equal(bindingCard.attention[0].revision, 8, "binding machine CAS token heals");
  assert.equal(bindingCard.items[0].revision, 6, "the bound goal item CAS token heals in the same read");
  assert.equal(bindingCard.lastActionFailed, true);
});

test("workgraph second parent edges never re-home the subtree and surface as an also-under note", () => {
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-mp-a",
      name: "workgraph_create",
      callArgs: { title: "Goal A" },
      result: { item: workGraphItem({ id: "goal-a", title: "Goal A", status: "open", revision: 1, createdAt: "2026-07-08T08:00:00Z" }) },
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-mp-b",
      name: "workgraph_create",
      callArgs: { title: "Goal B" },
      result: { item: workGraphItem({ id: "goal-b", title: "Goal B", status: "open", revision: 1, createdAt: "2026-07-08T08:01:00Z" }) },
      timestampMs: 1_779_405_465_000,
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-mp-c",
      name: "workgraph_create",
      callArgs: { title: "Shared child" },
      result: { item: workGraphItem({ id: "child-shared", title: "Shared child", status: "open", revision: 1, createdAt: "2026-07-08T08:02:00Z" }) },
      timestampMs: 1_779_405_466_000,
    }),
    ...workGraphToolFrames({
      idPrefix: "wg-mp-l1",
      name: "workgraph_link",
      callArgs: { kind: "parent", from_id: "child-shared", to_id: "goal-a" },
      result: { edge: { kind: "parent", from_id: "child-shared", to_id: "goal-a" } },
      timestampMs: 1_779_405_467_000,
    }),
  ];
  const secondParent = workGraphToolFrames({
    idPrefix: "wg-mp-l2",
    name: "workgraph_link",
    callArgs: { kind: "parent", from_id: "child-shared", to_id: "goal-b" },
    result: { edge: { kind: "parent", from_id: "child-shared", to_id: "goal-b" } },
    timestampMs: 1_779_405_468_000,
  });

  const before = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  const after = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...frames, ...secondParent])
    .filter((entry) => entry.kind === "workgraph");

  const cardFor = (cards: typeof after, rootId: string) => {
    const card = cards.find((entry) => entry.kind === "workgraph" && entry.rootId === rootId);
    assert.ok(card && card.kind === "workgraph", `expected a card rooted at ${rootId}`);
    return card;
  };

  // The child stays homed under its first parent…
  const beforeA = cardFor(before, "goal-a");
  assert.deepEqual(beforeA.items.map((item) => item.itemId), ["goal-a", "child-shared"]);
  const afterA = cardFor(after, "goal-a");
  assert.deepEqual(
    afterA.items.map((item) => item.itemId),
    ["goal-a", "child-shared"],
    "the second parent edge must not move the subtree",
  );
  assert.equal(afterA.id, beforeA.id, "goal A's card id stays stable");
  const child = afterA.items.find((item) => item.itemId === "child-shared");
  assert.ok(child);
  assert.equal(child.parentId, "goal-a");
  assert.deepEqual(child.alsoUnder, ["Goal B"], "the additional parent is listed by title in the row detail");

  // …and goal B's card does not absorb it.
  const afterB = after.find((entry) => entry.kind === "workgraph" && entry.rootId === "goal-b");
  if (afterB && afterB.kind === "workgraph") {
    assert.deepEqual(
      afterB.items.map((item) => item.itemId).filter((id) => id === "child-shared"),
      [],
    );
  }

  // Duplicate re-delivery of the first edge never demotes it to an extra.
  const redelivered = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [
    ...frames,
    ...secondParent,
    ...workGraphToolFrames({
      idPrefix: "wg-mp-l1-dup",
      name: "workgraph_link",
      callArgs: { kind: "parent", from_id: "child-shared", to_id: "goal-a" },
      result: { edge: { kind: "parent", from_id: "child-shared", to_id: "goal-a" } },
      timestampMs: 1_779_405_469_000,
    }),
  ]).filter((entry) => entry.kind === "workgraph");
  const redeliveredChild = cardFor(redelivered, "goal-a").items.find((item) => item.itemId === "child-shared");
  assert.ok(redeliveredChild);
  assert.equal(redeliveredChild.parentId, "goal-a");
  assert.deepEqual(redeliveredChild.alsoUnder, ["Goal B"]);
});

test("workgraph events dedupe by seq (content for seq-less echoes) so overlapping polls add nothing", () => {
  const event = (seq: number | undefined, kind: string, at: string) => ({
    ...(seq !== undefined ? { seq } : {}),
    realm_id: "realm-1",
    namespace: "default",
    item_id: "item-ev",
    kind,
    at,
  });
  const base = workGraphToolFrames({
    idPrefix: "wg-ev-0",
    name: "workgraph_create",
    callArgs: { title: "Event target" },
    result: { item: workGraphItem({ id: "item-ev", title: "Event target", status: "open", revision: 1 }) },
  });
  const firstPoll = workGraphToolFrames({
    idPrefix: "wg-ev-1",
    name: "workgraph_events",
    callArgs: { limit: 10 },
    result: { events: [event(1, "created", "2026-07-08T09:00:00Z"), event(2, "claimed", "2026-07-08T09:01:00Z")] },
    timestampMs: 1_779_405_465_000,
  });
  // The second poll window overlaps the first entirely and adds one new event.
  const secondPoll = workGraphToolFrames({
    idPrefix: "wg-ev-2",
    name: "workgraph_events",
    callArgs: { limit: 10 },
    result: {
      events: [
        event(1, "created", "2026-07-08T09:00:00Z"),
        event(2, "claimed", "2026-07-08T09:01:00Z"),
        event(3, "updated", "2026-07-08T09:02:00Z"),
      ],
    },
    timestampMs: 1_779_405_466_000,
  });

  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, ...firstPoll, ...secondPoll]);
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.deepEqual(workGraphFallbackToolBlocks(entries), [], "represented event polls must not duplicate their card as generic tools");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.deepEqual(
    card.recentEvents,
    ["created", "claimed", "updated"].map((kind, index) => `${kind} · ${new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(`2026-07-08T09:0${index}:00Z`))}`),
    "replayed seq'd events fold exactly once",
  );

  // Seq-less events (local echoes) dedupe by content; distinct content still
  // folds.
  const echoPollOnce = workGraphToolFrames({
    idPrefix: "wg-ev-3",
    name: "workgraph_events",
    callArgs: { limit: 10 },
    result: { events: [event(undefined, "evidence added", "2026-07-08T09:03:00Z")] },
    timestampMs: 1_779_405_467_000,
  });
  const echoPollAgain = workGraphToolFrames({
    idPrefix: "wg-ev-4",
    name: "workgraph_events",
    callArgs: { limit: 10 },
    result: {
      events: [
        event(undefined, "evidence added", "2026-07-08T09:03:00Z"),
        event(undefined, "closed", "2026-07-08T09:04:00Z"),
      ],
    },
    timestampMs: 1_779_405_468_000,
  });
  const echoEntries = mapFramesToTimelineEntries(
    WORKGRAPH_AGENT,
    [...base, ...firstPoll, ...secondPoll, ...echoPollOnce, ...echoPollAgain],
  );
  const echoCards = echoEntries.filter((entry) => entry.kind === "workgraph");
  assert.deepEqual(workGraphFallbackToolBlocks(echoEntries), [], "replayed event calls remain represented by the same card");
  const echoCard = echoCards[0];
  if (echoCard.kind !== "workgraph") return;
  assert.deepEqual(
    echoCard.recentEvents,
    ["created", "claimed", "updated", "evidence added", "closed"].map((kind, index) => `${kind} · ${new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(`2026-07-08T09:0${index}:00Z`))}`),
    "seq-less duplicates dedupe by content; the 5-slot window holds five distinct events",
  );
});

for (const [scenario, events] of [
  ["empty", []],
  ["unknown item", [{ seq: 101, item_id: "unseen-item", kind: "created", at: "2026-07-08T09:00:00Z" }]],
  ["partly unrendered", [
    { seq: 101, item_id: "known-item", kind: "claimed", at: "2026-07-08T09:00:00Z" },
    { seq: 102, item_id: "unseen-item", kind: "created", at: "2026-07-08T09:01:00Z" },
  ]],
] as const) {
  test(`workgraph event fallback preserves ${scenario} results beside an unrelated card`, () => {
    const base = workGraphToolFrames({ idPrefix: "event-fallback-base", name: "workgraph_create", callArgs: {}, result: { item: workGraphItem({ id: "known-item" }) } });
    const result = { events };
    const poll = workGraphToolFrames({ idPrefix: "event-fallback-poll", name: "workgraph_events", callArgs: { limit: 10 }, result, timestampMs: 1_779_405_465_000 });
    const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, ...poll]);
    assert.equal(entries.filter((entry) => entry.kind === "workgraph").length, 1, "events must not synthesize an unknown item card");
    const blocks = workGraphFallbackToolBlocks(entries);
    assert.deepEqual(blocks.map((block) => block.toolCallId), ["event-fallback-poll-tc"]);
    assert.equal(blocks[0].result, JSON.stringify(result));
  });
}

// Big-store snapshot fixture: 3 attention-bound goals (36 + 3 + 3 items in
// their trees) plus 58 loose items — 100 items total in one interaction.
function bigSnapshotFrames(): Array<Record<string, unknown>> {
  const pad = (value: number) => String(value).padStart(2, "0");
  const items: Array<Record<string, unknown>> = [];
  const edges: Array<Record<string, unknown>> = [];
  const attention: Array<Record<string, unknown>> = [];
  for (let goal = 1; goal <= 3; goal++) {
    items.push(workGraphItem({
      id: `goal-${goal}`,
      title: `Goal ${goal}`,
      status: "in_progress",
      revision: 1,
      createdAt: `2026-07-08T07:0${goal}:00Z`,
    }));
    attention.push({
      binding_id: `b-${goal}`,
      work_ref: { realm_id: "realm-1", namespace: "default", item_id: `goal-${goal}` },
      target: { kind: "session", session_id: `sess-${goal}` },
      mode: "pursue",
      status: { state: "active" },
      machine_state: { revision: goal },
    });
    const childCount = goal === 1 ? 35 : 2;
    for (let child = 1; child <= childCount; child++) {
      const id = `g${goal}-c${pad(child)}`;
      items.push(workGraphItem({
        id,
        title: `Goal ${goal} child ${pad(child)}`,
        // Goal 1 children 25..35 are completed — several of them fall past
        // the row cap, so progress must count what the cap hides.
        status: goal === 1 && child >= 25 ? "completed" : "open",
        revision: 1,
        createdAt: `2026-07-08T08:${pad(child)}:00Z`,
      }));
      edges.push({ kind: "parent", from_id: id, to_id: `goal-${goal}` });
    }
  }
  for (let loose = 1; loose <= 58; loose++) {
    items.push(workGraphItem({
      id: `loose-${pad(loose)}`,
      title: `Loose item ${pad(loose)}`,
      status: loose >= 50 ? "completed" : "open",
      revision: 1,
      createdAt: `2026-07-08T09:${pad(loose)}:00Z`,
    }));
  }
  return workGraphToolFrames({
    idPrefix: "wg-big",
    name: "workgraph_snapshot",
    callArgs: {},
    result: { snapshot: { realm_id: "realm-1", items, edges, attention } },
    interactionId: "turn-big",
  });
}

test("workgraph full-store snapshots mint goal cards only; loose items share one bounded catch-all with overflow accounting", () => {
  const entries = mapFramesToTimelineEntries(WORKGRAPH_AGENT, bigSnapshotFrames());
  const cards = entries.filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 4, "3 attention-bound goal cards + 1 catch-all, never a card per loose root");
  const byId = new Map(cards.map((card) => [card.id, card]));

  const goal1 = byId.get("workgraph:goal-1");
  assert.ok(goal1 && goal1.kind === "workgraph");
  assert.deepEqual(goal1.progress, { completed: 11, total: 36 }, "progress counts the full tree");
  assert.equal(goal1.items.length, WORKGRAPH_CARD_ITEM_ROW_LIMIT, "rows cap at the render limit");
  assert.equal(goal1.itemOverflowCount, 6, "hidden rows surface as overflow, not silence");
  assert.equal(
    goal1.items.filter((item) => item.status === "completed").length,
    5,
    "part of the completed set is hidden by the cap yet still counted in progress",
  );

  for (const rootId of ["goal-2", "goal-3"]) {
    const card = byId.get(`workgraph:${rootId}`);
    assert.ok(card && card.kind === "workgraph", `goal card for ${rootId}`);
    assert.deepEqual(card.progress, { completed: 0, total: 3 });
    assert.equal(card.itemOverflowCount, undefined, "small trees need no overflow row");
    assert.equal(card.attention.length, 1);
  }

  const catchAll = byId.get("workgraph:interaction:turn-big");
  assert.ok(catchAll && catchAll.kind === "workgraph", "loose items share ONE catch-all card");
  assert.equal(catchAll.title, "Work items");
  assert.deepEqual(catchAll.progress, { completed: 9, total: 58 }, "catch-all progress counts every loose item");
  assert.equal(catchAll.items.length, WORKGRAPH_CARD_ITEM_ROW_LIMIT);
  assert.equal(catchAll.itemOverflowCount, 28);
});

test("workgraph row caps keep the most recently active rows visible", () => {
  const frames = [
    ...bigSnapshotFrames(),
    // A later targeted update touches a loose item deep past the visible
    // window: recency must pull it into the visible rows.
    ...workGraphToolFrames({
      idPrefix: "wg-big-touch",
      name: "workgraph_claim",
      callArgs: { id: "loose-45", expected_revision: 1 },
      result: { item: workGraphItem({ id: "loose-45", title: "Loose item 45", status: "in_progress", revision: 2 }) },
      interactionId: "turn-big",
      timestampMs: 1_779_405_470_000,
    }),
  ];
  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  const catchAll = cards.find((card) => card.kind === "workgraph" && card.id === "workgraph:interaction:turn-big");
  assert.ok(catchAll && catchAll.kind === "workgraph");
  assert.equal(catchAll.items.length, WORKGRAPH_CARD_ITEM_ROW_LIMIT);
  const touched = catchAll.items.find((item) => item.itemId === "loose-45");
  assert.ok(touched, "the freshly touched row stays visible despite sitting past the cap in fold order");
  assert.equal(touched.revision, 2);
});

test("workgraph refolds with the same frames never re-JSON.parse tool results", () => {
  const frames = bigSnapshotFrames().map((frame) => ({ ...frame, id: `${frame.id}-memo` }));
  const originalParse = JSON.parse;
  let parseCalls = 0;
  JSON.parse = ((text: string, reviver?: Parameters<typeof JSON.parse>[1]) => {
    parseCalls += 1;
    return originalParse(text, reviver);
  }) as typeof JSON.parse;
  try {
    mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
    assert.ok(parseCalls > 0, "the first fold parses the snapshot payload");
    parseCalls = 0;
    mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames);
    assert.equal(parseCalls, 0, "re-renders reuse the memoized parse per (frame id, frameVersion)");
  } finally {
    JSON.parse = originalParse;
  }
});

test("workgraph failure notes dedupe by tool-call id across live and history twin frames", () => {
  const frames = [
    ...workGraphToolFrames({
      idPrefix: "wg-twin-0",
      name: "workgraph_create",
      callArgs: { title: "Twin target" },
      result: { item: workGraphItem({ id: "item-twin", title: "Twin target", status: "open", revision: 1 }) },
      interactionId: "turn-twin",
    }),
    {
      id: "wg-twin-call",
      event: "tool_call_requested",
      interactionId: "turn-twin",
      timestampMs: 1_779_405_465_000,
      data: { id: "wg-twin-tc", name: "workgraph_claim", args: { id: "item-twin", expected_revision: 9 } },
    },
    // The live result frame and its session-history backfill twin: distinct
    // frame ids, one tool call.
    {
      id: "wg-twin-live",
      event: "tool_result_received",
      interactionId: "turn-twin",
      timestampMs: 1_779_405_465_200,
      data: { id: "wg-twin-tc", name: "workgraph_claim", is_error: true, result: "revision conflict" },
    },
    {
      id: "wg-twin-history",
      event: "tool_execution_completed",
      interactionId: "turn-twin",
      timestampMs: 1_779_405_465_200,
      sourceKind: "session_history",
      data: { id: "wg-twin-tc", is_error: true, result: "revision conflict" },
    },
  ];

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.lastActionFailed, true);
  assert.equal(
    (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_claim failed:")).length,
    1,
    `twin frames of one failed call must fold ONE note, got ${JSON.stringify(card.recentEvents)}`,
  );
});

test("workgraph cards fold the result event and take failure detail from it, not the bodiless execution fact", () => {
  const created = { item: workGraphItem({ id: "item-split", title: "Split target", status: "open", revision: 1 }) };
  const frames = [
    {
      id: "wg-split-0-call",
      event: "tool_call_requested",
      interactionId: "turn-split",
      timestampMs: 1_779_405_464_000,
      data: { id: "wg-split-0-tc", name: "workgraph_create", args: { title: "Split target" } },
    },
    {
      id: "wg-split-0-completed",
      event: "tool_execution_completed",
      interactionId: "turn-split",
      timestampMs: 1_779_405_464_100,
      data: { id: "wg-split-0-tc", name: "workgraph_create", is_error: false, duration_ms: 4 },
    },
    {
      id: "wg-split-0-received",
      event: "tool_result_received",
      interactionId: "turn-split",
      timestampMs: 1_779_405_464_200,
      data: { id: "wg-split-0-tc", name: "workgraph_create", is_error: false, result: JSON.stringify(created) },
    },
    {
      id: "wg-split-call",
      event: "tool_call_requested",
      interactionId: "turn-split",
      timestampMs: 1_779_405_465_000,
      data: { id: "wg-split-tc", name: "workgraph_claim", args: { id: "item-split", expected_revision: 9 } },
    },
    // The execution fact comes first and carries no body; the result event
    // for the same call carries the failure detail.
    {
      id: "wg-split-completed",
      event: "tool_execution_completed",
      interactionId: "turn-split",
      timestampMs: 1_779_405_465_100,
      data: { id: "wg-split-tc", name: "workgraph_claim", is_error: true, duration_ms: 4 },
    },
    {
      id: "wg-split-received",
      event: "tool_result_received",
      interactionId: "turn-split",
      timestampMs: 1_779_405_465_200,
      data: { id: "wg-split-tc", name: "workgraph_claim", is_error: true, result: "revision conflict: expected 9, found 1" },
    },
  ];

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames)
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1);
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.match(JSON.stringify(card), /Split target/, "the created item is folded from the result event");
  assert.equal(card.lastActionFailed, true);
  const failureLines = (card.recentEvents || []).filter((line) => line.startsWith("✗ workgraph_claim failed"));
  assert.equal(failureLines.length, 1, `one note per failed call, got ${JSON.stringify(card.recentEvents)}`);
  assert.match(failureLines[0], /revision conflict/);
});

test("workgraph post-reload snapshot refresh echoes re-hydrate operator-driven state without minting cards", () => {
  // History restored after a page reload: the agent created the item at
  // revision 1; the operator's close ran pre-reload and its client-local
  // echo frame died with the page.
  const base = workGraphToolFrames({
    idPrefix: "wg-reload-0",
    name: "workgraph_create",
    callArgs: { title: "Reload target" },
    result: { item: workGraphItem({ id: "item-reload", title: "Reload target", status: "open", revision: 1 }) },
    interactionId: "turn-reload",
  });
  // The pane-mount hook fetches ONE snapshot and folds it as a refresh echo.
  const refreshEcho = buildWorkGraphOperatorResultFrame({
    method: "meta/workgraph/snapshot",
    params: {},
    result: {
      snapshot: {
        realm_id: "realm-1",
        items: [
          workGraphItem({ id: "item-reload", title: "Reload target", status: "completed", revision: 4 }),
          // Unrelated store items ride along in a full snapshot: a refresh
          // heals what the transcript shows but never introduces new work.
          workGraphItem({ id: "item-unrelated", title: "Someone else's item", status: "open", revision: 7 }),
        ],
        edges: [],
        attention: [],
      },
    },
    identity: "planner",
    timestampMs: 1_779_405_466_000,
    frameId: "local-wg-rehydrate",
    refresh: true,
  });

  const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, [...base, refreshEcho])
    .filter((entry) => entry.kind === "workgraph");
  assert.equal(cards.length, 1, "the refresh echo updates the existing card and mints nothing new");
  const card = cards[0];
  if (card.kind !== "workgraph") return;
  assert.equal(card.items[0].status, "completed", "the operator-driven status survives the reload");
  assert.equal(card.items[0].revision, 4, "the next action CASes against the live revision");
  assert.equal(card.lastActionFailed, undefined, "a refresh is a read: no outcome flags");
  assert.deepEqual(
    card.items.map((item) => item.itemId),
    ["item-reload"],
    "unrelated snapshot items never fold through a refresh echo",
  );
});

test("createWorkGraphHydrationGate leaves an identity unhydrated when the initial page has no cards", () => {
  const gate = createWorkGraphHydrationGate();
  // The recent-200 backfill page folds in first and shows no workgraph
  // cards. Regression: this used to permanently mark the identity done,
  // so a card folded in later via scroll-up would never re-hydrate.
  const fetched = gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: false });
  assert.equal(fetched, false, "no cards on this page means no fetch yet");
  // The identity must still be eligible: calling again with the same
  // (still cardless) page keeps returning false, not throwing or wedging.
  assert.equal(
    gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: false }),
    false,
    "still no cards: still no fetch",
  );
});

test("createWorkGraphHydrationGate fetches exactly once when an older-page fold first introduces cards", () => {
  const gate = createWorkGraphHydrationGate();
  // Initial backfill page: no cards yet, per the scenario above.
  assert.equal(gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: false }), false);
  // loadOlderIdentityTimeline folds an older page that does contain a card.
  assert.equal(
    gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: true }),
    true,
    "the first page with cards triggers the one-time snapshot fetch",
  );
  // A second older-page fold (still cards present) must not re-fetch.
  assert.equal(
    gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: true }),
    false,
    "already hydrated: a later fold does not re-fetch",
  );
});

test("createWorkGraphHydrationGate does not consume the identity while the workgraph experience is unavailable", () => {
  const gate = createWorkGraphHydrationGate();
  // Experience not loaded yet: skip without marking done, same as the
  // no-cards case, so the availability flip can still trigger a fetch.
  assert.equal(gate.shouldFetch("planner", { workgraphAvailable: false, hasCards: true }), false);
  assert.equal(
    gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: true }),
    true,
    "once available, the pending cards trigger the fetch",
  );
});

test("createWorkGraphHydrationGate tracks identities independently", () => {
  const gate = createWorkGraphHydrationGate();
  assert.equal(gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: true }), true);
  assert.equal(
    gate.shouldFetch("other-agent", { workgraphAvailable: true, hasCards: true }),
    true,
    "a different identity gets its own independent fetch",
  );
  assert.equal(gate.shouldFetch("planner", { workgraphAvailable: true, hasCards: true }), false);
});

// ---------------------------------------------------------------------------
// meerkat 0.7.25 ask 15: UUID interaction ids are authoritative twin identity
// ---------------------------------------------------------------------------

const UUID_A = "6fa459ea-ee8a-3ca4-894e-db77e160355e";
const UUID_B = "886313e1-3b8a-5372-9b90-0c9aee199e5d";

function entryVisibleTestText(entry: unknown): string {
  const record = entry as { text?: unknown; blocks?: unknown };
  if (typeof record.text === "string") return record.text;
  if (Array.isArray(record.blocks)) {
    return record.blocks
      .map((block) => (block as { type?: string }).type === "markdown"
        ? (block as { source: string }).source
        : typeof (block as { text?: unknown }).text === "string"
          ? (block as { text: string }).text
          : "")
      .join(" ");
  }
  return "";
}

test("history twin of a live terminal frame is dropped by interaction id even when texts drift", () => {
  const entries = mapFramesToTimelineEntries(
    { agent_id: "a", member_id: "a", label: "A", kind: "identity" },
    [
      {
        id: "live-terminal",
        event: "interaction_complete",
        interactionId: UUID_A,
        timestampMs: 1000,
        data: { text: "Done.", result: "Done." },
      },
      {
        id: "history-twin",
        event: "interaction_complete",
        sourceKind: "session_history",
        interactionId: UUID_A,
        timestampMs: 900_000,
        data: {
          // Persisted rendering can differ from the live text (whitespace,
          // scaffold stripping) — the id join must not depend on it.
          text: "Done.\n",
          result: "Done.\n",
          source_event_type: "session_history",
        },
      },
    ],
  );
  const doneEntries = entries.filter((e) => entryVisibleTestText(e).includes("Done."));
  assert.equal(doneEntries.length, 1, JSON.stringify(entries, null, 2));
});

test("identical repeated replies from DISTINCT interactions both render (over-cull fix)", () => {
  // The over-cull class: an agent legitimately answers two different
  // interactions with the same text. The content+time heuristic used to
  // collapse the second reply; differing UUID interaction ids now veto it.
  const entries = mapFramesToTimelineEntries(
    { agent_id: "a", member_id: "a", label: "A", kind: "identity" },
    [
      {
        id: "live-first",
        event: "interaction_complete",
        interactionId: UUID_A,
        timestampMs: 1000,
        data: { text: "ACK", result: "ACK" },
      },
      {
        id: "live-second",
        event: "interaction_complete",
        interactionId: UUID_B,
        timestampMs: 2000,
        data: { text: "ACK", result: "ACK" },
      },
    ],
  );
  const ackEntries = entries.filter((e) => entryVisibleTestText(e).includes("ACK"));
  assert.equal(ackEntries.length, 2, JSON.stringify(entries, null, 2));
});

test("legacy console-interaction ids keep the text heuristic (same interaction, mixed schemes)", () => {
  // Classic (non-identity-first) sends: the live frame carries a legacy
  // reservation id, the history frame carries meerkat's UUID — the SAME
  // interaction under two id schemes. Mixed schemes must fall back to the
  // text heuristic, not read the id mismatch as two interactions.
  const entries = mapFramesToTimelineEntries(
    { agent_id: "a", member_id: "a", label: "A", kind: "identity" },
    [
      {
        id: "live-terminal",
        event: "interaction_complete",
        interactionId: "console-interaction-abc123",
        timestampMs: 1000,
        data: { text: "Same reply.", result: "Same reply." },
      },
      {
        id: "history-twin",
        event: "interaction_complete",
        sourceKind: "session_history",
        interactionId: UUID_B,
        timestampMs: 1500,
        data: {
          text: "Same reply.",
          result: "Same reply.",
          source_event_type: "session_history",
        },
      },
    ],
  );
  const replyEntries = entries.filter((e) => entryVisibleTestText(e).includes("Same reply."));
  assert.equal(replyEntries.length, 1, JSON.stringify(entries, null, 2));
});

// Run lineage is the owner for startup turns, including runs with no interaction.
for (const [surface, mapper] of [
  ["stock", mapFramesToTimelineEntries],
  ["shared", mapFramesToTimelineEntriesShared],
] as const) {
  const agent = { agent_id: "router:main", member_id: "router:main", label: "Router", kind: "identity" };
  const runA = "01a0dae6-a27d-7720-baad-1288fc90471a";
  const runB = "01a0dae6-a27d-7720-baad-1288fc90471b";
  const history = (id: string, text: string, runId: string, interactionId?: string) => ({
    id, event: "interaction_complete", sourceKind: "session_history", runId, interactionId,
    data: { text, result: text, message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text } }] } },
  });
  const texts = (frames: Parameters<typeof mapper>[1]) => mapper(agent, frames, { textMode: "markdown" })
    .filter((entry) => entry.kind === "message" && entry.identity.role === "assistant");

  test(`${surface}: queued steer keeps the original streamed answer once across overlapping input`, () => {
    const first = { runId: runA, interactionId: UUID_A };
    const steer = { runId: runB, interactionId: UUID_B };
    const answer = "First segment. Second segment. Final segment.\n";
    const frames = [
      { id: "original-input", event: "user_input", interactionId: UUID_A, timestampMs: 1, data: { content: "Begin." } },
      { id: "original-start", event: "run_started", ...first, timestampMs: 2, data: {} },
      { id: "original-prefix", event: "text_delta", ...first, timestampMs: 3, data: { delta: "First segment. " } },
      { id: "steer-input", event: "user_input", interactionId: UUID_B, timestampMs: 4, data: { content: "Steer." } },
      { id: "original-middle", event: "text_delta", ...first, timestampMs: 5, data: { delta: "Second segment. " } },
      { ...history("original-history", answer, runA, UUID_A), timestampMs: 6 },
      { id: "original-tail", event: "text_delta", ...first, timestampMs: 7, data: { delta: "Final segment.\n" } },
      { id: "original-text-complete", event: "text_complete", ...first, timestampMs: 8, data: { content: answer } },
      { id: "original-complete", event: "run_completed", ...first, timestampMs: 9, data: { result: answer } },
      { id: "steer-start", event: "run_started", ...steer, timestampMs: 10, data: {} },
      { id: "steer-answer", event: "text_delta", ...steer, timestampMs: 11, data: { delta: "Acknowledged." } },
      { id: "steer-complete", event: "run_completed", ...steer, timestampMs: 12, data: { result: "Acknowledged." } },
      { ...history("steer-history", "Acknowledged.", runB, UUID_B), timestampMs: 13 },
    ];
    const entries = mapper(agent, frames, { textMode: "markdown", renderInteractionStartsAsUser: true });
    const original = entries.filter(entry => entry.kind === "message" && entry.identity.role === "assistant" && entry.runId === runA);
    assert.equal(original.map(entryVisibleTestText).join(""), answer);
    assert.deepEqual(original.map(entry => [entry.id, entry.createdAt, entry.interactionId]), [
      ["original-prefix", new Date(3).toISOString(), UUID_A],
      ["original-middle", new Date(5).toISOString(), UUID_A],
    ], "chronological segments retain their opening source identity across the steer input");
    const acknowledgement = entries.filter(entry => entry.kind === "message" && entry.runId === runB);
    assert.deepEqual(acknowledgement.map(entry => [entry.id, entry.interactionId, entryVisibleTestText(entry)]), [
      ["steer-answer", UUID_B, "Acknowledged."],
    ]);
    const historyBeforeTail = mapper(agent, frames.slice(0, 6), { textMode: "markdown", renderInteractionStartsAsUser: true });
    const visibleOriginal = historyBeforeTail.filter(entry => entry.kind === "message" && entry.identity.role === "assistant" && entry.runId === runA);
    assert.deepEqual(visibleOriginal.map(entry => [entry.id, entryVisibleTestText(entry)]), [
      ["original-prefix", "First segment. "],
      ["original-middle", "Second segment. Final segment.\n"],
    ], "history fills the open segment without moving later text ahead of the steer");
  });

  test(`${surface}: overlapping equal responses preserve both typed stream owners`, () => {
    const frames = [
      { id: "first-open", event: "text_delta", runId: runA, interactionId: UUID_A, data: { delta: "Ready. " } },
      { id: "second-open", event: "text_delta", runId: runB, interactionId: UUID_B, data: { delta: "Ready. " } },
      { id: "first-tail", event: "text_delta", runId: runA, interactionId: UUID_A, data: { delta: "Done." } },
      { id: "first-terminal", event: "run_completed", runId: runA, interactionId: UUID_A, data: { result: "Ready. Done." } },
      history("first-saved", "Ready. Done.", runA, UUID_A),
      { id: "second-tail", event: "text_delta", runId: runB, interactionId: UUID_B, data: { delta: "Done." } },
      { id: "second-terminal", event: "run_completed", runId: runB, interactionId: UUID_B, data: { result: "Ready. Done." } },
      history("second-saved", "Ready. Done.", runB, UUID_B),
    ];
    const entries = texts(frames);
    for (const runId of [runA, runB]) {
      const owned = entries.filter(entry => entry.kind === "message" && entry.runId === runId);
      assert.equal(owned.map(entryVisibleTestText).join(""), "Ready. Done.");
      assert.equal(owned.length, 2, "both authored stream segments remain in chronology");
    }
  });

  for (const sourceKind of [undefined, "session_history"]) {
    const transport = sourceKind || "console_event";
    test(`${surface}: ${transport} typed tool text preserves exact source and failure state`, () => {
      for (const [content, expected] of [
        [[{ type: "text", text: "  Release notes\nKeep both spaces and this final newline.\n" }], "  Release notes\nKeep both spaces and this final newline.\n"],
        [[{ type: "text", text: "alpha" }, { type: "text", text: "  \n" }, { type: "text", text: "beta\n" }], "alpha  \nbeta\n"],
        [[{ type: "text", text: "   \n" }], "   \n"],
        [[{ type: "text", text: "" }], ""],
      ] as const) {
        for (const isError of [false, true]) {
          const entries = mapper(agent, [
            { id: "file-call", event: "tool_call_requested", sourceKind, data: { id: "read-1", name: "read_file", args: { path: "notes.txt" } } },
            { id: "file-result", event: "tool_result_received", sourceKind, data: { tool_call_id: "read-1", name: "read_file", result: content, is_error: isError } },
          ]);
          const blocks = entries.flatMap((entry) => entry.kind === "message" ? entry.blocks || [] : []);
          const block = blocks.find((candidate) => candidate.type === "tool-call");
          assert.equal(block?.type === "tool-call" ? block.result : undefined, expected);
          assert.equal(block?.type === "tool-call" ? block.status : undefined, isError ? "error" : "success");
        }
      }
    });

    test(`${surface}: ${transport} mixed and structured tool results keep all content semantics`, () => {
      const payloads = [
        [{ type: "text", text: "A picture follows." }, { type: "image", data: "image-bytes", mime_type: "image/png" }],
        [{ type: "resource", text: "Resource label", uri: "file:///notes.txt" }],
        [{ type: "text", text: "Annotated text", annotations: { audience: ["user"] } }],
        { records: [{ text: "A field named text is structured data." }], count: 1 },
      ];
      for (const payload of payloads) {
        const entries = mapper(agent, [
          { id: "mixed-call", event: "tool_call_requested", sourceKind, data: { id: "mixed-1", name: "lookup", args: {} } },
          { id: "mixed-result", event: "tool_result_received", sourceKind, data: { tool_call_id: "mixed-1", name: "lookup", result: payload, is_error: false } },
        ]);
        const block = entries.flatMap((entry) => entry.kind === "message" ? entry.blocks || [] : [])
          .find((candidate) => candidate.type === "tool-call");
        if (payload === payloads[0]) {
          // The image is presented as an image beside its text, never as text.
          assert.equal(block?.type === "tool-call" ? block.result : undefined, "A picture follows.");
          assert.deepEqual(
            block?.type === "tool-call" ? block.images?.map((image) => image.src) : undefined,
            ["data:image/png;base64,image-bytes"],
          );
          continue;
        }
        assert.equal(block?.type === "tool-call" ? block.result : undefined, JSON.stringify(payload, null, 2));
      }
    });
  }

  test(`${surface}: persisted tool-use blocks retain empty and whitespace-only actual text results`, () => {
    for (const text of ["", "   \n", "  File content\n"]) {
      const entries = mapper(agent, [
        { ...history("saved-tool", "", runA), data: { message: { role: "block_assistant", blocks: [
          { block_type: "tool_use", data: { id: "saved-call", name: "read_file", args: { path: "notes.txt" } } },
        ] } } },
        { id: "saved-result", event: "tool_result_received", sourceKind: "session_history", runId: runA,
          data: { tool_call_id: "saved-call", result: [{ type: "text", text }], is_error: false } },
      ]);
      const block = entries.flatMap((entry) => entry.kind === "message" ? entry.blocks || [] : [])
        .find((candidate) => candidate.type === "tool-call");
      assert.equal(block?.type === "tool-call" ? block.result : undefined, text);
    }
  });

  for (const interactionId of [undefined, UUID_A]) {
    const lineage = interactionId ? "interaction and run" : "run only";
    for (const historyEvent of ["interaction_complete", "text_complete"]) {
      test(`${surface}: startup ${lineage} joins ${historyEvent} history before the final live chunk`, () => {
        const content = "| Result | Status |\n| --- | --- |\n| Router | Ready |\n";
        const owner = { runId: runA, interactionId };
        const prefix = content.slice(0, -3);
        const entries = texts([
          { id: "startup-open", event: "run_started", ...owner, data: {} },
          { id: "startup-first-text", event: "text_delta", ...owner, data: { delta: prefix } },
          { ...history("startup-history", content, runA, interactionId), event: historyEvent },
          { id: "startup-last-text", event: "text_delta", ...owner, data: { delta: content.slice(-3) } },
          { id: "startup-complete", event: "run_completed", ...owner, data: { result: content } },
        ]);
        assert.equal(entries.length, 1, JSON.stringify(entries));
        assert.equal(entries[0].id, "startup-first-text");
        assert.equal(entries[0].runId, runA);
        assert.equal(entries[0].interactionId, interactionId);
        assert.equal(entryVisibleTestText(entries[0]), content);
      });
    }

    test(`${surface}: startup ${lineage} preserves equal replies belonging to different runs`, () => {
      const entries = texts([
        { id: "first-reply", event: "interaction_complete", runId: runA, interactionId, data: { result: "Ready." } },
        history("second-reply", "Ready.", runB, interactionId),
      ]);
      assert.deepEqual(entries.map((entry) => [entry.id, entry.runId, entryVisibleTestText(entry)]), [
        ["first-reply", runA, "Ready."], ["second-reply", runB, "Ready."],
      ]);
    });
  }

  test(`${surface}: startup run ownership does not erase a different persisted message from that run`, () => {
    const entries = texts([
      { id: "final-reply", event: "run_completed", runId: runA, data: { result: "All done." } },
      history("earlier-response", "I will inspect the files first.", runA),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
      ["final-reply", "All done."], ["earlier-response", "I will inspect the files first."],
    ]);
  });

  test(`${surface}: startup preserves equal distinct history messages in one run`, () => {
    const entries = texts([history("saved-first", "Ready.", runA), history("saved-second", "Ready.", runA)]);
    assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
      ["saved-first", "Ready."], ["saved-second", "Ready."],
    ]);
  });

  test(`${surface}: startup preserves whitespace-significant authored messages in one run`, () => {
    const entries = texts([
      { id: "live-paragraphs", event: "run_completed", runId: runA, data: { result: "alpha\n\nbeta" } },
      history("saved-one-line", "alpha beta", runA),
      history("saved-hard-break", "alpha  \nbeta", runA),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
      ["live-paragraphs", "alpha\n\nbeta"], ["saved-one-line", "alpha beta"], ["saved-hard-break", "alpha  \nbeta"],
    ]);
  });

  test(`${surface}: startup consumes each owned live text occurrence once`, () => {
    const entries = texts([
      { id: "only-live-answer", event: "run_completed", runId: runA, data: { result: "Ready." } },
      history("saved-twin", "Ready.", runA),
      history("saved-second-occurrence", "Ready.", runA),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
      ["only-live-answer", "Ready."], ["saved-second-occurrence", "Ready."],
    ]);
  });

  for (const historyEvent of ["interaction_complete", "text_complete"]) {
    test(`${surface}: startup ${historyEvent} reconciles every arrival prefix with the opening live identity`, () => {
      const answer = "## Acceptance reply\n\n| State | Value |\n| --- | --- |\n| Result | Ready |\n";
      const owner = { runId: runA, interactionId: UUID_A };
      const frames = [
        { id: "prefix-open", event: "run_started", ...owner, data: {} },
        { id: "prefix-first", event: "text_delta", ...owner, data: { delta: answer.slice(0, -3) } },
        { ...history("prefix-history", answer, runA, UUID_A), event: historyEvent },
        { id: "prefix-last", event: "text_delta", ...owner, data: { delta: answer.slice(-3) } },
        { id: "prefix-terminal", event: "run_completed", ...owner, data: { result: answer } },
      ];
      assert.equal(texts(frames.slice(0, 1)).length, 0);
      for (let length = 2; length <= frames.length; length++) {
        const entries = texts(frames.slice(0, length));
        assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
          ["prefix-first", length === 2 ? answer.slice(0, -3) : answer],
        ], `arrival prefix ${length}`);
      }
    });
  }

  test(`${surface}: startup cannot treat a completed short message as the prefix of a different saved message`, () => {
    const entries = texts([
      { id: "short-first", event: "text_delta", runId: runA, data: { delta: "Ready." } },
      { id: "short-complete", event: "text_complete", runId: runA, data: { content: "Ready." } },
      history("longer-distinct", "Ready. Another message.", runA),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
      ["short-first", "Ready."], ["longer-distinct", "Ready. Another message."],
    ]);
  });

  test(`${surface}: startup history stays available when text deltas are disabled`, () => {
    const entries = mapper(agent, [
      { id: "ignored-delta", event: "text_delta", runId: runA, data: { delta: "Ready." } },
      history("only-rendered-answer", "Ready.", runA),
    ], { textMode: "markdown", renderTextDeltas: false });
    assert.deepEqual(entries.map((entry) => [entry.id, entryVisibleTestText(entry)]), [
      ["only-rendered-answer", "Ready."],
    ]);
  });

  test(`${surface}: startup live text does not erase persisted tool and reasoning siblings`, () => {
    const entries = texts([
      { id: "live-answer", event: "run_completed", runId: runA, data: { result: "Ready." } },
      { ...history("history-with-siblings", "Ready.", runA), data: {
        text: "Ready.", result: "Ready.", message: { role: "block_assistant", blocks: [
          { block_type: "reasoning", data: { text: "Check the file first." } },
          { block_type: "tool_use", data: { id: "history-only-tool", name: "read_file", args: { path: "a" } } },
          { block_type: "text", data: { text: "Ready." } },
        ] },
      } },
    ]);
    const blocks = entries.flatMap((entry) => entry.kind === "message" ? entry.blocks || [] : []);
    assert.equal(blocks.filter((block) => block.type === "markdown").length, 1);
    assert.equal(blocks.filter((block) => block.type === "tool-call").length, 1);
    assert.equal(blocks.filter((block) => block.type === "thinking").length, 1);
  });

  for (const historyEvent of ["interaction_complete", "text_complete"]) {
    test(`${surface}: startup ${historyEvent} joins only owned thinking occurrences without splitting live text`, () => {
      const thought = "Check again again and check again.";
      const answer = "Review complete. Both independent checks support the release candidate.";
      const owner = { runId: runA, interactionId: UUID_A };
      const entries = texts([
        { id: "first-thought", event: "reasoning_complete", ...owner, data: { content: thought } },
        { id: "second-thought", event: "reasoning_complete", ...owner, data: { content: thought } },
        { id: "answer-start", event: "text_delta", ...owner, data: { delta: answer.slice(0, -5) } },
        { ...history("answer-history", answer, runA, UUID_A), event: historyEvent, data: {
          result: answer, message: { role: "block_assistant", blocks: [
            { block_type: "reasoning", data: { text: thought } },
            { block_type: "reasoning", data: { text: thought } },
            { block_type: "text", data: { text: answer } },
          ] },
        } },
        { id: "answer-tail", event: "text_delta", ...owner, data: { delta: answer.slice(-5) } },
        { id: "answer-complete", event: "interaction_complete", ...owner, data: { result: answer } },
      ]);
      assert.deepEqual(entries.map((entry) => entry.id), ["first-thought", "second-thought", "answer-start"]);
      assert.deepEqual(entries.flatMap((entry) => entry.kind === "message" ? entry.blocks || [] : [])
        .filter((block) => block.type === "thinking").map((block) => block.text), [thought, thought]);
      assert.equal(entryVisibleTestText(entries[2]), answer);
    });
  }

  test(`${surface}: startup thinking join preserves excess occurrences and equal blocks from other runs`, () => {
    const thought = "Check again.";
    const historyThinking = (id: string, runId: string, count: number) => ({
      ...history(id, "", runId, UUID_A), data: { result: "", message: { role: "block_assistant",
        blocks: Array.from({ length: count }, () => ({ block_type: "reasoning", data: { text: thought } })),
      } },
    });
    const entries = texts([
      { id: "only-live-thought", event: "reasoning_complete", runId: runA, interactionId: UUID_A, data: { content: thought } },
      historyThinking("two-history-thoughts", runA, 2),
      historyThinking("foreign-history-thought", runB, 1),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entry.kind === "message" ? entry.blocks?.length : 0]), [
      ["only-live-thought", 1], ["two-history-thoughts", 1], ["foreign-history-thought", 1],
    ]);
  });

  test(`${surface}: startup admission alone cannot suppress a persisted answer`, () => {
    const entries = texts([
      { id: "startup-open", event: "run_started", runId: runA, interactionId: UUID_A, data: {} },
      history("only-answer", "Durable answer.", runA, UUID_A),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entry.runId, entryVisibleTestText(entry)]), [
      ["only-answer", runA, "Durable answer."],
    ]);
  });

  test(`${surface}: startup run boundaries keep equal adjacent streams separate`, () => {
    const entries = texts([
      { id: "first-delta", event: "text_delta", runId: runA, interactionId: UUID_A, data: { delta: "Ready." } },
      { id: "second-delta", event: "text_delta", runId: runB, interactionId: UUID_A, data: { delta: "Ready." } },
      { id: "second-complete", event: "run_completed", runId: runB, interactionId: UUID_A, data: { result: "Ready." } },
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entry.runId, entryVisibleTestText(entry)]), [
      ["first-delta", runA, "Ready."], ["second-delta", runB, "Ready."],
    ]);
  });

  test(`${surface}: startup completion without live deltas renders its owned answer`, () => {
    const entries = texts([
      { id: "complete-only", event: "run_completed", runId: runA, data: { result: "Ready." } },
      history("durable-twin", "Ready.", runA),
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entry.runId, entryVisibleTestText(entry)]), [
      ["complete-only", runA, "Ready."],
    ]);
  });

  test(`${surface}: startup foreign terminal cannot consume another run's equal streamed answer`, () => {
    const entries = texts([
      { id: "active-delta", event: "text_delta", runId: runA, data: { delta: "Ready." } },
      { id: "foreign-terminal", event: "interaction_complete", runId: runB, data: { result: "Ready." } },
    ]);
    assert.deepEqual(entries.map((entry) => [entry.id, entry.runId, entryVisibleTestText(entry)]), [
      ["active-delta", runA, "Ready."], ["foreign-terminal", runB, "Ready."],
    ]);
  });
}

test("a council emits ONE card even when two frames each carry the sealed result", () => {
  // Regression: tool_result_received AND tool_execution_completed can each
  // carry the result, and each parses to the same `council:{id}` key. Without
  // a dedupe the adapter pushed both - duplicate cards and a colliding React
  // key. Reverting the dedupe must fail HERE; the fold tests only pin that
  // both frames parse, which is the precondition, not the behaviour.
  const sealed = {
    result: {
      council_id: "c-dupe",
      exit_reason: { reason: "completed" },
      rounds_completed: 1,
      participants: [],
      exchanges: [],
      merge: { kind: "no_merge" },
    },
    cleanup: { debts: [] },
    replayed: false,
  };
  const entries = mapFramesToTimelineEntries(
    { agent_id: "planner", member_id: "planner", label: "Planner", kind: "identity" },
    [
      {
        id: "evt-1",
        event: "tool_call",
        data: {
          name: "council",
          tool_call_id: "call-1",
          arguments: JSON.stringify({ topic: "Ship or hold?", participants: [] }),
        },
      },
      {
        id: "evt-2",
        event: "tool_result_received",
        data: { name: "council", tool_call_id: "call-1", result: sealed },
      },
      {
        id: "evt-3",
        event: "tool_execution_completed",
        data: { name: "council", tool_call_id: "call-1", result: sealed },
      },
    ] as never,
  );

  const councils = entries.filter((entry) => entry.kind === "council");
  assert.equal(councils.length, 1, "exactly one council card");
  assert.equal(new Set(councils.map((entry) => entry.id)).size, 1);
  // The topic comes from the CALL frame, so the fold must still see it.
  assert.equal((councils[0] as { topic: string }).topic, "Ship or hold?");
  // And no bare "council" tool row survives beside the card.
  const toolRows = entries.filter((entry) => (
    entry.kind === "message" && JSON.stringify(entry).includes('"name":"council"')
  ));
  assert.equal(toolRows.length, 0, "the raw council tool call is suppressed");
});

test("a council card renders from the result event when the execution fact carries no body", () => {
  const sealed = {
    result: {
      council_id: "c-single",
      exit_reason: { reason: "completed" },
      rounds_completed: 1,
      participants: [],
      exchanges: [],
      merge: { kind: "no_merge" },
    },
    cleanup: { debts: [] },
    replayed: false,
  };
  const entries = mapFramesToTimelineEntries(
    { agent_id: "planner", member_id: "planner", label: "Planner", kind: "identity" },
    [
      {
        id: "evt-1",
        event: "tool_call",
        data: {
          name: "council",
          tool_call_id: "call-1",
          arguments: JSON.stringify({ topic: "Ship or hold?", participants: [] }),
        },
      },
      {
        id: "evt-2",
        event: "tool_execution_completed",
        data: { name: "council", tool_call_id: "call-1", is_error: false, duration_ms: 40 },
      },
      {
        id: "evt-3",
        event: "tool_result_received",
        data: { name: "council", tool_call_id: "call-1", is_error: false, result: JSON.stringify(sealed) },
      },
    ] as never,
  );

  const councils = entries.filter((entry) => entry.kind === "council");
  assert.equal(councils.length, 1, "exactly one council card, from the result event");
  assert.equal((councils[0] as { topic: string }).topic, "Ship or hold?");
  const toolRows = entries.filter((entry) => (
    entry.kind === "message" && JSON.stringify(entry).includes('"name":"council"')
  ));
  assert.equal(toolRows.length, 0, "the bodiless execution fact leaves no bare council row");
});

// A MID-TURN STEER. The operator types while the agent is still working, and
// everything the agent produces afterwards must render BELOW that message.
//
// The frames below reproduce the real shape rather than a convenient one: a
// steer is minted as its OWN interaction (the aggregator mints a fresh id for
// every console send), while the running turn's in-flight frames keep the
// ORIGINAL id, because `active_interaction_by_identity` is only reassigned by
// `run_started` and a steer merely joins the pending queue.
//
// That is the case the transcript sort used to get wrong: it compared the two
// frames' interaction GROUP START times and returned before own timestamps were
// ever consulted, so the turn's later frames inherited the turn's earlier start
// and sorted above the steer permanently.
test("a mid-turn steer renders before the frames its own turn produced after it", () => {
  const STEER_MS = 1_779_405_500_000;
  const entries = mapFramesToTimelineEntries(
    {
      agent_id: "calendar",
      member_id: "calendar",
      label: "calendar",
      kind: "identity",
    },
    [
      {
        id: "turn-start",
        cursor: "console:1",
        event: "interaction_started",
        timestampMs: STEER_MS - 60_000,
        interactionId: "turn-0",
        data: { content: "check the attendance records" },
      },
      {
        id: "before",
        cursor: "console:2",
        event: "tool_call_requested",
        timestampMs: STEER_MS - 30_000,
        interactionId: "turn-0",
        data: { id: "call-before", name: "get_activities_by_date", args: {} },
      },
      // The operator's steer: its own interaction, later than the turn's start.
      {
        id: "steer",
        cursor: "console:3",
        event: "user_input",
        timestampMs: STEER_MS,
        interactionId: "steer-1",
        data: { content: "fyi these were the fixes", status: "accepted" },
      },
      // Still the ORIGINAL turn, produced AFTER the steer. This is the frame
      // that used to jump above it.
      {
        id: "after",
        cursor: "console:4",
        event: "tool_call_requested",
        timestampMs: STEER_MS + 27_000,
        interactionId: "turn-0",
        data: { id: "call-after", name: "get_activities_by_date", args: {} },
      },
    ],
    { renderInteractionStartsAsUser: true, renderTextDeltas: true },
  );

  const order = entries.map((entry) => entry.id);
  const steerIndex = order.findIndex((id) => String(id).includes("steer"));
  const afterIndex = order.findIndex((id) => String(id).includes("after"));
  const beforeIndex = order.findIndex((id) => String(id).includes("before"));

  assert.ok(steerIndex >= 0, `the steer must render at all: ${JSON.stringify(order)}`);
  assert.ok(afterIndex >= 0, `the post-steer frame must render: ${JSON.stringify(order)}`);
  assert.ok(beforeIndex >= 0, `the pre-steer frame must render: ${JSON.stringify(order)}`);

  // The property the operator actually asked for.
  assert.ok(
    afterIndex > steerIndex,
    `a frame produced 27s AFTER the steer rendered ABOVE it. The operator's message is \
pinned to the bottom while the turn streams over it. order=${JSON.stringify(order)}`,
  );
  // The other half: the steer must not be dragged to the top either. Ordering
  // by own timestamp has to keep the turn's EARLIER frames above it.
  assert.ok(
    beforeIndex < steerIndex,
    `a frame produced 30s BEFORE the steer rendered below it: order=${JSON.stringify(order)}`,
  );
});

// Contiguity is not sacrificed when interactions do NOT overlap. Ordering by
// own timestamp gives grouping for free in the ordinary case, so this pins that
// the fix above did not trade one defect for another.
test("two sequential interactions stay contiguous when they do not overlap", () => {
  const BASE = 1_779_405_600_000;
  const entries = mapFramesToTimelineEntries(
    { agent_id: "calendar", member_id: "calendar", label: "calendar", kind: "identity" },
    [
      {
        id: "a-start",
        cursor: "console:1",
        event: "interaction_started",
        timestampMs: BASE,
        interactionId: "turn-a",
        data: { content: "first question" },
      },
      {
        id: "a-tool",
        cursor: "console:2",
        event: "tool_call_requested",
        timestampMs: BASE + 1_000,
        interactionId: "turn-a",
        data: { id: "call-a", name: "lookup", args: {} },
      },
      {
        id: "b-start",
        cursor: "console:3",
        event: "interaction_started",
        timestampMs: BASE + 10_000,
        interactionId: "turn-b",
        data: { content: "second question" },
      },
      {
        id: "b-tool",
        cursor: "console:4",
        event: "tool_call_requested",
        timestampMs: BASE + 11_000,
        interactionId: "turn-b",
        data: { id: "call-b", name: "lookup", args: {} },
      },
    ],
    { renderInteractionStartsAsUser: true, renderTextDeltas: true },
  );

  const order = entries.map((entry) => String(entry.id));
  const idx = (needle: string) => order.findIndex((id) => id.includes(needle));
  assert.ok(
    idx("a-start") < idx("a-tool") && idx("a-tool") < idx("b-start") && idx("b-start") < idx("b-tool"),
    `non-overlapping interactions must stay grouped and in order: ${JSON.stringify(order)}`,
  );
});

test("failure summary reads meerkat's typed error_report before the flat keys", () => {
  // A meerkat >= 0.7 `run_failed` payload as Meta projects it into an
  // `interaction_failed` frame: the typed report is the failure truth.
  const typed = {
    type: "run_failed",
    session_id: "01a068a0-f911-7331-8187-d436145ac895",
    error_report: {
      class: "llm",
      reason: { reason_type: "llm_auth_error" },
      message: "LLM error: authentication failed (401)",
    },
  };
  assert.deepEqual(summarizeFailureData(typed), {
    message: "LLM error: authentication failed (401)",
    reasonType: "llm_auth_error",
  });
  assert.equal(describeFailure(typed), "LLM error: authentication failed (401) (llm_auth_error)");

  // Frames Meta mints itself carry only the flat keys.
  assert.equal(describeFailure({ reason: "superseded_by_later_run" }), "superseded_by_later_run");
  assert.equal(describeFailure({ error: "member not found: mk--x" }), "member not found: mk--x");

  // meerkat's own interaction_failed carries a typed reason object; a
  // discriminator the message already spells out is not repeated.
  assert.equal(
    describeFailure({ error: "interaction cancelled", reason: { kind: "cancelled" } }),
    "interaction cancelled",
  );
  assert.equal(
    describeFailure({
      error: "peer response undeliverable",
      reason: { kind: "interaction_stream_abandoned" },
    }),
    "peer response undeliverable (interaction_stream_abandoned)",
  );

  // Nothing readable falls back to the caller's label, never to "[object Object]".
  assert.equal(describeFailure({ session_id: "s" }), "error");
  assert.equal(describeFailure({ session_id: "s" }, ""), "");
  assert.equal(describeFailure(undefined), "error");
});

test("mapFramesToTimelineEntries on a head-trimmed log equals the suffix of the full derivation", () => {
  // ConsoleApp caps each identity log at MAX_IDENTITY_LOG_EVENTS and drops
  // the oldest frames in transcript order. The cut can land inside an
  // interaction (its `interaction_started` gone, its completion kept). The
  // adapter must neither throw nor change any entry for interactions whose
  // frames all survived, so the visible transcript below the cut is
  // byte-identical to what the uncapped log would have rendered.
  const agent = {
    agent_id: "capped",
    member_id: "capped",
    label: "Capped",
    kind: "identity" as const,
  };
  const full: Array<Record<string, unknown>> = [];
  const base = 1_700_000_000_000;
  for (let i = 0; i < 40; i += 1) {
    const iid = `turn-${i}`;
    const ts = base + i * 10_000;
    full.push({ id: `${i}:start`, event: "interaction_started", interactionId: iid, timestampMs: ts, cursor: `console:${i * 4 + 1}`, data: { text: `question ${i}` } });
    full.push({ id: `${i}:delta-a`, event: "text_delta", interactionId: iid, timestampMs: ts + 1_000, cursor: `console:${i * 4 + 2}`, data: { delta: "part one " } });
    full.push({ id: `${i}:delta-b`, event: "text_delta", interactionId: iid, timestampMs: ts + 2_000, cursor: `console:${i * 4 + 3}`, data: { delta: "part two" } });
    full.push({ id: `${i}:done`, event: "interaction_complete", interactionId: iid, timestampMs: ts + 3_000, cursor: `console:${i * 4 + 4}`, data: { text: `**answer ${i}** with \`code\`` } });
  }
  const options = { renderInteractionStartsAsUser: true, renderTextDeltas: true };
  const fullEntries = mapFramesToTimelineEntries(agent, full as never, options as never);
  // Cut mid-interaction 3: keep from its first delta onward.
  const cut = 3 * 4 + 1;
  const trimmed = full.slice(cut);
  const trimmedEntries = mapFramesToTimelineEntries(agent, trimmed as never, options as never);
  assert.ok(trimmedEntries.length > 0);
  // Entries for interactions 4.. must be identical in both derivations.
  const fromTurn4 = (entries: unknown[]) => {
    const index = entries.findIndex((entry) => {
      const iid = (entry as { interactionId?: string }).interactionId;
      return iid === "turn-4";
    });
    assert.ok(index >= 0, "turn-4 entry present");
    return entries.slice(index);
  };
  // Entry ids carry the frame's position in the input as a uniqueness
  // suffix, so they shift by the number of dropped frames; every other
  // field must match exactly.
  const withoutPositionalId = (entries: unknown[]) =>
    entries.map((entry) => {
      const { id, ...rest } = entry as { id?: string };
      return { ...rest, idStem: typeof id === "string" ? id.replace(/:\d+$/, "") : id };
    });
  assert.deepEqual(
    withoutPositionalId(fromTurn4(trimmedEntries)),
    withoutPositionalId(fromTurn4(fullEntries)),
  );
  // The orphaned tail of interaction 3 renders as something, not nothing.
  assert.ok(
    trimmedEntries.some((entry) => (entry as { interactionId?: string }).interactionId === "turn-3"),
    "partially retained interaction still renders",
  );
});

test("a deduped user input keeps the typed origin carried by any of its twins", () => {
  const agent = { agent_id: "triage", member_id: "triage", label: "Triage", kind: "identity" as const };
  const interactionId = "5b0f6c1e-2f4a-4d6e-9a51-3c1d2e4f5a6b";
  const frames = [
    {
      id: "history-1",
      event: "user_input",
      interactionId,
      sourceKind: "session_history",
      timestampMs: 1_000,
      cursor: "console:1",
      data: { content: "Reply with the proof token.", message: { role: "user", content: "Reply with the proof token." } },
    },
    {
      id: "send-1",
      event: "user_input",
      interactionId,
      timestampMs: 1_000,
      cursor: "console:2",
      data: { content: "Reply with the proof token.", origin: "example:gate", origin_kind: "operator_probe" },
    },
  ];
  for (const map of [mapFramesToTimelineEntries, mapFramesToTimelineEntriesShared]) {
    const entries = map(agent as never, frames as never, { renderInteractionStartsAsUser: true } as never);
    const users = entries.filter((entry) => entry.kind === "message" && entry.identity.role === "user");
    assert.equal(users.length, 1);
    const origin = users[0].kind === "message" ? users[0].origin : null;
    assert.deepEqual(origin, { sendOrigin: "example:gate", originKind: "operator_probe" });
  }
});

for (const [surface, project] of [["stock", mapFramesToTimelineEntries], ["shared", mapFramesToTimelineEntriesShared]] as const) {
  test(`${surface}: canonical peer display metadata remains distinct from exact identity`, () => {
    const peerId = "7a643fe3-e700-5846-85ab-a6ed817c7754";
    const entries = project({ agent_id: "router:main", member_id: "router:main", label: "Router", kind: "identity" }, [{
      id: "canonical-peer-label", event: "system_notice", sourceKind: "session_history", timestampMs: 1000,
      data: { message: typedCommsNotice({ peer: "console-acceptance/lead/mk--domain_cdelivery", peerId, body: "Delivery review completed." }) },
    }]);
    const block = entries.flatMap(entry => entry.kind === "message" ? entry.blocks || [] : []).find(block => block.type === "tool-call");
    assert.equal(block?.type, "tool-call");
    if (block?.type !== "tool-call") return;
    assert.equal(block.peerIdentity, peerId);
    assert.equal(block.peerDisplayLabel, "domain:delivery");
  });
  test(`${surface}: outgoing display labels require canonical peer registry evidence`, () => {
    const peerId = "7a643fe3-e700-5846-85ab-a6ed817c7754";
    const send = { id: "send-canonical-peer", event: "tool_call_requested", timestampMs: 2000,
      data: { name: "send_message", id: "send-label", args: { peer_id: peerId, display_name: "Untrusted sender hint", content: "Please review." } } };
    const agent = { agent_id: "router:main", member_id: "router:main", label: "Router", kind: "identity" };
    const blockFrom = (frames: typeof send[]) => project(agent, frames).flatMap(entry => entry.kind === "message" ? entry.blocks || [] : []).find(block => block.type === "tool-call" && block.toolCallId === "send-label");
    const unknown = blockFrom([send]);
    assert.equal(unknown?.type, "tool-call");
    if (unknown?.type === "tool-call") assert.equal(unknown.peerDisplayLabel, undefined);
    const known = blockFrom([{ id: "peers-label", event: "tool_result_received", timestampMs: 1000,
      data: { name: "peers", id: "peers-call", result: JSON.stringify({ peers: [{ peer_id: peerId, name: "console-acceptance/lead/mk--domain_cdelivery" }] }) } } as unknown as typeof send, send]);
    assert.equal(known?.type, "tool-call");
    if (known?.type === "tool-call") {
      assert.equal(known.peerIdentity, peerId);
      assert.equal(known.peerDisplayLabel, "domain:delivery");
    }
  });
}

const DURATION_AGENT = { agent_id: "router:main", member_id: "router:main", label: "Router", kind: "identity" };
const DURATION_OWNER = {
  runtimeKey: "default", identity: "router:main", sessionId: "01a0dc27-ae06-7371-914e-418f0a5949fc",
  runId: "01a0dc27-b35d-7b30-8075-046846042446", interactionId: "9e46e6d7-a73e-5616-b323-daf84d0fa535",
};
const DURATION_FRAMES: Parameters<typeof mapFramesToTimelineEntries>[1] = [
  { ...DURATION_OWNER, id: "duration-start", event: "run_started", timestampMs: 1790399984485, data: { type: "run_started" } },
  { ...DURATION_OWNER, id: "duration-prefix", event: "text_delta", timestampMs: 1790399984838, data: { delta: "Reviewing. " } },
  { ...DURATION_OWNER, id: "duration-notice", event: "system_notice", timestampMs: 1790399984900, data: { kind: "generic", body: "Keep the background instruction." } },
  { ...DURATION_OWNER, id: "duration-final", event: "text_delta", timestampMs: 1790399985000, data: { delta: "Review complete." } },
  { ...DURATION_OWNER, id: "duration-complete", event: "interaction_complete", timestampMs: 1790399990171, data: { type: "run_completed", extraction_required: false, result: "Reviewing. Review complete." } },
];

function runTimedEntries(frames: Parameters<typeof mapFramesToTimelineEntries>[1]) {
  return mapFramesToTimelineEntries(DURATION_AGENT, frames).filter((entry) => entry.kind === "message" && entry.runDurationMs !== undefined);
}

test("stock run duration uses exact start and terminal across a delayed stream and notice", () => {
  const entries = mapFramesToTimelineEntries(DURATION_AGENT, DURATION_FRAMES);
  const timed = entries.filter((entry) => entry.kind === "message" && entry.runDurationMs !== undefined);
  assert.equal(timed.length, 1, JSON.stringify(entries));
  assert.equal(timed[0].id, "duration-final");
  assert.equal(timed[0].kind === "message" ? timed[0].runDurationMs : undefined, 5686);
  assert.equal(timed[0].createdAt, new Date(1790399985000).toISOString());
  assert.equal(entries.find((entry) => entry.id === "duration-notice")?.identity.role, "system");
});

test("stock run duration requires valid complete run timing", () => {
  const start = DURATION_FRAMES[0];
  const terminal = DURATION_FRAMES[4];
  const middle = DURATION_FRAMES.slice(1, 4);
  const cases: Array<[string, Parameters<typeof mapFramesToTimelineEntries>[1]]> = [
    ["missing start", [...middle, terminal]],
    ["missing terminal", [start, ...middle]],
    ["text completion only", [start, ...middle, { ...terminal, event: "text_complete", data: { content: "Reviewing. Review complete." } }]],
    ["untyped interaction completion", [start, ...middle, { ...terminal, data: { result: "Reviewing. Review complete." } }]],
    ["extraction pending", [start, ...middle, { ...terminal, data: { ...terminal.data as object, extraction_required: true } }]],
    ["delivery receipt", [start, ...middle, { ...terminal, data: { ...terminal.data as object, reason: "steer_delivered" } }]],
    ["history terminal", [start, ...middle, { ...terminal, sourceKind: "session_history" }]],
    ["reversed timing", [{ ...start, timestampMs: terminal.timestampMs! + 1 }, ...middle, terminal]],
    ["missing start time", [{ ...start, timestampMs: undefined }, ...middle, terminal]],
    ["invalid terminal time", [start, ...middle, { ...terminal, timestampMs: Number.NaN }]],
    ["conflicting starts", [start, { ...start, id: "other-start", timestampMs: start.timestampMs! + 1 }, ...middle, terminal]],
  ];
  for (const [label, frames] of cases) assert.equal(runTimedEntries(frames).length, 0, label);
});

test("stock run duration never crosses runtime session agent run or interaction ownership", () => {
  for (const field of ["runtimeKey", "sessionId", "identity", "runId", "interactionId"] as const) {
    const frames = DURATION_FRAMES.map((frame) => frame.id === "duration-start" ? { ...frame, [field]: `other-${field}` } : frame);
    assert.equal(runTimedEntries(frames).length, 0, field);
  }
  const noRun = DURATION_FRAMES.map((frame) => ({ ...frame, runId: undefined }));
  assert.equal(runTimedEntries(noRun).length, 0, "missing run");
  const noSession = DURATION_FRAMES.map((frame) => ({ ...frame, sessionId: undefined }));
  assert.equal(runTimedEntries(noSession).length, 0, "missing session");
});

test("stock run duration stays accurate with canonical history present", () => {
  const frames = [
    DURATION_FRAMES[0],
    { ...DURATION_OWNER, id: "duration-history", event: "text_complete", sourceKind: "session_history", timestampMs: 1790399990169,
      data: { text: "Reviewing. Review complete.", result: "Reviewing. Review complete.", message: { role: "block_assistant", stop_reason: "end_turn", blocks: [{ block_type: "text", data: { text: "Reviewing. Review complete." } }] } } },
    DURATION_FRAMES[4],
  ];
  const timed = runTimedEntries(frames);
  assert.equal(timed.length, 1);
  assert.equal(entryVisibleTestText(timed[0]), "Reviewing. Review complete.");
  assert.equal(timed[0].kind === "message" ? timed[0].runDurationMs : undefined, 5686);
});

test("stock run duration accepts a direct successful run terminal and ignores trailing tool evidence", () => {
  const frames = [...DURATION_FRAMES.slice(0, 4),
    { ...DURATION_FRAMES[4], event: "run_completed" },
    { ...DURATION_OWNER, id: "duration-tool", event: "tool_call", timestampMs: 1790399990172, data: { id: "trailing-tool", name: "check", args: {} } },
    { ...DURATION_OWNER, id: "duration-tool-done", event: "tool_execution_completed", timestampMs: 1790399990173, data: { id: "trailing-tool", name: "check", result: "Checked", is_error: false } },
  ];
  const timed = runTimedEntries(frames);
  assert.equal(timed.length, 1);
  assert.equal(timed[0].id, "duration-final");
  assert.equal(timed[0].kind === "message" ? timed[0].runDurationMs : undefined, 5686);
});


test("workgraph local card event and paused times use the browser clock without changing source", () => {
  const previousTimeZone = process.env.TZ;
  process.env.TZ = "America/Los_Angeles";
  try {
    const at = "2026-07-09T01:15:30Z";
    for (const [instant, expectedEvent, expectedPause] of [
      [at, "created · 18:15", "paused until 2026-07-08 18:15"],
      ["invalid-event-timestamp", "created", "paused"],
    ]) {
      const frames = workGraphToolFrames({ idPrefix: `local-time-${instant}`, name: "workgraph_snapshot", callArgs: {}, result: {
        snapshot: { items: [workGraphItem({ id: "local-item", title: "Local time", createdAt: at })], edges: [],
          attention: [{ binding_id: "local-binding", mode: "pursue", work_ref: { item_id: "local-item" }, status: { state: "paused", until: instant } }] },
        events: [{ seq: 1, kind: "created", item_id: "local-item", at: instant }],
      } });
      const before = JSON.stringify(frames);
      const cards = mapFramesToTimelineEntries(WORKGRAPH_AGENT, frames).filter((entry) => entry.kind === "workgraph");
      assert.equal(cards.length, 1);
      assert.deepEqual(cards[0].recentEvents, [expectedEvent]);
      assert.equal(cards[0].attention[0].statusLabel, expectedPause);
      assert.equal(cards[0].items[0].createdAt, at);
      assert.equal(JSON.stringify(frames), before, "formatting must not rewrite event timestamps or IDs");
    }
  } finally {
    if (previousTimeZone === undefined) delete process.env.TZ;
    else process.env.TZ = previousTimeZone;
  }
});

// An approval can arrive before the model produces its first assistant token.
// Project real input shapes first so the renderer is not handed invented IDs.
for (const [surface, project] of [["stock", mapFramesToTimelineEntries], ["shared", mapFramesToTimelineEntriesShared]] as const) {
  const ownerId = "b6503200-06b6-4157-afc0-2a3c8d124178";
  const otherId = "cd425e60-ae28-4013-84c4-e25edc113c04";
  const agent = { agent_id: "router:main", member_id: "router:main", label: "Router", kind: "identity" };
  const content = [
    { type: "text", text: "Review this release before the model responds." },
    { type: "image", source: "blob", blob_id: "release-proof", media_type: "image/png" },
  ];
  const cases = [
    ["live text", "user_input", { content: content[0].text }, "console_event"],
    ["live image", "user_input", { content }, "console_event"],
    ["history text", "interaction_started", { content: content[0].text }, "session_history"],
    ["history image", "user_input", { content }, "session_history"],
    ["prompt text", "run_started", { prompt: content[0].text }, "console_event"],
    ["prompt image", "run_started", { prompt: content }, "console_event"],
  ] as const;
  for (const [label, event, data, sourceKind] of cases) {
    test(`${surface}: operator approval ownership survives ${label} before the first assistant token`, () => {
      const input = { id: "accepted-input", event, sourceKind, identity: "router:main", interactionId: ownerId,
        runId: "active-run", sessionId: "active-session", timestampMs: 1000, data };
      const before = JSON.stringify(input);
      const entries = project(agent, [input], { renderInteractionStartsAsUser: true, textMode: "markdown" });
      assert.equal(entries.length, 1);
      assert.equal(entries[0].identity.role, "user");
      assert.equal(entries[0].interactionId, ownerId);
      const approval = normalizePendingApprovalForInputTest({ pending_id: "release-approval", action_id: "release", action: "Review release",
        origin: { identity: "router:main", interaction_id: ownerId } })!;
      const interactionIds = entries.flatMap(entry => entry.interactionId ? [entry.interactionId] : []);
      const target = { identity: "router:main", conversationId: "router:main", interactionIds };
      assert.equal(approvalMatchesInputConversation(approval, target), true);
      assert.equal(approvalMatchesInputConversation({ ...approval, origin: { identity: "router:main", interactionId: otherId } }, target), false);
      assert.equal(approvalMatchesInputConversation({ ...approval, origin: { identity: "other-agent", interactionId: ownerId } }, target), false);
      assert.equal(JSON.stringify(input), before, "projection must not mutate source frames");
    });
  }
  for (const event of ["user_input", "run_started"]) {
    test(`${surface}: operator approval ownership survives plain legacy ${event}`, () => {
      const entries = project(agent, [{ id: "plain-input", event, interactionId: ownerId,
        data: { content: content[0].text, prompt: content[0].text } }], { renderInteractionStartsAsUser: true, textMode: "legacy" });
      assert.equal(entries.length, 1);
      assert.equal(entries[0].kind === "message" && entries[0].variant, "plain");
      assert.equal(entries[0].interactionId, ownerId);
    });
  }
  test(`${surface}: operator approval ownership and typed origin survive deduped input twins`, () => {
    const common = { identity: "router:main", interactionId: ownerId, sessionId: "active-session" };
    const history = { ...common, id: "history-input", event: "interaction_started", sourceKind: "session_history", data: { content: content[0].text } };
    const live = { ...common, id: "accepted-input", event: "user_input", sourceKind: "console_event", data: { content: content[0].text, origin: "console:operator", origin_kind: "operator" } };
    const prompt = { ...common, id: "run-prompt", event: "run_started", sourceKind: "console_event", runId: "active-run", data: { prompt: content[0].text } };
    for (const order of [[history, live, prompt], [live, prompt, history], [prompt, history, live]]) {
      const frames = order.map((frame, index) => ({ ...frame, timestampMs: 1000 + index, cursor: `console:${index + 1}` }));
      const entries = project(agent, frames, { renderInteractionStartsAsUser: true, textMode: "markdown" });
      assert.equal(entries.length, 1, order[0].id);
      assert.equal(entries[0].id, order[0].id, "retain the first canonical entry identity");
      assert.equal(entries[0].interactionId, ownerId);
      assert.deepEqual(entries[0].kind === "message" ? entries[0].origin : null, { sendOrigin: "console:operator", originKind: "operator" });
    }
  });
  test(`${surface}: operator approval ownership is never inferred from text actors or nested hints`, () => {
    for (const event of ["user_input", "run_started"]) {
      const hinted = { content: `Review interaction ${ownerId}`, prompt: `Review interaction ${ownerId}`, actor_id: ownerId,
        origin: "console:operator", identity: { interaction_id: ownerId }, interaction_id: ownerId };
      const entries = project(agent, [{ id: "unscoped-input", event, data: hinted }], { renderInteractionStartsAsUser: true });
      assert.equal(entries.length, 1);
      assert.equal(entries[0].identity.role, "user");
      assert.equal(entries[0].interactionId, undefined);
      const request = normalizePendingApprovalForInputTest({ pending_id: "release-approval", origin: { identity: "router:main", interaction_id: ownerId } })!;
      assert.equal(approvalMatchesInputConversation(request, { identity: "router:main", interactionIds: [] }), false);
    }
  });
}

for (const [name, map] of [["console", mapFramesToTimelineEntries], ["shared", mapFramesToTimelineEntriesShared]] as const) {
  test(`authorization feedback ${name} keeps separate attempts, suppresses replay and omits private diagnostics`, () => {
    const base = { runId: "run-a", interactionId: "work-a", sessionId: "session", identity: "a" };
    const start = { ...base, id: "tool-start", event: "tool_call_requested", data: { id: "call-a", name: "calendar", args: {} } };
    const refused = { ...base, id: "refusal", event: "tool_result_received", data: { id: "call-a", name: "calendar", is_error: true,
      content: [{ type: "text", text: JSON.stringify({ error: "operation_refused", message: "private-refusal-canary", data: { secret: "hidden-policy-canary" } }) }] } };
    const result = map({ agent_id: "a", label: "Agent" }, [start, refused,
      { ...refused, id: "replay", event: "tool_execution_completed" },
      { ...refused, id: "other-attempt", runId: "run-b", interactionId: "work-b" },
      { ...base, id: "allowed-start", event: "tool_call_requested", data: { id: "allowed", name: "read", args: {} } },
      { ...base, id: "allowed-result", event: "tool_result_received", data: { id: "allowed", name: "read", is_error: false, content: [{ type: "text", text: "Actual permitted result" }] } },
      { ...base, id: "audit", event: "operation_observation_failed", data: { operation_id: "op-a", phase: "outcome", secret: "audit-secret-canary" } },
    ]);
    assert.equal(result.filter(e => "operationFeedback" in e && e.operationFeedback?.kind === "operation-refused").length, 2);
    assert.match(JSON.stringify(result), /Actual permitted result/);
    assert.doesNotMatch(JSON.stringify(result), /private-refusal-canary|hidden-policy-canary|audit-secret-canary/);
    const ordinary = map({ agent_id: "a", label: "Agent" }, [{ ...refused, data: { ...refused.data,
      content: [{ type: "text", text: JSON.stringify({ error: "other", message: "operation_refused" }) }] } }]);
    assert.equal(ordinary.some(e => "operationFeedback" in e && e.operationFeedback), false);
  });
}

for (const [name, project] of [["console", mapFramesToTimelineEntries], ["shared", mapFramesToTimelineEntriesShared]] as const) {
  test(`authorization feedback ${name} sanitizes reconstructed history tools without changing completion`, () => {
    const base = { sourceKind: "session_history", identity: "agent", runId: "run", sessionId: "session" };
    const entries = project({ agent_id: "agent", label: "Agent" }, [
      { ...base, id: "assistant", event: "text_complete", data: { message: { role: "block_assistant", blocks: [
        { block_type: "tool_use", data: { id: "denied", name: "delete", args: {} } },
        { block_type: "tool_use", data: { id: "allowed", name: "read", args: {} } },
      ] } } },
      { ...base, id: "denied-result", event: "tool_result_received", data: { id: "denied", is_error: true, content: [{ type: "text", text: JSON.stringify({ error: "operation_refused", message: "PRIVATE_HISTORY_CANARY" }) }] } },
      { ...base, id: "allowed-result", event: "tool_result_received", data: { id: "allowed", is_error: false, content: [{ type: "text", text: "Actual history result" }] } },
    ]);
    assert.doesNotMatch(JSON.stringify(entries), /PRIVATE_HISTORY_CANARY/);
    assert.match(JSON.stringify(entries), /Actual history result/);
    const tools = entries.flatMap(entry => entry.kind === "message" ? entry.blocks ?? [] : []).filter(block => block.type === "tool-call");
    assert(tools.some(block => block.toolCallId === "denied" && block.status === "error"));
    assert(tools.some(block => block.toolCallId === "allowed" && block.status === "success"));
  });
}

function toolCallBlocksIn(value: unknown): Array<Record<string, unknown>> {
  if (!value || typeof value !== "object") return [];
  if (Array.isArray(value)) return value.flatMap(toolCallBlocksIn);
  const record = value as Record<string, unknown>;
  const own = record.type === "tool-call" ? [record] : [];
  return [...own, ...Object.values(record).flatMap(toolCallBlocksIn)];
}

const SCREENSHOT_CONTENT = [
  { type: "text", text: "Captured the screen." },
  { type: "image", media_type: "image/png", source: "inline", data: "iVBORw0KGgo=" },
  { type: "text", text: "receipt: 1" },
];

for (const [surface, mapTimeline] of [["stock", mapFramesToTimelineEntries], ["shared", mapFramesToTimelineEntriesShared]] as const) {
test(`${surface}: live tool results render their images and keep only text as the result`, () => {
  const agent = { agent_id: "pilot", member_id: "pilot", label: "pilot", kind: "mob_agent" as const };
  const start = { id: "start", event: "tool_execution_started", data: { id: "shot", name: "computer_screenshot", args: {} } };
  // The execution fact carries no body. The result event carries the
  // content and the text-only `result` Meta derives from it.
  const completed = { id: "completed", event: "tool_execution_completed", data: { id: "shot", name: "computer_screenshot", is_error: false, duration_ms: 12 } };
  const received = { id: "received", event: "tool_result_received", data: { id: "shot", name: "computer_screenshot", is_error: false, result: "Captured the screen.\nreceipt: 1", content: SCREENSHOT_CONTENT } };
  // A result event from an older server carries the content only.
  const contentOnly = { ...received, data: { id: "shot", name: "computer_screenshot", is_error: false, content: SCREENSHOT_CONTENT } };
  for (const [order, frames] of [
    ["the execution fact first", [start, completed, received]],
    ["the result event first", [start, received, completed]],
    ["results before their call", [received, completed, start]],
    ["a result event without a derived result", [start, completed, contentOnly]],
  ] as Array<[string, ConsoleFrame[]]>) {
    const entries = mapTimeline(agent, frames);
    const [block] = toolCallBlocksIn(entries).filter((item) => item.toolCallId === "shot");
    assert.ok(block, order);
    assert.equal(block.status, "success", order);
    const images = block.images as Array<Record<string, unknown>>;
    assert.equal(images?.length, 1, order);
    assert.equal(images[0]?.src, "data:image/png;base64,iVBORw0KGgo=");
    assert.equal(typeof block.result, "string", order);
    assert.doesNotMatch(block.result as string, /iVBORw0KGgo/, "no base64 is shown as text");
    assert.match(block.result as string, /Captured the screen\./, order);
  }
});

test(`${surface}: session-history tool results render blob images through the blob route`, () => {
  const agent = { agent_id: "pilot", member_id: "pilot", label: "pilot", kind: "mob_agent" as const };
  const entries = mapTimeline(agent, [
    {
      id: "history-assistant",
      event: "interaction_complete",
      identity: "pilot",
      sourceKind: "session_history",
      data: {
        message: {
          role: "block_assistant",
          blocks: [{ block_type: "tool_use", data: { name: "computer_screenshot", args: {}, id: "shot" } }],
        },
      },
    },
    {
      id: "history-result",
      event: "tool_execution_completed",
      identity: "pilot",
      sourceKind: "session_history",
      data: {
        id: "shot",
        name: "computer_screenshot",
        is_error: false,
        result: "Captured the screen.",
        content: [
          { type: "text", text: "Captured the screen." },
          { type: "image", media_type: "image/png", source: "blob", blob_id: "sha256:abc" },
        ],
      },
    },
  ], { blobBaseUrl: "http://gateway.test/" });
  const [block] = toolCallBlocksIn(entries).filter((item) => item.toolCallId === "shot");
  assert.ok(block);
  const images = block.images as Array<Record<string, unknown>>;
  assert.equal(images.length, 1);
  assert.equal(images[0]?.src, "http://gateway.test/blobs/sha256%3Aabc");
  assert.equal(block.result, "Captured the screen.");
});

test(`${surface}: residual non-image content keeps its canonical formatting beside presented images`, () => {
  const agent = { agent_id: "pilot", member_id: "pilot", label: "pilot", kind: "mob_agent" as const };
  const resource = { type: "resource", uri: "file:///shot.txt", text: "Saved alongside" };
  const structured = { type: "data", value: { width: 1280, height: 720 } };
  const unpresentable = { type: "image", media_type: "image/png" };
  // Live: the presented image leaves the content; text, resource and
  // structured blocks keep the canonical JSON payload.
  const live = mapTimeline(agent, [
    { id: "start", event: "tool_execution_started", data: { id: "shot", name: "computer_screenshot", args: {} } },
    { id: "done", event: "tool_result_received", data: { id: "shot", name: "computer_screenshot", is_error: false,
      content: [{ type: "text", text: "Captured." }, SCREENSHOT_CONTENT[1], resource, structured, unpresentable] } },
  ]);
  const [liveBlock] = toolCallBlocksIn(live).filter((item) => item.toolCallId === "shot");
  assert.ok(liveBlock);
  assert.deepEqual((liveBlock.images as Array<Record<string, unknown>>).map((image) => image.src), ["data:image/png;base64,iVBORw0KGgo="]);
  assert.equal(
    liveBlock.result,
    JSON.stringify([{ type: "text", text: "Captured." }, resource, structured, unpresentable], null, 2),
    "an image that cannot be presented stays in the residual",
  );
  // History: a `result` that is a block list with a stored image.
  const history = mapTimeline(agent, [
    {
      id: "history-assistant",
      event: "interaction_complete",
      identity: "pilot",
      sourceKind: "session_history",
      data: { message: { role: "block_assistant", blocks: [{ block_type: "tool_use", data: { name: "computer_screenshot", args: {}, id: "shot" } }] } },
    },
    {
      id: "history-result",
      event: "tool_execution_completed",
      identity: "pilot",
      sourceKind: "session_history",
      data: { id: "shot", name: "computer_screenshot", is_error: false,
        result: [{ type: "text", text: "Captured." }, { type: "image", media_type: "image/png", source: "blob", blob_id: "sha256:abc" }, resource] },
    },
  ], { blobBaseUrl: "http://gateway.test" });
  const [historyBlock] = toolCallBlocksIn(history).filter((item) => item.toolCallId === "shot");
  assert.ok(historyBlock);
  assert.deepEqual((historyBlock.images as Array<Record<string, unknown>>).map((image) => image.src), ["http://gateway.test/blobs/sha256%3Aabc"]);
  assert.equal(historyBlock.result, JSON.stringify([{ type: "text", text: "Captured." }, resource], null, 2));
});

test(`${surface}: tool results without images keep their existing result text`, () => {
  const agent = { agent_id: "pilot", member_id: "pilot", label: "pilot", kind: "mob_agent" as const };
  const entries = mapTimeline(agent, [
    { id: "start", event: "tool_execution_started", data: { id: "read", name: "read_file", args: {} } },
    { id: "done", event: "tool_result_received", data: { id: "read", name: "read_file", is_error: false, content: [{ type: "text", text: "a" }, { type: "resource", uri: "file:///x" }] } },
  ]);
  const [block] = toolCallBlocksIn(entries).filter((item) => item.toolCallId === "read");
  assert.ok(block);
  assert.equal(block.images, undefined);
  assert.match(block.result as string, /"resource"/, "mixed non-image content keeps its complete payload");
});
}
