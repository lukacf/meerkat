import assert from "node:assert/strict";
import test from "node:test";
import { sanitizeConversationEntries } from "./conversation-visibility";
import type { ConversationTimelineEntry } from "@console-core";

import {
  mapFramesToTimelineEntries as stockMap,
  mergeConversationFrames as stockMerge,
} from "./adapters";
import {
  parseSseFrames as stockParse,
  queryTimeline as stockQuery,
  subscribeTimelineEvents as stockSubscribe,
} from "./network";
import {
  mapFramesToTimelineEntries as sharedMap,
  mergeConversationFrames as sharedMerge,
} from "../../../packages/console-core/src/adapters";
import {
  parseSseFrames as sharedParse,
  queryTimeline as sharedQuery,
  subscribeTimelineEvents as sharedSubscribe,
} from "../../../packages/console-core/src/network";
import { conversationEntryText } from "../../../packages/console-core/src/conversation";
import { reconcileRuntimeAppendFrames } from "../../../packages/console-core/src/runtime-append-projection";
import type { ConsoleFrame } from "../../../packages/console-core/src/runtime-types";

// JSON fixtures exercise existing public APIs before the new upstream types ship.
// The accepted input owns the ordinal in its WHOLE append vector, including roles
// that are not SystemNotice. The source envelope owns session and runtime scope.
const SESSION_A = "01900000-0000-7000-8000-000000000001";
const SESSION_B = "01900000-0000-7000-8000-000000000002";
const RUN_A = "01900000-0000-7000-8000-000000000010";
const RUN_B = "01900000-0000-7000-8000-000000000011";
const INPUT_A = "01900000-0000-7000-8000-000000000020";
const INPUT_B = "01900000-0000-7000-8000-000000000021";
const INTERACTION = "01900000-0000-7000-8000-000000000030";
const TURN_OLD = "01900000-0000-7000-8000-000000000031";
const TURN_MID = "01900000-0000-7000-8000-000000000032";
const TURN_NEW = "01900000-0000-7000-8000-000000000033";
const BODY = "Boundary notice A\u0301 \u{1f680}: preserve both paragraphs.\nSecond paragraph.";
const MODEL_ONLY = "Compatibility model projection, never a second visible notice.";
const agent = {
  agent_id: "router:main", member_id: "router:main", label: "Router", kind: "identity",
};

type Origin = {
  session_id: string;
  run_id: string;
  input_id: string;
  append_ordinal: number;
};
type Notice = {
  kind: string;
  body: string;
  blocks: Array<Record<string, unknown>>;
  created_at: string;
  runtime_origin?: Origin;
};
type WireFrame = {
  id: string;
  kind: string;
  cursor: string;
  identity: string;
  runtime_key: string;
  session_id: string;
  timestamp_ms: number;
  status: string;
  run_id?: string;
  interaction_id?: string;
  source: { kind: string; source_cursor?: string };
  payload: Record<string, unknown>;
};

function notice(origin: Partial<Origin> = {}, body = BODY): Notice {
  return {
    kind: "generic", body, blocks: [], created_at: new Date(900).toISOString(),
    runtime_origin: {
      session_id: SESSION_A, run_id: RUN_A, input_id: INPUT_A, append_ordinal: 0,
      ...origin,
    },
  };
}

function wire(id: string, kind: string, timestamp: number, payload: Record<string, unknown>,
  overrides: Partial<WireFrame> = {}): WireFrame {
  return {
    id, kind, cursor: `console:${timestamp}`, identity: agent.agent_id,
    runtime_key: "runtime-a", session_id: SESSION_A, timestamp_ms: timestamp,
    status: "delivered", source: { kind: "console_event" }, payload, ...overrides,
  };
}

function applied(id: string, notices: Notice[] = [notice()], overrides: Partial<WireFrame> = {}): WireFrame {
  const origin = notices[0]?.runtime_origin;
  return wire(id, "boundary_append_applied", 3000, {
    type: "boundary_append_applied", run_id: origin?.run_id ?? RUN_A,
    input_id: origin?.input_id ?? INPUT_A, content: MODEL_ONLY,
    append_count: Math.max(0, ...notices.map(row => row.runtime_origin?.append_ordinal ?? 0)) + 1,
    transcript_start: 3, notices,
  }, { run_id: origin?.run_id ?? RUN_A, ...overrides });
}

function saved(id: string, message: Notice, offset = 3, overrides: Partial<WireFrame> = {}): WireFrame {
  return wire(id, "system_notice", Date.parse(message.created_at), {
    type: "session_history", source_event_type: "session_history",
    message: { role: "system_notice", ...message },
    kind: message.kind, body: message.body, blocks: message.blocks,
  }, {
    run_id: message.runtime_origin?.run_id, status: "completed",
    source: { kind: "session_history", source_cursor: `${SESSION_A}:${offset}` },
    ...overrides,
  });
}

function discarded(id: string, inputIds = [INPUT_A], runId = RUN_A,
  overrides: Partial<WireFrame> = {}): WireFrame {
  return wire(id, "boundary_appends_discarded", 5000, {
    type: "boundary_appends_discarded", session_id: SESSION_A,
    run_id: runId, input_ids: inputIds,
  }, { run_id: runId, ...overrides });
}

function snapshot(id: string, notices: Array<{ offset: number; message: Notice }> = [],
  settled: Array<{ run_id: string; input_id: string }> = [], overrides: Partial<WireFrame> = {}): WireFrame {
  return wire(id, "runtime_notice_snapshot", 8000, {
    session_id: SESSION_A, complete: true, observed_through: "console:7000",
    notices: notices.map(row => ({ ...row, message: { role: "system_notice", ...row.message } })),
    settled_attempts: settled,
  }, { source: { kind: "session_history" }, ...overrides });
}

function sse(frames: WireFrame[]): string {
  return frames.map(frame => `id: ${frame.cursor}\nevent: console_frame\ndata: ${JSON.stringify({
    type: "console_frame", frame,
  })}\n\n`).join("");
}

function permutations<T>(values: T[]): T[][] {
  if (values.length < 2) return [values];
  return values.flatMap((value, index) => permutations(values.filter((_, other) => other !== index))
    .map(rest => [value, ...rest]));
}

for (const [surface, map, merge, parse, query, subscribe] of [
  ["stock", stockMap, stockMerge, stockParse, stockQuery, stockSubscribe],
  ["shared", sharedMap, sharedMerge, sharedParse, sharedQuery, sharedSubscribe],
] as const) {
  const project = (frames: WireFrame[], restoreOrder = true) => map(agent, parse(sse(frames)), {
    renderInteractionStartsAsUser: restoreOrder, textMode: "markdown",
  });
  const rows = (frames: WireFrame[], body = BODY, restoreOrder = true) => project(frames, restoreOrder)
    .filter(entry => conversationEntryText(entry) === body);

  test(`${surface}: background jobs preserve typed identity, status and exact detail in live and history views`, () => {
    const detail = "  Review A\u030A and <admin>.\nKeep every space.  ";
    for (const status of ["completed", "terminated"]) {
      const typed = { ...notice({}, "Background release review finished."), kind: "background_job",
        blocks: [{ type: "background_job", job_id: "job-1", display_name: "Release review", status, detail, persisted: true }] };
      const expected = { type: "background-job", jobId: "job-1", displayName: "Release review", status,
        detail, copyText: `${detail.trim()}\n${status}` };
      for (const frames of [[applied("job-live", [typed])], [saved("job-saved", typed)],
        [applied("job-live", [typed]), saved("job-saved", typed)]]) {
        const entries = project(frames);
        assert.equal(entries.length, 1);
        assert.deepEqual(entries[0].blocks, [expected]);
        assert.equal(entries[0].identity.role, "system");
        assert.equal(conversationEntryText(entries[0]), expected.copyText);
      }
    }
    const ordinary = project([applied("plain-status", [notice({}, "Background job completed")])]);
    assert(!ordinary[0].blocks?.some(block => block.type === "background-job"), "prose cannot mint typed job status");
  });

  test(`${surface}: typed boundary rows render exact Unicode once without model projection text`, () => {
    for (const restoreOrder of [false, true]) {
      const entries = project([applied("live-a")], restoreOrder);
      assert.deepEqual(entries.map(conversationEntryText), [BODY]);
      assert.ok(entries[0]?.id, "the live notice has a usable DOM row key");
      assert.notEqual(entries[0]?.identity.role, "user", "a Generic notice is not an operator message");
    }
  });

  test(`${surface}: typed notice blocks render through the existing canonical notice renderer`, () => {
    const typed = notice({}, "Typed boundary body.");
    typed.blocks = [{ type: "runtime_notice", category: "delivery", detail: "Use the second attachment." }];
    const expected = project([saved("saved-typed", typed)]).map(conversationEntryText);
    assert.ok(expected.some(text => text.includes("Use the second attachment.")));
    assert.deepEqual(project([applied("live-typed", [typed])]).map(conversationEntryText), expected);
    assert.equal(project([applied("live-typed", [typed]), saved("saved-typed", typed)]).length, 1);
  });

  test(`${surface}: live and canonical history share one stable row key in every arrival order`, () => {
    const live = applied("live-a");
    const history = saved("history-a", notice());
    const expected = rows([live]);
    assert.equal(expected.length, 1);
    for (const frames of [[history], [live, history], [history, live], [live, history, live, history]]) {
      const actual = rows(frames);
      assert.equal(actual.length, 1, "exact provenance joins live, replay, and saved history");
      assert.equal(actual[0].id, expected[0].id, "history handoff and cold reload do not remount the row");
    }
  });

  test(`${surface}: byte-identical notices keep input and whole-vector ordinal identity`, () => {
    const first = notice({ append_ordinal: 1 });
    const second = notice({ append_ordinal: 3 });
    const other = notice({ input_id: INPUT_B, append_ordinal: 0 });
    const live = [applied("live-many", [first, second]), applied("live-other", [other])];
    const before = rows(live);
    assert.equal(before.length, 3);
    assert.equal(new Set(before.map(row => row.id)).size, 3, "body equality cannot collapse accepted appends");
    const after = rows([...live, saved("h-first", first, 4), saved("h-second", second, 6), saved("h-other", other, 7)]);
    assert.deepEqual(new Set(after.map(row => row.id)), new Set(before.map(row => row.id)));
    assert.equal(after.length, 3, "notices do not duplicate the User/InjectedContext gaps in their ordinals");
  });

  test(`${surface}: exact notice origins prevent existing comms text heuristics from collapsing distinct inputs`, () => {
    const peerText = "Peer message from domain:delivery: Delivery accepted.";
    const first = notice({}, peerText);
    const second = notice({ input_id: INPUT_B }, peerText);
    const entries = project([applied("first", [first]), saved("second", second, 4)]);
    assert.equal(entries.length, 2);
    assert.notEqual(entries[0].id, entries[1].id);
  });

  test(`${surface}: committed typed content replaces its exact live twin without matching text`, () => {
    const live = applied("live-a");
    const canonicalBody = "Canonical saved notice content.";
    const history = saved("history-a", notice({}, canonicalBody));
    const key = rows([live])[0]?.id;
    assert.ok(key);
    for (const frames of [[live, history], [history, live]]) {
      const entries = project(frames);
      assert.deepEqual(entries.map(conversationEntryText), [canonicalBody]);
      assert.equal(entries[0]?.id, key);
    }
  });

  test(`${surface}: replaying one transport event does not grow the logical notice projection`, () => {
    const frames = parse(sse([applied("live-a")]));
    const merged = merge(frames, frames, frames);
    const entries = map(agent, merged, { textMode: "markdown" });
    assert.deepEqual(entries.map(conversationEntryText), [BODY]);
    assert.equal(entries[0]?.id, rows([applied("live-a")])[0]?.id);
  });

  test(`${surface}: discard removes only the exact old application and stays out of transcript prose`, () => {
    const other = notice({ input_id: INPUT_B });
    const entries = project([applied("live-a"), applied("live-b", [other]), discarded("discard-a")]);
    assert.equal(rows([applied("live-a"), discarded("discard-a")]).length, 0);
    assert.deepEqual(entries.map(conversationEntryText), [BODY]);
    assert.equal(entries[0]?.id, rows([applied("live-b", [other])])[0]?.id);
  });

  test(`${surface}: late run-A discard cannot remove run-B retry before or after its history arrives`, () => {
    const first = applied("run-a-applied");
    const retryNotice = notice({ run_id: RUN_B });
    const retry = applied("run-b-applied", [retryNotice], { timestamp_ms: 6000, cursor: "console:6000" });
    const lateDiscard = discarded("run-a-late-discard", [INPUT_A], RUN_A, {
      timestamp_ms: 7000, cursor: "console:7000",
    });
    const history = saved("run-b-saved", retryNotice, 8);
    const originalKey = rows([first])[0]?.id;
    assert.ok(originalKey);
    for (const tail of [[retry, lateDiscard], [lateDiscard, retry], ...permutations([retry, lateDiscard, history])]) {
      const actual = rows([first, ...tail]);
      assert.equal(actual.length, 1, "discard scope is original run, not the whole accepted input");
      assert.equal(actual[0].id, originalKey, "a retry preserves logical input/ordinal identity in this session");
    }
  });

  test(`${surface}: positive canonical history remains visible beside a discarded earlier live image`, () => {
    const history = saved("recovered-current-history", notice());
    for (const tail of permutations([history, discarded("prior-live-image-discard")])) {
      const actual = rows([applied("live-a"), ...tail]);
      assert.equal(actual.length, 1, "discard is not authority to delete an actual canonical history row");
      assert.equal(actual[0].id, rows([history])[0]?.id);
    }
  });

  test(`${surface}: latest exact application wins without a discard despite replay order and display time`, () => {
    const first = applied("older-application", [notice({}, "Earlier application.")], {
      cursor: "console:10", timestamp_ms: 9000,
      payload: { ...applied("first").payload, source_sequence: 10,
        notices: [notice({}, "Earlier application.")] },
    });
    const retryNotice = notice({ run_id: RUN_B }, "Current application.");
    const retry = applied("current-application", [retryNotice], {
      cursor: "console:20", timestamp_ms: 100,
      payload: { ...applied("retry", [retryNotice]).payload, source_sequence: 20, transcript_start: 8 },
    });
    for (const withSequence of [true, false]) {
      const attempts = [first, retry].map(frame => ({ ...frame, payload: { ...frame.payload,
        ...(withSequence ? {} : { source_sequence: undefined }) } }));
      for (const input of permutations(attempts)) {
        for (const restoreOrder of [true, false]) {
          const entries = project(input, restoreOrder);
          assert.deepEqual(entries.map(conversationEntryText), [retryNotice.body]);
          assert.equal(entries[0].id, rows([first], "Earlier application.")[0].id);
        }
        const selected = reconcileRuntimeAppendFrames(parse(sse(input)));
        assert.equal(selected[0].runId, RUN_B);
        assert.equal((selected[0].data as { message: Notice }).message.runtime_origin?.run_id, RUN_B);
        assert.equal(selected[0].cursor, "console:20");
      }
    }
    // Source sequence remains authoritative if an old event is replayed into
    // the console store after the current application.
    const replayed = { ...first, cursor: "console:25" };
    assert.deepEqual(project([retry, replayed]).map(conversationEntryText), [retryNotice.body]);
    const replayWithoutSequence = { ...first, id: "legacy-replay", cursor: "console:22",
      payload: { ...first.payload, source_sequence: undefined } };
    for (const input of permutations([replayed, replayWithoutSequence, retry])) {
      assert.deepEqual(project(input).map(conversationEntryText), [retryNotice.body],
        "mixed old envelopes cannot turn source recency into an input-order-dependent comparison");
      assert.equal(reconcileRuntimeAppendFrames(parse(sse(input)))[0].runId, RUN_B);
    }
  });

  test(`${surface}: newest positive canonical observation wins independently of timestamp or array order`, () => {
    const old = saved("old-positive", notice({}, "Old positive."), 3,
      { cursor: "console:10", timestamp_ms: 9000 });
    const current = saved("new-positive", notice({ run_id: RUN_B }, "Current positive."), 8,
      { cursor: "console:20", timestamp_ms: 100 });
    for (const input of permutations([old, current])) {
      assert.deepEqual(project(input).map(conversationEntryText), ["Current positive."]);
    }
    const image = snapshot("older-positive-image", [{ offset: 3, message: notice({}, "Snapshot positive.") }], [], {
      cursor: "console:30", payload: { ...snapshot("template", [{ offset: 3, message: notice({}, "Snapshot positive.") }]).payload,
        observed_through: "console:15" },
    });
    for (const input of permutations([old, current, image])) {
      assert.deepEqual(project(input).map(conversationEntryText), ["Current positive."],
        "snapshot publication time cannot outrank history newer than its observation bound");
    }
  });

  test(`${surface}: failed or completed runs alone do not assert that a notice was discarded`, () => {
    for (const kind of ["run_failed", "run_completed"]) {
      const actual = rows([applied("live-a"), wire("terminal", kind, 5000, {
        run_id: RUN_A,
      }, { run_id: RUN_A })]);
      assert.equal(actual.length, 1);
      assert.equal(actual[0].id, rows([applied("live-a")])[0]?.id);
    }
  });

  test(`${surface}: discard cannot borrow runtime, session, run, or source authority`, () => {
    const badDiscards = [
      discarded("other-runtime", [INPUT_A], RUN_A, { runtime_key: "runtime-b" }),
      discarded("other-session", [INPUT_A], RUN_A, {
        session_id: SESSION_B,
        payload: { type: "boundary_appends_discarded", session_id: SESSION_B, run_id: RUN_A, input_ids: [INPUT_A] },
      }),
      discarded("mismatched-envelope", [INPUT_A], RUN_A, { session_id: SESSION_B }),
      discarded("other-run", [INPUT_A], RUN_B),
      discarded("other-input", [INPUT_B]),
      discarded("synthetic-source", [INPUT_A], RUN_A, { source: { kind: "synthetic" } }),
    ];
    for (const event of badDiscards) {
      const actual = rows([applied("live-a"), event]);
      assert.equal(actual.length, 1, event.id);
    }
  });

  test(`${surface}: a live typed notice requires matching source session, run, input, and valid ordinal`, () => {
    const invalid = [
      applied("wrong-session", [notice()], { session_id: SESSION_B }),
      applied("wrong-run", [notice()], { run_id: RUN_B }),
      applied("wrong-input", [notice({ input_id: INPUT_B })], {
        payload: { ...applied("template").payload, notices: [notice({ input_id: INPUT_B })] },
      }),
      applied("negative-ordinal", [notice({ append_ordinal: -1 })]),
      applied("fractional-ordinal", [notice({ append_ordinal: 0.5 })]),
      applied("unsafe-ordinal", [notice({ append_ordinal: Number.MAX_SAFE_INTEGER + 1 })]),
      applied("synthetic-source", [notice()], { source: { kind: "synthetic" } }),
    ];
    for (const event of invalid) {
      assert.equal(rows([event]).length, 0, event.id);
    }
  });

  test(`${surface}: same logical append in separate runtime/session scopes never shares a DOM key`, () => {
    const first = applied("a");
    const otherSession = applied("other-session", [notice({ session_id: SESSION_B })], { session_id: SESSION_B });
    const otherRuntime = applied("other-runtime", [notice()], { runtime_key: "runtime-b" });
    const actual = rows([first, otherSession, otherRuntime]);
    assert.equal(actual.length, 3);
    assert.equal(new Set(actual.map(row => row.id)).size, 3);
  });

  test(`${surface}: forked parent-origin history remains visible without claiming a child live notice`, () => {
    const parentHistoryInChild = saved("parent-row-in-child", notice(), 3, {
      session_id: SESSION_B,
      source: { kind: "session_history", source_cursor: `${SESSION_B}:3` },
    });
    const childLive = applied("child-live", [notice({ session_id: SESSION_B, run_id: RUN_B })], {
      session_id: SESSION_B,
    });
    const actual = rows([parentHistoryInChild, childLive]);
    assert.equal(actual.length, 2, "a retained parent row is displayable but is not a child application twin");
    assert.notEqual(actual[0].id, actual[1].id);
  });

  test(`${surface}: origin-free legacy rows are preserved without body-based reconciliation`, () => {
    const legacy = notice();
    delete legacy.runtime_origin;
    assert.equal(rows([saved("legacy", legacy), applied("new-live")]).length, 2);
    const oldEvent = applied("old-event", []);
    delete oldEvent.payload.notices;
    delete oldEvent.payload.transcript_start;
    const oldEntries = project([oldEvent]);
    assert.deepEqual(oldEntries, [], "origin-free transport acknowledgements have no display notice");
  });

  test(`${surface}: whole canonical positions order tool, notice, answer even with inverted timestamps`, () => {
    const call = wire("tool-call", "tool_call_requested", 2100, {
      id: "call-ready", tool_call_id: "call-ready", name: "lookup_delivery",
      args: { order_id: "delivery-1" },
    }, { source: { kind: "session_history", source_cursor: `${SESSION_A}:1:tool:0` } });
    const tool = wire("tool-result", "tool_execution_completed", 2000, {
      id: "call-ready", tool_call_id: "call-ready", name: "lookup_delivery",
      result: "Ready from the real tool.", is_error: false,
    }, { source: { kind: "session_history", source_cursor: `${SESSION_A}:2:0` } });
    const history = saved("notice-saved", notice(), 3);
    const answer = wire("answer-saved", "text_complete", 700, {
      result: "Answer after the notice.",
      message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text: "Answer after the notice." } }] },
    }, { run_id: RUN_A, source: { kind: "session_history", source_cursor: `${SESSION_A}:4` } });
    for (const input of permutations([call, tool, history, answer])) {
      const entries = project(input);
      const toolIndex = entries.findIndex(entry => entry.kind === "message"
        && entry.blocks?.some(block => block.type === "tool-call" && block.toolCallId === "call-ready"));
      const noticeIndex = entries.findIndex(entry => conversationEntryText(entry) === BODY);
      const answerIndex = entries.findIndex(entry => conversationEntryText(entry) === "Answer after the notice.");
      assert.ok(toolIndex >= 0 && noticeIndex > toolIndex && answerIndex > noticeIndex,
        `canonical order is independent of arrival and notice lowering time: ${JSON.stringify(entries)}`);
      const toolEntry = entries[toolIndex];
      const toolBlock = toolEntry.kind === "message"
        ? toolEntry.blocks?.find(block => block.type === "tool-call" && block.toolCallId === "call-ready")
        : undefined;
      assert.equal(toolBlock?.type === "tool-call" ? toolBlock.result : undefined, "Ready from the real tool.");
      assert.equal(entries[noticeIndex].createdAt, new Date(900).toISOString(), "ordering must not rewrite source time");
    }
  });

  test(`${surface}: the live insertion position fits between existing canonical rows`, () => {
    const before = saved("before", notice({ input_id: INPUT_B }, "Canonical before."), 2);
    const after = saved("after", notice({ input_id: INPUT_B, append_ordinal: 1 }, "Canonical after."), 4);
    const live = applied("live-a");
    for (const input of permutations([before, live, after])) {
      assert.deepEqual(project(input).map(conversationEntryText), ["Canonical before.", BODY, "Canonical after."]);
    }
  });

  test(`${surface}: a shrinking history image repositions retained tool activity and drops discarded coordinates`, () => {
    const call = wire("retained-call", "tool_call_requested", 2100, {
      id: "retained-tool", name: "lookup_delivery", args: {},
    }, { source: { kind: "session_history", source_cursor: `${SESSION_A}:10:tool:0` } });
    const result = wire("retained-result", "tool_execution_completed", 2000, {
      tool_call_id: "retained-tool", result: "Retained result.", is_error: false,
    }, { source: { kind: "session_history", source_cursor: `${SESSION_A}:11:0` } });
    const oldNotice = saved("retained-notice", notice(), 12);
    const answer = wire("retained-answer", "text_complete", 700, {
      result: "Answer after compaction.",
      message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text: "Answer after compaction." } }] },
    }, { run_id: RUN_A, source: { kind: "session_history", source_cursor: `${SESSION_A}:13` } });
    const audit = wire("discarded-context", "user_input", 4000, { content: "Old audit context." }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:0` },
    });
    const image = snapshot("shrinking-image", [{ offset: 3, message: notice() }]);
    image.payload.history_positions = [
      { frame_id: call.id, source_cursor: `${SESSION_A}:1:tool:0` },
      { frame_id: result.id, source_cursor: `${SESSION_A}:2:0` },
      { frame_id: answer.id, source_cursor: `${SESSION_A}:4` },
    ];
    const reversed = [answer, oldNotice, result, call, audit, image];
    const projected = reconcileRuntimeAppendFrames(parse(sse(reversed)));
    assert.deepEqual(projected.filter(frame => frame.id !== audit.id).map(frame => frame.id), [call.id, result.id,
      rows([oldNotice])[0].id, answer.id], "retained rows use one coherent set of current coordinates");
    assert.ok(projected.findIndex(frame => frame.id === audit.id) > projected.findIndex(frame => frame.id === call.id),
      "discarded audit context no longer uses its old zero offset to precede current history");
    assert.equal(projected.find(frame => frame.id === call.id)?.sourceCursor, `${SESSION_A}:10:tool:0`,
      "position projection leaves the retained source frame evidence intact");
    for (const restoreOrder of [true, false]) {
      const entries = project(reversed, restoreOrder);
      const texts = entries.map(conversationEntryText);
      assert.ok(entries.findIndex(entry => entry.id === call.id) < texts.indexOf(BODY));
      assert.ok(texts.indexOf(BODY) < texts.indexOf("Answer after compaction."));
    }
    const liveCall = { ...call, id: "retained-live-call", cursor: "console:100", source: { kind: "console_event" },
      payload: { ...call.payload, source_sequence: 10 } };
    const liveResult = { ...result, id: "retained-live-result", cursor: "console:101", source: { kind: "console_event" },
      payload: { ...result.payload, source_sequence: 11 } };
    const entries = project([...reversed, liveResult, liveCall]);
    assert.equal(entries.filter(entry => entry.kind === "message"
      && entry.blocks?.some(block => block.type === "tool-call")).length, 1);
    assert.ok(entries.findIndex(entry => entry.id === liveCall.id) < entries.findIndex(entry => conversationEntryText(entry) === BODY),
      "live tool counterparts borrow current coordinates and retain their stable row key");
    const discardedTool = { ...call, id: "discarded-tool-observation", cursor: "console:600",
      source: { kind: "session_history", source_cursor: `${SESSION_A}:0:tool:0` } };
    const withDiscardedTool = reconcileRuntimeAppendFrames(parse(sse([
      answer, oldNotice, result, call, image, discardedTool,
    ])));
    assert.ok(withDiscardedTool.findIndex(frame => frame.id === discardedTool.id)
      > withDiscardedTool.findIndex(frame => frame.id === result.id),
      "unmapped audit rows cannot regain canonical position by borrowing a current tool counterpart");
  });

  test(`${surface}: current history positions leave newer frames and legacy snapshots compatible`, () => {
    const old = wire("old-answer", "text_complete", 800, { result: "Mapped answer.",
      message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text: "Mapped answer." } }] },
    }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:9` },
    });
    const newer = wire("new-answer", "text_complete", 100, { result: "Newer history.",
      message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text: "Newer history." } }] },
    }, {
      cursor: "console:7100", source: { kind: "session_history", source_cursor: `${SESSION_A}:2` },
    });
    const image = snapshot("position-image", [{ offset: 3, message: notice() }]);
    image.payload.history_positions = [{ frame_id: old.id, source_cursor: `${SESSION_A}:4` },
      { frame_id: newer.id, source_cursor: `${SESSION_A}:8` }];
    assert.deepEqual(project([old, image, newer]).map(conversationEntryText), ["Newer history.", BODY, "Mapped answer."]);
    const before = { ...old, source: { kind: "session_history", source_cursor: `${SESSION_A}:1` } };
    const legacy = snapshot("legacy-position-image", [{ offset: 3, message: notice() }]);
    assert.deepEqual(project([legacy, before]).map(conversationEntryText), ["Mapped answer.", BODY]);
  });

  test(`${surface}: shrinking history also rebases ordinary notices before durable notices`, () => {
    const generic = notice({}, "Retained ordinary notice.");
    delete generic.runtime_origin;
    const ordinary = saved("ordinary-notice", generic, 10);
    const durable = saved("durable-notice", notice(), 11);
    const answer = wire("ordinary-tail-answer", "text_complete", 100, {
      result: "Answer after both notices.",
      message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text: "Answer after both notices." } }] },
    }, { run_id: RUN_A, source: { kind: "session_history", source_cursor: `${SESSION_A}:12` } });
    const image = snapshot("ordinary-shrinking-image", [{ offset: 2, message: notice() }]);
    image.payload.history_positions = [
      { frame_id: ordinary.id, source_cursor: `${SESSION_A}:1` },
      { frame_id: answer.id, source_cursor: `${SESSION_A}:3` },
    ];
    for (const input of permutations([ordinary, durable, answer, image])) {
      for (const restoreOrder of [false, true]) {
        assert.deepEqual(project(input, restoreOrder).map(conversationEntryText),
          [generic.body, BODY, "Answer after both notices."]);
      }
    }
    image.payload.history_positions = [{ frame_id: answer.id, source_cursor: `${SESSION_A}:3` }];
    const unmatched = reconcileRuntimeAppendFrames(parse(sse([ordinary, durable, answer, image])));
    assert.equal(unmatched[0].id, ordinary.id, "an unmapped ordinary audit notice has no remaining old-position constraint");
  });

  test(`${surface}: sparse history positions preserve unchanged coordinates and explicitly restore removed coordinates`, () => {
    const unchanged = wire("unchanged-row", "user_input", 100, { content: "Unchanged input." }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:1` },
    });
    const moved = wire("moved-row", "user_input", 200, { content: "Moved input." }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:10` },
    });
    const removed = wire("removed-row", "user_input", 300, { content: "Retained audit input." }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:0` },
    });
    const image = snapshot("sparse-image", [{ offset: 3, message: notice() }]);
    image.payload.history_positions_mode = "sparse";
    image.payload.history_positions = [{ frame_id: moved.id, source_cursor: `${SESSION_A}:2` }];
    image.payload.removed_history_frame_ids = [removed.id];
    for (const input of permutations([image, moved, unchanged])) {
      assert.deepEqual(project(input).map(conversationEntryText), ["Unchanged input.", "Moved input.", BODY]);
    }
    const projected = reconcileRuntimeAppendFrames(parse(sse([image, moved, unchanged, removed])));
    assert.deepEqual(projected.filter(frame => frame.id !== removed.id).map(frame => frame.id),
      [unchanged.id, moved.id, rows([saved("n", notice())])[0].id]);
    assert.ok(projected.findIndex(frame => frame.id === removed.id) > projected.findIndex(frame => frame.id === moved.id),
      "explicit removal drops its old zero coordinate while preserving audit content in source order");
    const restored = snapshot("restored-image", [{ offset: 3, message: notice() }], [], { cursor: "console:9000" });
    restored.payload.history_positions_mode = "sparse";
    restored.payload.history_positions = [{ frame_id: removed.id, source_cursor: `${SESSION_A}:0` }];
    restored.payload.removed_history_frame_ids = [];
    assert.deepEqual(reconcileRuntimeAppendFrames(parse(sse([image, unchanged, removed, restored])))
      .map(frame => frame.id), [removed.id, unchanged.id, rows([saved("n", notice())])[0].id],
      "an explicit sparse row restores its original coordinate");
    assert.equal(projected.find(frame => frame.id === moved.id)?.sourceCursor, `${SESSION_A}:10`);
  });

  test(`${surface}: sparse notice refreshes preserve full compaction positions and keep latest notice authority`, () => {
    const history = (id: string, offset: number, cursor: number) => wire(id, "user_input", cursor, { content: id }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:${offset}` },
    });
    const removed = history("removed", 0, 10);
    const kept = history("kept", 10, 20);
    const appended = history("appended", 2, 150);
    const compacted = snapshot("full-compaction", [{ offset: 0, message: notice({}, "Old notice body") }], [], { cursor: "console:110" });
    compacted.payload.observed_through = "console:100";
    compacted.payload.history_positions = [{ frame_id: kept.id, source_cursor: `${SESSION_A}:1` }];
    const refreshed = snapshot("sparse-refresh", [{ offset: 0, message: notice({}, "Latest notice body") }], [], { cursor: "console:210" });
    Object.assign(refreshed.payload, { observed_through: "console:200", history_positions_mode: "sparse", history_positions: [], removed_history_frame_ids: [] });
    const noticeId = rows([saved("n", notice())])[0].id;
    const ids = (frames: WireFrame[]) => reconcileRuntimeAppendFrames(parse(sse(frames))).map(row => row.id);
    for (const images of [[compacted, refreshed], [refreshed, compacted]]) {
      assert.deepEqual(ids([appended, kept, removed, ...images]), [removed.id, noticeId, kept.id, appended.id],
        "an empty sparse notice refresh cannot reset moved or removed coordinates from the full image");
      assert.deepEqual(project([compacted, refreshed]).map(conversationEntryText), ["Latest notice body"]);
    }
    const restored = snapshot("full-restoration", [{ offset: 0, message: notice() }], [], { cursor: "console:310" });
    restored.payload.observed_through = "console:300";
    restored.payload.history_positions = [removed, kept, appended].map((row, index) => ({ frame_id: row.id, source_cursor: `${SESSION_A}:${index + 1}` }));
    assert.deepEqual(ids([appended, kept, removed, compacted, refreshed, restored]), [noticeId, removed.id, kept.id, appended.id]);
  });

  test(`${surface}: successive full compaction maps preserve removals with latest-only replay and explicit restoration`, () => {
    const history = (id: string, offset: number, cursor: number) => wire(id, "user_input", cursor, { content: id }, {
      source: { kind: "session_history", source_cursor: `${SESSION_A}:${offset}` },
    });
    const first = history("first", 1, 10);
    const second = history("second", 2, 20);
    const full = (id: string, cursor: number, positions: Array<[WireFrame, number]>) => {
      const image = snapshot(id, [{ offset: 0, message: notice() }], [], { cursor: `console:${cursor}` });
      image.payload.observed_through = "console:100";
      image.payload.history_positions = positions.map(([row, offset]) => ({ frame_id: row.id, source_cursor: `${SESSION_A}:${offset}` }));
      return image;
    };
    // These are the native successive_compactions producer's exact shapes.
    const compactedOnce = full("compaction-1", 110, [[second, 1]]);
    const compactedTwice = full("compaction-2", 120, []);
    const unchanged = full("compaction-unchanged", 130, []);
    const restored = full("compaction-restored", 140, [[first, 1], [second, 2]]);
    const noticeId = rows([saved("n", notice())])[0].id;
    const ids = (input: WireFrame[]) => reconcileRuntimeAppendFrames(parse(sse(input))).map(row => row.id);
    assert.deepEqual(ids([second, first, compactedOnce]), [first.id, noticeId, second.id]);
    for (const images of [[compactedTwice], [compactedOnce, compactedTwice], [compactedTwice, compactedOnce],
      [compactedOnce, compactedTwice, unchanged], [unchanged]]) {
      assert.deepEqual(ids([second, first, ...images]), [second.id, first.id, noticeId],
        "the latest complete survivor map cannot resurrect coordinates removed by an earlier compaction");
      assert.deepEqual(project([second, first, ...images], false).map(entry => entry.id), [second.id, first.id, noticeId]);
    }
    for (const images of [[restored], [compactedOnce, compactedTwice, restored], [restored, compactedTwice, compactedOnce]]) {
      assert.deepEqual(ids([second, first, ...images]), [noticeId, first.id, second.id],
        "only explicit full-map survivors restore the original source coordinates");
    }
    const newer = history("newer-than-boundary", 3, 150);
    assert.deepEqual(ids([newer, second, first, restored]), [noticeId, first.id, second.id, newer.id],
      "the snapshot cannot erase a newer unlisted canonical row");
    for (const image of [compactedOnce, compactedTwice, unchanged, restored]) {
      assert.equal(image.payload.history_positions_mode, undefined);
      assert.equal(image.payload.removed_history_frame_ids, undefined);
    }
  });

  test(`${surface}: malformed sparse position metadata cannot invalidate current notice evidence`, () => {
    for (const fields of [
      { history_positions_mode: "future", removed_history_frame_ids: [] },
      { history_positions_mode: "sparse" },
      { history_positions_mode: "sparse", removed_history_frame_ids: [""] },
      { history_positions_mode: "sparse", removed_history_frame_ids: ["row", "row"] },
      { history_positions_mode: "sparse", removed_history_frame_ids: ["row"], history_positions: [{ frame_id: "row", source_cursor: `${SESSION_A}:2` }] },
      { removed_history_frame_ids: ["row"] },
    ]) {
      const image = snapshot("invalid-sparse", [], [{ run_id: RUN_A, input_id: INPUT_A }]);
      Object.assign(image.payload, { history_positions: [] }, fields);
      assert.equal(rows([applied("live-sparse"), image]).length, 1, JSON.stringify(fields));
    }
  });

  test(`${surface}: malformed current history positions cannot authorize snapshot replacement`, () => {
    const invalid = [null, {}, [{ frame_id: "", source_cursor: `${SESSION_A}:1` }],
      [{ frame_id: "history", source_cursor: `${SESSION_B}:1` }],
      [{ frame_id: "history", source_cursor: `${SESSION_A}:-1` }],
      [{ frame_id: "history", source_cursor: `${SESSION_A}:1::0` }],
      [{ frame_id: "history", source_cursor: `${SESSION_A}:1:9007199254740992` }],
      [{ frame_id: "history", source_cursor: `${SESSION_A}:1` }, { frame_id: "history", source_cursor: `${SESSION_A}:2` }],
    ];
    for (const history_positions of invalid) {
      const image = snapshot("bad-position-image");
      image.payload.history_positions = history_positions;
      assert.deepEqual(project([saved("history", notice()), image]).map(conversationEntryText), [BODY],
        JSON.stringify(history_positions));
    }
  });

  test(`${surface}: canonical offsets are parsed numerically rather than compared as cursor strings`, () => {
    const before = saved("offset-2", notice({ input_id: INPUT_B }, "Second row."), 2);
    const after = saved("offset-10", notice({ input_id: INPUT_B, append_ordinal: 1 }, "Tenth row."), 10);
    for (const input of permutations([before, after])) {
      assert.deepEqual(project(input).map(conversationEntryText), ["Second row.", "Tenth row."]);
    }
  });

  test(`${surface}: canonical notice keeps its live source sequence between unpositioned tool and answer rows`, () => {
    const call = wire("live-call", "tool_call_requested", 8000, {
      id: "call-sequence", name: "lookup_delivery", args: {}, source_sequence: 18,
    });
    const result = wire("live-result", "tool_execution_completed", 7000, {
      tool_call_id: "call-sequence", result: "Delivery is ready.", is_error: false, source_sequence: 19,
    });
    const live = applied("live-notice", [notice()], {
      timestamp_ms: 6000, cursor: "console:6000",
      payload: { ...applied("template").payload, source_sequence: 20 },
    });
    const answer = wire("live-answer", "text_delta", 100, {
      delta: "Answer after live notice.", source_sequence: 21,
    }, { run_id: RUN_A, interaction_id: INTERACTION });
    const history = saved("saved-notice", notice());
    const expected = project([call, result, live, answer]);
    assert.equal(expected.length, 3);
    assert.equal(expected[0].id, "live-call");
    assert.equal(conversationEntryText(expected[1]), BODY);
    assert.equal(conversationEntryText(expected[2]), "Answer after live notice.");
    for (const input of [[history, answer, live, result, call], [answer, call, history, result, live]]) {
      assert.deepEqual(project(input), expected, "history retains the application source sequence as its live anchor");
    }
  });

  test(`${surface}: retained canonical user and tool anchors preserve one live row and its original key`, () => {
    const user = wire("live-user", "user_input", 200, {
      content: "Review delivery.",
    }, { interaction_id: INTERACTION });
    const userHistory = wire("saved-user", "user_input", 100, {
      content: "Review delivery.", message: { role: "user", content: "Review delivery." },
    }, { interaction_id: INTERACTION, source: { kind: "session_history", source_cursor: `${SESSION_A}:0` } });
    const call = wire("live-provider-call", "tool_call_requested", 600, {
      id: "provider-call-id", name: "lookup_delivery", args: {}, source_sequence: 10,
    });
    const result = wire("live-provider-result", "tool_execution_completed", 500, {
      tool_call_id: "provider-call-id", result: "Delivery is ready.", is_error: false, source_sequence: 11,
    });
    const callHistory = wire("saved-provider-call", "tool_call_requested", 300, {
      id: "provider-call-id", name: "lookup_delivery", args: {},
    }, { source: { kind: "session_history", source_cursor: `${SESSION_A}:1:tool:0` } });
    const resultHistory = wire("saved-provider-result", "tool_execution_completed", 400, {
      tool_call_id: "provider-call-id", result: "Delivery is ready.", is_error: false,
    }, { source: { kind: "session_history", source_cursor: `${SESSION_A}:2:0` } });
    const history = saved("saved-notice", notice(), 3);
    const baseline = project([user, call, result]);
    for (const input of [[history, callHistory, userHistory, resultHistory, user, result, call],
      [call, user, result, resultHistory, userHistory, history, callHistory]]) {
      const entries = project(input);
      const visible = (items: typeof entries) => items.map(entry => ({
        id: entry.id, identity: entry.identity, text: conversationEntryText(entry),
      }));
      assert.deepEqual(visible(entries.filter(entry => conversationEntryText(entry) !== BODY)), visible(baseline));
      assert.equal(entries[0].id, "live-user");
      assert.equal(entries[1].id, "live-provider-call");
      assert.equal(conversationEntryText(entries[2]), BODY);
    }
  });

  test(`${surface}: preserved user, assistant, and typed peer rows coexist with a mixed-role boundary`, () => {
    const user = wire("operator", "user_input", 100, { content: "Check the delivery plan." }, { interaction_id: INTERACTION });
    const assistant = wire("assistant", "text_delta", 200, { delta: "Checking now." }, {
      interaction_id: INTERACTION, run_id: RUN_A,
    });
    const peer = wire("peer", "system_notice", 300, { message: {
      role: "system_notice", kind: "comms", body: "Delivery accepted.",
      blocks: [{ type: "comms", kind: "message", direction: "incoming",
        peer: { id: "domain:delivery", display_name: "Delivery" },
        request_id: "delivery-reply", content: [{ type: "text", text: "Delivery accepted." }] }],
    } });
    const ordinary = [user, assistant, peer];
    const baseline = project(ordinary);
    const withBoundary = project([...ordinary, applied("notice", [notice({ append_ordinal: 2 })])]);
    assert.deepEqual(withBoundary.filter(entry => conversationEntryText(entry) !== BODY), baseline,
      "new notice reconciliation must not relabel or alter existing row keys, roles, content, or comms blocks");
    assert.equal(withBoundary.filter(entry => entry.identity.role === "user").length, 1);
    assert.equal(withBoundary.filter(entry => conversationEntryText(entry) === BODY).length, 1);
  });

  test(`${surface}: canonical source cursor and typed notice survive SSE and JSON query normalization`, async () => {
    const frames = [saved("saved", notice(), 3), applied("live"), discarded("discard")];
    const fromSse = parse(sse(frames));
    assert.equal((fromSse[0] as ConsoleFrame & { sourceCursor?: string }).sourceCursor, `${SESSION_A}:3`);
    for (let index = 0; index < frames.length; index++) {
      assert.equal(fromSse[index].sessionId, frames[index].session_id);
      assert.equal(fromSse[index].runtimeKey, frames[index].runtime_key);
      assert.deepEqual(fromSse[index].data, frames[index].payload);
    }
    const originalFetch = globalThis.fetch;
    globalThis.fetch = (async () => Response.json({ jsonrpc: "2.0", id: "test", result: {
      frames, available: true, exhausted: true, next_cursor: "console:5000",
    } })) as typeof fetch;
    try {
      const page = await query("http://fixture.test", { identity: agent.agent_id }, 400);
      assert.equal(page.available, true);
      assert.deepEqual(page.frames, fromSse);
    } finally { globalThis.fetch = originalFetch; }
  });

  test(`${surface}: an empty partial history page preserves the current live notice`, async () => {
    const originalFetch = globalThis.fetch;
    const live = parse(sse([applied("live-a")]));
    globalThis.fetch = (async () => Response.json({ jsonrpc: "2.0", id: "test", result: {
      frames: [], available: true, exhausted: false, next_cursor: "console:10",
    } })) as typeof fetch;
    try {
      const page = await query("http://fixture.test", { identity: agent.agent_id }, 1);
      assert.equal(page.exhausted, false);
      const entries = map(agent, merge(live, page.frames), { textMode: "markdown" });
      assert.deepEqual(entries.map(conversationEntryText), [BODY]);
    } finally { globalThis.fetch = originalFetch; }
  });

  test(`${surface}: history request failure cannot become a synthetic discard frame`, async () => {
    const originalFetch = globalThis.fetch;
    const live = parse(sse([applied("live-a")]));
    const before = map(agent, live, { textMode: "markdown" });
    assert.deepEqual(before.map(conversationEntryText), [BODY]);
    globalThis.fetch = (async () => new Response("History temporarily unavailable", { status: 503 })) as typeof fetch;
    try {
      await assert.rejects(query("http://fixture.test", { identity: agent.agent_id }, 10), /503/);
      assert.deepEqual(map(agent, live, { textMode: "markdown" }), before);
    } finally { globalThis.fetch = originalFetch; }
  });

  test(`${surface}: incremental SSE subscription retains canonical position and exact typed content`, async () => {
    const row = saved("streamed-history", notice(), 3);
    const originalFetch = globalThis.fetch;
    const encoder = new TextEncoder();
    let stop = () => {};
    globalThis.fetch = (async () => new Response(new ReadableStream<Uint8Array>({
      start(controller) { controller.enqueue(encoder.encode(sse([row]))); },
    }))) as typeof fetch;
    try {
      const received = await new Promise<ConsoleFrame>((resolve, reject) => {
        const deadline = setTimeout(() => reject(new Error("subscription did not deliver the fixture frame")), 1000);
        stop = subscribe("http://fixture.test", { identity: agent.agent_id }, frame => {
          clearTimeout(deadline);
          resolve(frame);
        });
      });
      assert.equal((received as ConsoleFrame & { sourceCursor?: string }).sourceCursor, `${SESSION_A}:3`);
      assert.deepEqual(received.data, row.payload);
    } finally { stop(); globalThis.fetch = originalFetch; }
  });

  test(`${surface}: nested frame updates preserve canonical source cursor on the updated row`, () => {
    const row = saved("updated-history", notice(), 3);
    const update = wire("update-envelope", "frame_updated", 8000, { frame: row }, {
      source: { kind: "synthetic" },
    });
    const [frame] = parse(sse([update]));
    assert.equal(frame.event, "frame_updated");
    const updated = (frame.data as { frame: ConsoleFrame & { sourceCursor?: string } }).frame;
    assert.equal(updated.sourceCursor, `${SESSION_A}:3`);
    assert.equal(updated.sessionId, SESSION_A);
    assert.deepEqual(updated.data, row.payload);
  });

  test(`${surface}: a complete current notice snapshot replaces old canonical rows and never renders itself`, () => {
    const old = saved("old-history", notice());
    const current = notice({ input_id: INPUT_B }, "Current canonical notice.");
    const image = snapshot("current-image", [{ offset: 2, message: current }]);
    for (const frames of [[old, image], [image, old]]) {
      assert.deepEqual(project(frames).map(conversationEntryText), [current.body]);
    }
    assert.deepEqual(project([old, snapshot("complete-empty")]), []);
  });

  test(`${surface}: latest valid snapshot uses its console cursor, independent of arrival and display time`, () => {
    const old = snapshot("old-image", [{ offset: 3, message: notice() }]);
    const empty = snapshot("new-empty", [], [], { cursor: "console:9000", timestamp_ms: 100 });
    for (const frames of [[old, empty], [empty, old]]) assert.deepEqual(project(frames), []);
    const current = notice({}, "Current canonical notice.");
    const newer = snapshot("new-current", [{ offset: 1, message: current }], [], {
      cursor: "console:10000", timestamp_ms: 90,
    });
    for (const frames of permutations([old, empty, newer])) {
      assert.deepEqual(project(frames).map(conversationEntryText), [current.body]);
    }
  });

  test(`${surface}: a settled missing old application converges after a missed discard event`, () => {
    const image = snapshot("current-empty", [], [{ run_id: RUN_A, input_id: INPUT_A }]);
    assert.deepEqual(project([applied("old-live"), image]), []);
    assert.equal(rows([applied("old-live"), snapshot("active-missing")]).length, 1,
      "absence from committed history alone cannot discard a still-running application");
    assert.equal(rows([applied("old-live"), snapshot("other-settled", [], [{ run_id: RUN_B, input_id: INPUT_A }])]).length, 1);
  });

  test(`${surface}: complete snapshots cannot erase later live events or later canonical history`, () => {
    const image = snapshot("older-observation", [], [{ run_id: RUN_A, input_id: INPUT_A }]);
    const live = applied("newer-live", [notice()], { cursor: "console:7100", timestamp_ms: 7100 });
    const history = saved("newer-canonical", notice(), 5, { cursor: "console:7200", timestamp_ms: 7200 });
    assert.equal(rows([image, live]).length, 1);
    assert.equal(rows([image, history]).length, 1);
    assert.equal(rows([image, live, history]).length, 1);
  });

  test(`${surface}: current positive snapshot notice wins over an exact settled or discarded attempt`, () => {
    const image = snapshot("current-positive", [{ offset: 3, message: notice() }], [{ run_id: RUN_A, input_id: INPUT_A }]);
    for (const frames of permutations([applied("live"), discarded("old-discard"), image])) {
      const actual = rows(frames);
      assert.equal(actual.length, 1);
      assert.equal(actual[0].id, rows([applied("live")])[0]?.id);
    }
  });

  test(`${surface}: a positive settled snapshot keeps the exact live attempt's source sequence`, () => {
    const call = wire("live-call", "tool_call_requested", 8000, {
      id: "settled-call", name: "lookup_delivery", args: {}, source_sequence: 18,
    });
    const result = wire("live-result", "tool_execution_completed", 7000, {
      tool_call_id: "settled-call", result: "Delivery is ready.", is_error: false, source_sequence: 19,
    });
    const live = applied("live-notice", [notice()], {
      timestamp_ms: 6000, cursor: "console:6000",
      payload: { ...applied("template").payload, source_sequence: 20 },
    });
    const answer = wire("live-answer", "text_delta", 100, {
      delta: "Answer after settled notice.", source_sequence: 21,
    }, { run_id: RUN_A, interaction_id: INTERACTION });
    const image = snapshot("committed-positive", [{ offset: 3, message: notice() }],
      [{ run_id: RUN_A, input_id: INPUT_A }]);
    const expected = project([call, result, live, answer]);
    for (const input of [[image, answer, live, result, call],
      [answer, call, image, result, discarded("late-discard"), live]]) {
      assert.deepEqual(project(input), expected,
        "positive current history preserves the confirmed application's order even after settlement or discard");
    }
  });

  test(`${surface}: malformed or partial snapshots grant no canonical or provisional invalidation`, () => {
    const image = snapshot("valid-base");
    const badPayloads: Array<Record<string, unknown>> = [
      { ...image.payload, complete: false },
      { ...image.payload, session_id: SESSION_B },
      { ...image.payload, observed_through: "bad:7000" },
      { ...image.payload, observed_through: "console:9999" },
      { ...image.payload, notices: [{ offset: -1, message: notice() }] },
      { ...image.payload, notices: [{ offset: 1.5, message: notice() }] },
      { ...image.payload, notices: [{ offset: Number.MAX_SAFE_INTEGER + 1, message: notice() }] },
      { ...image.payload, notices: [{ offset: 1, message: { ...notice(), runtime_origin: { ...notice().runtime_origin, run_id: "" } } }] },
      { ...image.payload, notices: [{ offset: 1, message: { ...notice(), role: "user" } }] },
      { ...image.payload, notices: [{ offset: 1, message: notice() }, { offset: 1, message: notice({ input_id: INPUT_B }) }] },
      { ...image.payload, settled_attempts: [{ run_id: RUN_A }] },
      { ...image.payload, settled_attempts: null },
    ];
    const invalid = badPayloads.map((payload, index) => ({ ...image, id: `invalid-${index}`, payload }));
    invalid.push({ ...image, source: { kind: "console_event" } });
    invalid.push({ ...image, cursor: "not-a-console-cursor" });
    for (const bad of invalid) {
      const frames = [saved("old", notice()), applied("live"), bad];
      assert.equal(rows(frames).length, 1, bad.id);
      assert.equal(project(frames).length, 1, "invalid snapshots remain transport details, not prose");
    }
  });

  test(`${surface}: current image replacement preserves legacy and other runtime/session rows`, () => {
    const legacy = notice({}, "Legacy notice.");
    delete legacy.runtime_origin;
    const other = saved("other-runtime", notice({}, "Other runtime."), 3, { runtime_key: "runtime-b" });
    const otherSession = saved("other-session", notice({ session_id: SESSION_B }, "Other session."), 3, {
      session_id: SESSION_B, source: { kind: "session_history", source_cursor: `${SESSION_B}:3` },
    });
    const entries = project([saved("old", notice()), saved("legacy", legacy), other, otherSession, snapshot("empty")]);
    assert.deepEqual(new Set(entries.map(conversationEntryText)), new Set([legacy.body, "Other runtime.", "Other session."]));
  });

  test(`${surface}: malformed canonical notice kinds and typed fields cannot authorize image replacement`, () => {
    const malformed: Array<Record<string, unknown>> = [
      { kind: "unrecognized_notice_kind" },
      { blocks: [{}] },
      { blocks: [{ type: "runtime_notice" }] },
      { blocks: [{ type: "runtime_notice", category: 42 }] },
      { blocks: [{ type: "auth", state: false }] },
      { blocks: [{ type: "background_job", job_id: "job", status: "running" }] },
      { blocks: [{ type: "background_job", job_id: "job", status: "completed", persisted: "true" }] },
      { blocks: [{ type: "background_job", job_id: "job", status: "completed", persisted: "false" }] },
      { blocks: [{ type: "comms", kind: "message", direction: "sideways" }] },
      { blocks: [{ type: "comms", kind: "message", direction: "incoming", peer: { display_name: "missing id" } }] },
      { blocks: [{ type: "comms", kind: "message", direction: "incoming", content: [{ type: "text" }] }] },
      { blocks: [{ type: "external_event", source: "source" }] },
      { blocks: [{ type: "mcp", persisted: "false" }] },
      { blocks: [{ type: "mcp", settled: "false" }] },
      { blocks: [{ type: "mcp", pending_sources: [3] }] },
      { blocks: [{ type: "tool_config", payload: {} }] },
      { blocks: [{ type: "tool_config", payload: { operation: "reload", target: "tools", persisted: true,
        status_info: { kind: "boundary_applied", base_changed: true, visible_changed: false } } }] },
      { blocks: [{ type: "tool_config", payload: { operation: "reload", target: "tools", settled: "true",
        status_info: { kind: "boundary_applied", base_changed: true, visible_changed: false, revision: 2 } } }] },
      { blocks: [{ type: "tool_config", payload: { operation: "reload", target: "tools",
        status_info: { kind: "boundary_applied", base_changed: true, visible_changed: false, revision: 2 } } }] },
      { blocks: [{ type: "unknown", summary: 3 }] },
    ];
    for (const invalid of malformed) {
      const image = snapshot("bad-typed-image", [{ offset: 3, message: {
        ...notice({}, "Replacement."), ...invalid,
      } as Notice }]);
      assert.deepEqual(project([saved("canonical", notice()), applied("live"), image]).map(conversationEntryText), [BODY],
        JSON.stringify(invalid));
    }
  });

  test(`${surface}: snapshot validation retains the upstream notice vocabulary and forward-compatible blocks`, () => {
    const valid: Array<Record<string, unknown>> = [
      { type: "comms", kind: "future_comms_kind", direction: "internal", content: [{ type: "text", text: "content" }] },
      { type: "external_event", source: "source", event_type: "update" },
      { type: "tool_config", payload: { operation: "reload", target: "tools", persisted: true,
        status_info: { kind: "boundary_applied", base_changed: true, visible_changed: false, revision: 2 } } },
      { type: "tool_config", payload: { operation: "reload", target: "tools", settled: true,
        status_info: { kind: "boundary_applied", base_changed: true, visible_changed: false, revision: 2 } } },
      { type: "mcp" },
      { type: "mcp", phase: "applied", settled: true },
      { type: "mcp", phase: "applied", persisted: true },
      { type: "background_job", job_id: "job", status: "terminated" },
      { type: "background_job", job_id: "job", status: "completed", persisted: true },
      { type: "background_job", job_id: "job", status: "completed", persisted: false },
      { type: "auth", state: "reauth_required" },
      { type: "runtime_notice", category: "future_category" },
      { type: "unknown", summary: "preserved future data", payload: { any: true } },
      { type: "future_block_kind", anything: [1, 2] },
    ];
    const kinds = ["generic", "comms", "external_event", "mcp_pending", "mcp", "background_job",
      "tool_scope", "tool_scope_warning", "auth_reauth_required"];
    for (const kind of kinds) for (const block of valid) {
      const image = snapshot("valid-typed-image", [{ offset: 3, message: { ...notice(), kind, blocks: [block] } }]);
      const frames = reconcileRuntimeAppendFrames(parse(sse([image])));
      assert.equal(frames.length, 1, `${kind}: ${JSON.stringify(block)}`);
    }
    const empty = snapshot("empty-valid-notice", [{ offset: 3, message: { ...notice(), body: "", blocks: [] } }]);
    assert.equal(reconcileRuntimeAppendFrames(parse(sse([empty]))).length, 1);
  });

  test(`${surface}: snapshot rows retain parent provenance without claiming a child live attempt`, () => {
    const image = snapshot("child-image", [{ offset: 1, message: notice() }], [], {
      session_id: SESSION_B,
      payload: { ...snapshot("template", [{ offset: 1, message: notice() }]).payload, session_id: SESSION_B },
    });
    const live = applied("child-live", [notice({ session_id: SESSION_B, run_id: RUN_B })], { session_id: SESSION_B });
    const actual = rows([image, live]);
    assert.equal(actual.length, 2);
    assert.notEqual(actual[0].id, actual[1].id);
  });

  // A conversation opens on its newest timeline page, but the notice snapshot
  // on that page carries every notice of the session.
  const exported = { ...notice({ input_id: INPUT_B }, "Nightly export finished."),
    created_at: new Date(5000).toISOString() };
  const imageOf = (id: string, message: Notice, settled: Array<{ run_id: string; input_id: string }> = []) =>
    snapshot(id, [{ offset: 3, message }], settled, {
      cursor: "console:9200", timestamp_ms: 9200,
      payload: { ...snapshot("template", [{ offset: 3, message }], settled).payload, observed_through: "console:9150" },
    });
  const newestPage = [
    wire("asked", "user_input", 9000, { content: "Is the export done?" }, { interaction_id: TURN_NEW }),
    wire("answered", "text_delta", 9100, { delta: "Checking the export." }, { interaction_id: TURN_NEW, run_id: RUN_B }),
    imageOf("newest-image", exported),
  ];
  const olderPage = [
    wire("started", "user_input", 4000, { content: "Start the nightly export." }, { interaction_id: TURN_OLD }),
    applied("exported-live", [exported], { cursor: "console:5100", timestamp_ms: 5100 }),
    wire("nudged", "user_input", 6000, { content: "Tell me when it lands." }, { interaction_id: TURN_MID }),
  ];
  const pageTexts = (frames: WireFrame[], hasOlderHistory: boolean) => map(agent, parse(sse(frames)), {
    renderInteractionStartsAsUser: true, textMode: "markdown", hasOlderHistory,
  }).map(conversationEntryText);

  test(`${surface}: a snapshot notice older than every loaded frame waits while older history is unloaded`, () => {
    assert.deepEqual(pageTexts(newestPage, true), ["Is the export done?", "Checking the export."],
      "an old notice must not sit directly above newer rows with the history between them unloaded");
    assert.deepEqual(pageTexts(newestPage, false), [exported.body, "Is the export done?", "Checking the export."],
      "with no older history to load, the notice is the oldest row of the transcript");
  });

  test(`${surface}: loading older history places the held notice once, in time order`, () => {
    assert.ok(!pageTexts(newestPage, true).includes(exported.body));
    const placed = ["Start the nightly export.", exported.body, "Tell me when it lands.",
      "Is the export done?", "Checking the export."];
    for (const hasOlderHistory of [true, false]) {
      for (const frames of [[...olderPage, ...newestPage], [...newestPage, ...olderPage]]) {
        assert.deepEqual(pageTexts(frames, hasOlderHistory), placed,
          "the older page reaches the notice's time, so it renders once between its neighbours");
      }
    }
    const withoutLiveTwin = [olderPage[0], olderPage[2], ...newestPage];
    assert.deepEqual(pageTexts(withoutLiveTwin, true), pageTexts(withoutLiveTwin, false),
      "a notice inside the loaded range keeps its place whether or not older pages remain");
  });

  test(`${surface}: a held notice whose own application is loaded still renders once`, () => {
    const drained = { ...notice({}, "Review queue drained."), created_at: new Date(8500).toISOString() };
    const page = [
      applied("drained-live", [drained], { cursor: "console:9050", timestamp_ms: 9050 }),
      ...newestPage.slice(0, 2),
      imageOf("settled-image", drained, [{ run_id: RUN_A, input_id: INPUT_A }]),
    ];
    for (const hasOlderHistory of [true, false]) {
      assert.deepEqual(pageTexts(page, hasOlderHistory), [drained.body, "Is the export done?", "Checking the export."],
        "its application frame is loaded, so the notice is inside the loaded history");
    }
  });
}

test("runtime append reconciliation does not mutate source frames or nested notice messages", () => {
  const frames = stockParse(sse([applied("live"), saved("history", notice())]));
  const freeze = (value: unknown) => {
    if (!value || typeof value !== "object" || Object.isFrozen(value)) return;
    Object.freeze(value);
    for (const item of Object.values(value)) freeze(item);
  };
  freeze(frames);
  const before = JSON.stringify(frames);
  const projected = reconcileRuntimeAppendFrames(frames);
  assert.equal(projected.length, 1);
  assert.equal(JSON.stringify(frames), before);
});


test("stock visibility preserves typed job headers and exact details while dropping empty prose", () => {
  for (const detail of ["", "  Preserve A\u030A and <admin>.\nSecond line.  "]) {
    const block = { type: "background-job" as const, jobId: "job-visible", displayName: "Review",
      status: "completed", detail, copyText: detail };
    const job: ConversationTimelineEntry = { id: "job-row", kind: "message", variant: "rich",
      identity: { id: "system", label: "System", role: "system" }, blocks: [block] };
    const blank: ConversationTimelineEntry = { ...job, id: "empty", blocks: [{ type: "paragraph", text: "  " }] };
    const visible = sanitizeConversationEntries([blank, job]);
    assert.equal(visible.length, 1);
    assert.equal(visible[0].id, "job-row");
    assert.deepEqual(visible[0], job);
  }
});
