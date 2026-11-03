"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertStoppedFrames, assertStoppedHistory, assertCompletedBaseline, assertProofRetained, assertStoppedToolDisclosure, stoppedTools } = require("./real-routine-negative.cjs");

function disclosure({ expanded = "false", status = "success", grouped = false, clickOpens = true } = {}) {
  let clicks = 0;
  const classes = new Set([`cc-tool-call--${status}`, ...(grouped ? ["cc-tool-call--group"] : [])]);
  const header = {
    first() { return this; },
    async getAttribute(name) { assert.equal(name, "aria-expanded"); return expanded; },
    async click() { clicks++; if (clickOpens) expanded = "true"; },
  };
  return {
    locator(selector) { assert.equal(selector, ".cc-tool-call__header"); return header; },
    async evaluate(read) { return read({ classList: { contains: name => classes.has(name) } }); },
    clicks: () => clicks,
  };
}

test("full reload can open a completed standalone call for exact result inspection", async () => {
  const section = disclosure();
  assert.deepEqual(await assertStoppedToolDisclosure(section, "success", { fullReload: true }),
    { expandedBefore: "false", openedAfterReload: true });
  assert.equal(section.clicks(), 1);
  const alreadyOpen = disclosure({ expanded: "true" });
  assert.deepEqual(await assertStoppedToolDisclosure(alreadyOpen, "success", { fullReload: true }),
    { expandedBefore: "true", openedAfterReload: false });
  assert.equal(alreadyOpen.clicks(), 0);
});

test("the stopped tool oracle cannot open unresolved, grouped, or live-transition calls", async () => {
  for (const spec of [
    { state: "success", fullReload: false },
    ...["held", "unknown", "failure"].map(state => ({ state, fullReload: true })),
    { state: "success", fullReload: true, grouped: true },
    ...["pending", "error"].map(status => ({ state: "success", fullReload: true, status })),
  ]) {
    const section = disclosure(spec);
    await assert.rejects(() => assertStoppedToolDisclosure(section, spec.state, { fullReload: spec.fullReload }), /stays disclosed/);
    assert.equal(section.clicks(), 0, JSON.stringify(spec));
  }
});

test("live success and unresolved calls must already be open without a test click", async () => {
  for (const state of ["held", "unknown", "failure", "success"]) {
    const section = disclosure({ expanded: "true" });
    assert.deepEqual(await assertStoppedToolDisclosure(section, state),
      { expandedBefore: "true", openedAfterReload: false });
    assert.equal(section.clicks(), 0);
  }
});

test("a successful reload click must actually reveal the call", async () => {
  const section = disclosure({ clickOpens: false });
  await assert.rejects(() => assertStoppedToolDisclosure(section, "success", { fullReload: true }), /stays disclosed/);
  assert.equal(section.clicks(), 1);
});

function history() {
  const identity = { interaction_id: "earlier-interaction", run_id: "earlier-run" };
  const messages = [
    { role: "user", content: "Keep the prior completed run.\nExact bytes: A\u030A and 🚀.", interaction_id: identity.interaction_id },
    { role: "block_assistant", identity, blocks: [{ block_type: "text", data: { text: "  Prior completed reply.\n" } }] },
  ];
  return { session_id: "session-a", offset: 0, message_count: messages.length, has_more: false, messages };
}
const expected = { scenarioId: "negative-unit", action: "interrupt", runId: "run-a", baselineHistory: history(),
  accepted: { session_id: "session-a", interaction_id: "interaction-a" } };
test("baseline requires an exact completed warmup and its committed owner reply", () => {
  const warmup = { sessionId: "session-a", source: "  Prior completed reply.\n", accepted: { session_id: "session-a", interaction_id: "earlier-interaction" } };
  const terminal = { kind: "interaction_complete", identity: "router:main", session_id: "session-a", interaction_id: "earlier-interaction", run_id: "earlier-run",
    payload: { source_event_type: "run_completed", result: warmup.source }, source: { kind: "console_event" }, id: "warmup-terminal", source_event_id: "warmup-terminal" };
  assert.equal(assertCompletedBaseline(history(), [terminal], warmup).runId, "earlier-run");
  assert.throws(() => assertCompletedBaseline(history(), [], warmup), /exact completed warmup/);
  assert.throws(() => assertCompletedBaseline(history(), [{ ...terminal, payload: { result: "another reply" } }], warmup), /exact completed warmup/);
  assert.throws(() => assertCompletedBaseline(history(), [{ ...terminal, session_id: "foreign" }], warmup), /canonical session/);
  assert.throws(() => assertCompletedBaseline(history(), [{ ...terminal, run_id: "foreign" }], warmup), /one committed warmup reply/);
  const uncommitted = history(); uncommitted.messages.pop(); uncommitted.message_count--;
  assert.throws(() => assertCompletedBaseline(uncommitted, [terminal], warmup), /one committed warmup reply/);
  assert.throws(() => assertCompletedBaseline({ ...history(), has_more: true }, [terminal], warmup), /complete pre-run owner history/);
});
test("warmup pins one core completion while allowing its distinct directed terminal", () => {
  const warmup = { sessionId: "session-a", source: "  Prior completed reply.\n", accepted: { session_id: "session-a", interaction_id: "earlier-interaction" } };
  const core = { kind: "interaction_complete", identity: "router:main", session_id: "session-a", interaction_id: "earlier-interaction", run_id: "earlier-run",
    payload: { source_event_type: "run_completed", result: warmup.source }, source: { kind: "console_event" }, id: "warmup-core", source_event_id: "warmup-core" };
  const directed = { ...core, id: "warmup-directed", source_event_id: "warmup-directed", payload: { source_event_type: "interaction_complete", result: warmup.source } };
  assert.equal(assertCompletedBaseline(history(), [core, directed], warmup).terminalId, core.id);
  assert.equal(assertCompletedBaseline(history(), [directed, core], warmup).terminalId, core.id);
  assert.throws(() => assertCompletedBaseline(history(), [directed], warmup), /exact completed warmup/);
  const unattributed = { ...core, payload: { result: warmup.source } };
  assert.throws(() => assertCompletedBaseline(history(), [unattributed], warmup), /exact completed warmup/);
  assert.throws(() => assertCompletedBaseline(history(), [core, { ...core, id: "second-core", source_event_id: "second-core" }], warmup), /exact completed warmup/);
});
function frames(action = "interrupt") {
  const owner = { identity: "router:main", session_id: "session-a", interaction_id: "interaction-a", run_id: "run-a" };
  let seq = 0;
  return [{ kind: "run_started", payload: {} }, ...stoppedTools(expected.scenarioId).flatMap(tool => [
    { kind: "tool_call_requested", payload: { tool_call_id: tool.id, name: tool.name, args: tool.args } },
    ...(tool.step === "late" && action === "interrupt" ? [] : ["tool_execution_completed", "tool_result_received"].map(kind => ({ kind,
      payload: { tool_call_id: tool.id, name: tool.name, source_event_type: kind, is_error: tool.error,
        ...(kind === "tool_result_received" ? { content: [{ type: "text", text: tool.result }], result: tool.result } : {}) } }))),
  ]), action === "cancel_after_boundary"
    ? { kind: "interaction_failed", payload: { source_event_type: "run_failed", error_report: { class: "cancelled", message: "Stopped" } } }
    : { kind: "interaction_failed", payload: { source_event_type: "interaction_failed", reason: { kind: "cancelled" } } }]
    .map(frame => { const id = `event-${++seq}`; return { ...frame, ...owner, id, source_event_id: id, source: { kind: "console_event" } }; });
}

test("stopped-run oracle distinguishes real cooperative result from absent interrupted completion", () => {
  for (const action of ["interrupt", "cancel_after_boundary"]) {
    const spec = { ...expected, action };
    const proof = assertStoppedFrames(frames(action), spec);
    assert.equal(proof.tools.at(-1).outcome, action === "interrupt" ? "unknown" : "success");
    const wrong = frames(action);
    if (action === "interrupt") wrong.at(-1).payload.reason = { kind: "abandoned", detail: "cancelled" };
    else wrong.at(-1).payload.error_report.class = "internal";
    assert.throws(() => assertStoppedFrames(wrong, spec), /typed .*cancellation/);
    const foreign = frames(action); foreign.at(-1).run_id = "other-run";
    assert.throws(() => assertStoppedFrames(foreign, spec), /canonical run/);
  }
});

test("an interrupted run retains exactly the pre-run committed transcript without same-run prompt or tools", () => {
  const spec = { ...expected, action: "interrupt" };
  assert.deepEqual(assertStoppedHistory(history(), spec), history());
  for (const leaked of [
    { role: "user", content: "Cancelled prompt", interaction_id: spec.accepted.interaction_id },
    { role: "block_assistant", identity: { run_id: spec.runId, interaction_id: spec.accepted.interaction_id },
      blocks: [{ block_type: "tool_use", data: { id: stoppedTools(spec.scenarioId).at(-1).id } }] },
    { role: "tool_results", results: [{ tool_use_id: stoppedTools(spec.scenarioId)[0].id, is_error: false, content: [] }] },
  ]) {
    const bad = history(); bad.messages.push(leaked); bad.message_count++;
    assert.throws(() => assertStoppedHistory(bad, spec), /pre-run committed/);
  }
});

const NOTICE = "A previous run was interrupted before its boundary committed. Recovery preserved every durable message, closed the run as InterruptedByRecovery, and did not requeue its input. Continue from a new turn.";
const cancelled = { ...expected, action: "cancel_after_boundary", sentContent: "Inspect, then hold the delayed review." };
function cancelledHistory() {
  const page = history();
  const identity = { interaction_id: cancelled.accepted.interaction_id, run_id: cancelled.runId };
  const at = "2026-10-24T00:00:00Z";
  const calls = (tools, n) => ({ assistant_message_id: `assistant-${n}`, created_at: at, identity, role: "block_assistant", stop_reason: "tool_use",
    blocks: tools.map(tool => ({ block_type: "tool_use", data: { args: tool.args, id: tool.id, name: tool.name } })) });
  const results = tools => ({ created_at: at, role: "tool_results", results: tools.map(tool => ({ content: tool.result, is_error: tool.error, tool_use_id: tool.id })) });
  const [notes, files, missing, late] = stoppedTools(cancelled.scenarioId);
  page.messages.push({ content: cancelled.sentContent, created_at: at, identity, role: "user" },
    calls([notes, files], 1), results([notes, files]), calls([missing], 2), results([missing]), calls([late], 3), results([late]),
    { body: NOTICE, created_at: at, kind: "generic", role: "system_notice" });
  page.message_count = page.messages.length;
  return page;
}

test("a run cancelled after its held boundary commits its rounds, then the interruption notice, and nothing after", () => {
  const spec = { ...cancelled, baselineHistory: history() };
  assert.deepEqual(assertStoppedHistory(cancelledHistory(), spec), cancelledHistory());
  const reworded = cancelledHistory(); reworded.messages.at(-1).body = "A previous run was interrupted. Continue from a new turn.";
  assert.deepEqual(assertStoppedHistory(reworded, spec), reworded, "the notice is pinned by its kind and stable prefix");
  const ready = require("./real-routine-tools.cjs").expectedTools(spec.scenarioId).at(-1);
  for (const [mutate, message] of [
    [page => { page.messages.splice(2); }, /cancelled run commits its rounds/],
    [page => { page.messages.push({ assistant_message_id: "assistant-5", created_at: "t", identity: page.messages[3].identity, role: "block_assistant",
      blocks: [{ block_type: "text", data: { text: "Done." } }] }); }, /cancelled run/],
    [page => { page.messages.splice(-1, 0, { assistant_message_id: "assistant-4", created_at: "t", identity: page.messages[3].identity, role: "block_assistant", stop_reason: "tool_use",
      blocks: [{ block_type: "tool_use", data: { args: ready.args, id: ready.id, name: ready.name } }] }); }, /cancelled run commits its rounds/],
    [page => { page.messages.pop(); }, /cancelled run/],
    [page => { page.messages.at(-1).kind = "cancelled"; }, /generic recovery notice/],
    [page => { page.messages.at(-1).body = "Stopped."; }, /reads as an interrupted run/],
    [page => { page.messages.at(-1).role = "assistant"; }, /ends with its interruption notice/],
    [page => { page.messages[3].identity = { ...page.messages[3].identity, run_id: "other-run" }; }, /cancelled run commits its rounds/],
    [page => { delete page.messages[4].created_at; }, /commit time/],
    [page => { page.messages[4].assistant_message_id = "assistant-x"; }, /only an assistant row carries a message identity/],
    [page => { page.messages[1].identity = { ...page.messages[1].identity, run_id: "rewritten" }; }, /pre-run committed/],
  ]) {
    const bad = cancelledHistory(); mutate(bad); bad.message_count = bad.messages.length;
    assert.throws(() => assertStoppedHistory(bad, spec), message);
  }
});

test("committed boundary proof rejects lost prior runs, changed bytes or identity, wrong session and incomplete pages", () => {
  for (const mutate of [
    page => { page.messages.pop(); page.message_count--; },
    page => { page.messages[1].blocks[0].data.text = page.messages[1].blocks[0].data.text.trim(); },
    page => { page.messages[1].identity.run_id = "rewritten"; },
    page => { page.messages.reverse(); },
  ]) {
    const bad = history(); mutate(bad);
    assert.throws(() => assertStoppedHistory(bad, expected), /pre-run committed/);
  }
  assert.throws(() => assertStoppedHistory({ ...history(), session_id: "foreign" }, expected), /actual owner session/);
  for (const patch of [{ offset: 1 }, { has_more: true }, { message_count: 99 }]) {
    assert.throws(() => assertStoppedHistory({ ...history(), ...patch }, expected), /complete owner history/);
    assert.throws(() => assertStoppedHistory(history(), { ...expected, baselineHistory: { ...history(), ...patch } }), /complete pre-run owner history/);
  }
  assert.throws(() => assertStoppedHistory(history(), { ...expected, baselineHistory: undefined }), /pre-run owner history was captured/);
  assert.throws(() => assertStoppedHistory(history(), { ...expected, baselineHistory: { ...history(), session_id: "foreign" } }), /pre-run actual owner session/);
});

test("unknown requires absence of authoritative completion, never a result containing unknown prose", () => {
  assertStoppedFrames(frames(), expected);
  for (const kind of ["tool_result_received", "tool_execution_completed"]) {
    const bad = frames();
    const tool = stoppedTools(expected.scenarioId).at(-1);
    bad.splice(-1, 0, { ...bad[1], id: "invented", source_event_id: "invented", kind,
      payload: { tool_call_id: tool.id, is_error: false, content: [{ type: "text", text: "Cancelled, completion unknown" }] } });
    assert.throws(() => assertStoppedFrames(bad, expected), /no authoritative result/);
  }
});

test("cooperative operator cancellation requires its typed agent report without a directed terminal", () => {
  const cooperative = frames("cancel_after_boundary"), spec = { ...expected, action: "cancel_after_boundary" };
  assert.equal(assertStoppedFrames(cooperative, spec).terminalIds.length, 1);
  const noAgent = cooperative.filter(frame => frame.payload.source_event_type !== "run_failed");
  assert.throws(() => assertStoppedFrames(noAgent, spec), /cooperative.*agent cancellation/);
  const hardWithAgent = frames();
  hardWithAgent.splice(-1, 0, { ...cooperative.at(-1), id: "invented-run-failed", source_event_id: "invented-run-failed" });
  assert.throws(() => assertStoppedFrames(hardWithAgent, expected), /hard interruption.*agent cancellation/);
  const wrongAgent = structuredClone(cooperative); wrongAgent.at(-1).payload.error_report.class = "internal";
  assert.throws(() => assertStoppedFrames(wrongAgent, spec), /typed agent cancellation/);
  const untyped = structuredClone(cooperative); untyped.at(-1).payload = { source_event_type: "run_failed", error: "cancelled" };
  assert.throws(() => assertStoppedFrames(untyped, spec), /typed agent cancellation/);
  const duplicate = structuredClone(cooperative); duplicate.push({ ...duplicate.at(-1), id: "second-cancel", source_event_id: "second-cancel" });
  assert.throws(() => assertStoppedFrames(duplicate, spec), /one.*agent cancellation/);
  const falseReason = frames(); falseReason.at(-1).payload = { source_event_type: "run_failed", reason: { kind: "cancelled" } };
  assert.throws(() => assertStoppedFrames(falseReason, expected), /typed directed cancellation/);
});

test("cooperative cancellation rejects a foreign owner or rewritten source identity", () => {
  const spec = { ...expected, action: "cancel_after_boundary" };
  assertStoppedFrames(frames(spec.action), spec);
  for (const [field, value, error] of [
    ["interaction_id", "other-interaction", /agent cancellation/],
    ["run_id", "other-run", /canonical run/],
    ["session_id", "other-session", /canonical session/],
    ["identity", "domain:delivery", /canonical member/],
    ["source_event_id", "rewritten", /original source event ID/],
  ]) {
    const bad = frames(spec.action); bad.at(-1)[field] = value;
    assert.throws(() => assertStoppedFrames(bad, spec), error);
  }
});

test("completed tools retain distinct execution and received result identities", () => {
  for (const action of ["interrupt", "cancel_after_boundary"]) {
    const evidence = frames(action), proof = assertStoppedFrames(evidence, { ...expected, action });
    for (const tool of proof.tools) {
      const completed = evidence.find(frame => frame.kind === "tool_execution_completed" && frame.payload.tool_call_id === tool.id);
      const received = evidence.find(frame => frame.kind === "tool_result_received" && frame.payload.tool_call_id === tool.id);
      assert.equal(tool.completionId, completed?.id ?? null);
      assert.equal(tool.resultId, received?.id ?? null);
      if (completed) assert.notEqual(tool.completionId, tool.resultId);
    }
  }
});

test("each completed tool channel is required exactly once and preserves its body and outcome", () => {
  const spec = { ...expected, action: "cancel_after_boundary" };
  assertStoppedFrames(frames(spec.action), spec);
  for (const kind of ["tool_execution_completed", "tool_result_received"]) {
    for (const tool of stoppedTools(expected.scenarioId)) {
      const select = frame => frame.kind === kind && frame.payload.tool_call_id === tool.id;
      const missing = frames(spec.action).filter(frame => !select(frame));
      assert.throws(() => assertStoppedFrames(missing, spec), /one .*result/);
      const duplicate = frames(spec.action), original = duplicate.find(select);
      duplicate.splice(-1, 0, { ...original, id: "duplicate-result", source_event_id: "duplicate-result" });
      assert.throws(() => assertStoppedFrames(duplicate, spec), /one .*result/);
      const bodyMutations = kind === "tool_result_received" ? [
        frame => { frame.payload.content[0].text += "changed"; },
        frame => { frame.payload.result = "conflicting raw result"; },
      ] : [
        // The execution fact carries no body; the result event carries the output.
        frame => { frame.payload.result = tool.result; },
        frame => { frame.payload.content = [{ type: "text", text: tool.result }]; },
      ];
      for (const mutate of [
        ...bodyMutations,
        frame => { frame.payload.is_error = !tool.error; },
        frame => { frame.payload.name = "wrong_tool"; },
      ]) {
        const bad = frames(spec.action); mutate(bad.find(select));
        assert.throws(() => assertStoppedFrames(bad, spec), /exact result|actual result outcome|exact tool name|carries no result body/);
      }
    }
  }
});

test("each result channel retains source identity, owner and call-to-terminal ordering", () => {
  const spec = { ...expected, action: "cancel_after_boundary" };
  assertStoppedFrames(frames(spec.action), spec);
  for (const kind of ["tool_execution_completed", "tool_result_received"]) {
    const select = frame => frame.kind === kind;
    for (const [field, value, error] of [
      ["source_event_id", "rewritten", /original source event ID/],
      ["interaction_id", "foreign", /canonical interaction/],
      ["run_id", "foreign", /canonical run/],
      ["session_id", "foreign", /canonical session/],
      ["identity", "foreign", /canonical member/],
    ]) {
      const bad = frames(spec.action); bad.find(select)[field] = value;
      assert.throws(() => assertStoppedFrames(bad, spec), error);
    }
    const before = frames(spec.action), [early] = before.splice(before.findIndex(select), 1); before.unshift(early);
    assert.throws(() => assertStoppedFrames(before, spec), /result follows its call/);
    const after = frames(spec.action), [late] = after.splice(after.findIndex(select), 1); after.push(late);
    assert.throws(() => assertStoppedFrames(after, spec), /result precedes the owner terminal/);
  }
  const duplicateId = frames(spec.action), pair = duplicateId.filter(frame => ["tool_execution_completed", "tool_result_received"].includes(frame.kind));
  pair[1].id = pair[0].id; pair[1].source_event_id = pair[0].source_event_id;
  assert.throws(() => assertStoppedFrames(duplicateId, spec), /distinct original tool event IDs/);
});

test("later directed cancellation carrier extends proof without losing original terminal or tool identity", () => {
  const original = assertStoppedFrames(frames(), expected);
  const laterFrames = frames();
  laterFrames.push({ ...laterFrames.at(-1), id: "later-terminal", source_event_id: "later-terminal" });
  const later = assertStoppedFrames(laterFrames, expected);
  assertProofRetained(original, later);
  assert.throws(() => assertProofRetained(original, { ...later, terminalIds: ["later-terminal"] }), /original typed terminal/);
  const rewired = structuredClone(later); rewired.tools[0].callId = "rewritten";
  assert.throws(() => assertProofRetained(original, rewired), /exact tool source identities/);
  for (const field of ["completionId", "resultId"]) {
    const changedResult = structuredClone(later); changedResult.tools[0][field] = "rewritten";
    assert.throws(() => assertProofRetained(original, changedResult), /exact tool source identities/);
  }
});

test("stop proof rejects missing or duplicate calls, mutated bytes, later work, and false ownership", () => {
  const missing = frames().filter(frame => frame.payload.tool_call_id !== stoppedTools(expected.scenarioId).at(-1).id);
  assert.throws(() => assertStoppedFrames(missing, expected), /one call/);
  const duplicate = frames(); duplicate.push({ ...duplicate[1], id: "duplicate", source_event_id: "duplicate" });
  assert.throws(() => assertStoppedFrames(duplicate, expected), /one call/);
  const altered = frames(), alteredResult = altered.find(frame => frame.kind === "tool_result_received");
  alteredResult.payload.content[0].text = alteredResult.payload.content[0].text.trim();
  assert.throws(() => assertStoppedFrames(altered, expected), /exact result/);
  const continued = frames(); continued.push({ ...continued[1], id: "after", source_event_id: "after", payload: { tool_call_id: `fixture-${expected.scenarioId}-ready` } });
  assert.throws(() => assertStoppedFrames(continued, expected), /no later tool/);
});
