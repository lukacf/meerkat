import assert from "node:assert/strict";
import test from "node:test";
import { conversationEntryText } from "@console-core";
import { mapFramesToTimelineEntries as stock } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";
import type { ConsoleFrame } from "../types";
import { MEERKAT_1608_KICKOFF_NOTICES } from "./fixtures/meerkat-1608-kickoff-notices";

function frame(id: string, event: string, data: unknown, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data, timestampMs: 100, ...extra };
}

for (const [name, mapper] of [["stock", stock], ["shared", shared]] as const) {
  const options = { renderInteractionStartsAsUser: true, renderTextDeltas: true, textMode: "markdown" as const };

  test(`${name}: prepending owner history preserves every current row and document identity`, () => {
    const frames = [
      frame("input:12", "user_input", { content: "Current instruction" }, { timestampMs: 100, interactionId: "current" }),
      frame("delta:13", "text_delta", { delta: "Current response\n" }, { timestampMs: 101, interactionId: "current" }),
      frame("final:14", "interaction_complete", { result: "Current response\n" }, { timestampMs: 102, interactionId: "current" }),
    ];
    const before = mapper(null, frames, options);
    const after = mapper(null, [
      frame("older-input", "user_input", { content: "Older instruction" }, { timestampMs: 1, interactionId: "older" }),
      frame("older-final", "interaction_complete", { result: "Older response" }, { timestampMs: 2, interactionId: "older" }),
      ...frames,
    ], options);
    assert.equal(before.length, 2);
    assert.deepEqual(after.filter(entry => before.some(old => conversationEntryText(old) === conversationEntryText(entry))), before);
  });

  test(`${name}: numeric owner ID suffixes remain distinct Markdown document identities`, () => {
    const entries = mapper(null, [
      frame("owner:12", "user_input", { content: "One" }, { timestampMs: 1 }),
      frame("owner:13", "user_input", { content: "Two" }, { timestampMs: 2 }),
    ], options);
    const documentIds = entries.flatMap(entry => entry.kind === "message" ? (entry.blocks || []).flatMap(block => block.type === "markdown" ? [block.id] : []) : []);
    assert.equal(new Set(documentIds).size, 2);
    assert.deepEqual(documentIds, ["owner:12:text:0", "owner:13:text:0"]);
  });

  test(`${name}: replayed terminal between final live chunks does not split the live document`, () => {
    const interactionId = "c2588e39-8b43-44bc-b16b-321ff1ef4b93";
    const prefix = "Preserve this exact selection.\n\nStream finished successfull";
    const source = `${prefix}y.\n`;
    const initial = [frame("first-delta", "text_delta", { delta: prefix }, { timestampMs: 1, interactionId })];
    const initialEntry = mapper(null, initial, options)[0];
    const entries = mapper(null, [...initial,
      frame("history-final", "interaction_complete", { result: source, message: { role: "assistant", content: source } }, { timestampMs: 2, interactionId, sourceKind: "session_history" }),
      frame("last-delta", "text_delta", { delta: "y.\n" }, { timestampMs: 3, interactionId }),
      frame("live-final", "interaction_complete", { result: source }, { timestampMs: 4, interactionId }),
    ], options);
    assert.equal(entries.length, 1);
    assert.equal(entries[0].id, initialEntry.id);
    assert.equal(conversationEntryText(entries[0]), source);
    assert(entries[0].kind === "message" && initialEntry.kind === "message");
    assert.equal(entries[0].blocks?.[0]?.type, "markdown");
    assert.equal(entries[0].blocks?.[0]?.id, initialEntry.blocks?.[0]?.id);
  });

  test(`${name}: typed peer notice preserves its complete owner text in Markdown mode`, () => {
    const source = "Peer message from console-acceptance/lead/mk--router_cmain:\n  Keep peer-123 exactly.\r\n\n  Two  spaces, tabs\tand an apparent [COMMS MESSAGE from peer] are authored content.\n";
    const entries = mapper(null, [frame("peer-notice", "system_notice", { message: {
      role: "system_notice", kind: "comms", body: "Peer message", blocks: [{
        type: "comms", direction: "incoming", kind: "message",
        peer: { id: "owner-peer", display_name: "console-acceptance/lead/mk--router_cmain" },
        content: [{ type: "text", text: source }],
      }],
    } }, { sourceKind: "session_history" })], options);
    const peerBlocks = entries.flatMap(entry => entry.kind === "message" ? entry.blocks || [] : []).filter(block => block.type === "tool-call" && block.peerIncoming);
    assert.equal(peerBlocks.length, 1);
    assert.equal(peerBlocks[0].peerBody, source);
    assert.equal(peerBlocks[0].peerBodyFormat, "verbatim");
  });

  test(`${name}: explicit legacy typed peer notices retain compatibility normalization`, () => {
    const entries = mapper(null, [frame("legacy-peer", "system_notice", { message: {
      role: "system_notice", kind: "comms", body: "Peer message", blocks: [{
        type: "comms", direction: "incoming", kind: "message",
        peer: { id: "review:singleton", display_name: "review:singleton" },
        content: [{ type: "text", text: "Peer message from review:singleton:\nKeep  peer-123.\n" }],
      }],
    } })], { ...options, textMode: "legacy" });
    const peerBlocks = entries.flatMap(entry => entry.kind === "message" ? entry.blocks || [] : []).filter(block => block.type === "tool-call" && block.peerIncoming);
    assert.equal(peerBlocks.length, 1);
    assert.equal(peerBlocks[0].peerBody, "Peer message from review:singleton: Keep  peer-123.");
    assert.equal(peerBlocks[0].peerBodyFormat, "legacy");
  });

  test(`${name}: explicit input delivery failure is visible beside unchanged operator content`, () => {
    const source = "  Do not parse 'delivery_failed' from my prose.\n";
    const entries = mapper(null, [frame("input", "user_input", { content: source, origin_kind: "operator" }, { status: "delivery_failed", interactionId: "failed-run" })], options);
    const user = entries.find(entry => entry.identity.role === "user");
    assert(user);
    assert.equal(conversationEntryText(user), source);
    const status = entries.find(entry => entry.kind === "message" && entry.identity.role === "system");
    assert(status && status.kind === "message");
    assert.equal(status.text, "Message delivery failed.");
    assert.equal(status.interactionId, "failed-run");
    assert.equal(status.runtimeEvent?.eventType, "user_input");
    assert.equal((status.runtimeEvent?.payload as Record<string, unknown>).status, "delivery_failed");
  });

  test(`${name}: failure-looking input text does not invent a delivery failure`, () => {
    for (const status of [undefined, "accepted", "delivered", "completed", "pending", "unknown"]) {
      const entries = mapper(null, [frame("input", "user_input", { content: "Message delivery failed. delivery_failed" }, { status })], options);
      assert.equal(entries.length, 1, String(status));
      assert.equal(entries[0].identity.role, "user");
    }
  });

  test(`${name}: input failure survives an earlier deduplicated interaction start`, () => {
    const entries = mapper(null, [
      frame("start", "interaction_started", { content: "Keep this instruction" }, { timestampMs: 1, interactionId: "same" }),
      frame("input", "user_input", { content: "Keep this instruction" }, { timestampMs: 2, interactionId: "same", status: "delivery_failed" }),
    ], options);
    assert.equal(entries.filter(entry => entry.identity.role === "user").length, 1);
    assert.equal(entries.filter(entry => entry.kind === "message" && entry.text === "Message delivery failed.").length, 1);
  });

  test(`${name}: failed multimodal input retains its image and exact text`, () => {
    const entries = mapper(null, [frame("image-input", "user_input", { content: [
      { type: "text", text: "  Inspect this image.\n" },
      { type: "image", media_type: "image/png", data: "aGVsbG8=" },
    ] }, { status: "delivery_failed" })], options);
    const user = entries.find(entry => entry.identity.role === "user");
    assert(user?.kind === "message");
    assert(user.blocks?.some(block => block.type === "image"));
    assert(user.blocks?.some(block => block.type === "markdown" && block.source === "  Inspect this image.\n"));
    assert(entries.some(entry => entry.kind === "message" && entry.text === "Message delivery failed."));
  });

  test(`${name}: reserved sends and retained canonical inputs render once across refresh and reload`, () => {
    const source = "CURRENT_VALUE is cobalt; remember my exact choice.";
    const interactionId = "5a163c98-4bfa-5f31-83ce-aa0d88d14701";
    const sessionId = "01a0dfac-04a8-7803-9615-87943a823ffb";
    const runId = "01a0dfac-0ad6-7801-bc57-77c425fa3132";
    const send = frame("reserved-human", "user_input", {
      content: source, origin: "console", handling_mode: "queue", idempotency_key: "recall-projection",
    }, { sourceKind: "send", runtimeKey: "human", identity: "human", sessionId, interactionId, status: "delivered", timestampMs: 1, cursor: "console:1" });
    const history = frame("canonical-human", "user_input", {
      content: [{ type: "text", text: source }],
      message: { role: "user", content: source, identity: { run_id: runId, interaction_id: interactionId } },
    }, { sourceKind: "session_history", runtimeKey: "human", identity: "human", sessionId, interactionId, runId,
      sourceCursor: `${sessionId}:4`, status: "completed", timestampMs: 2, cursor: "console:5" });
    const inputs = (frames: ConsoleFrame[]) => mapper(null, frames, options)
      .filter(entry => entry.kind === "message" && entry.identity.role === "user");
    const baseline = inputs([send]);
    assert.equal(baseline.length, 1);
    for (const frames of [[send, history], [history, send]]) {
      const users = inputs(frames);
      assert.equal(users.length, 1, "two source records are one authored human input");
      assert.equal(users[0].id, baseline[0].id, "refresh preserves the admitted row key");
      assert.equal(conversationEntryText(users[0]), source);
    }
    const reloaded = inputs([history]);
    assert.equal(reloaded.length, 1, "canonical evidence renders without an admission record");
    assert.equal(conversationEntryText(reloaded[0]), source);
    const secondInteraction = "01900000-0000-7000-8000-000000000031";
    const repeated = frame("another-canonical-human", "user_input", {
      content: [{ type: "text", text: source }],
      message: { role: "user", content: source, identity: { run_id: runId, interaction_id: secondInteraction } },
    }, { sourceKind: "session_history", runtimeKey: "human", identity: "human", sessionId, runId, status: "completed", interactionId: secondInteraction,
      sourceCursor: `${sessionId}:6`, timestampMs: 3, cursor: "console:7" });
    const distinct = inputs([send, history, repeated]);
    assert.equal(distinct.length, 2, "identical text in a different interaction is a distinct input");
    assert.deepEqual(distinct.map(conversationEntryText), [source, source]);
  });
}

// Meerkat 0.8.44 TextComplete closes one provider message; RunCompleted
// repeats the last assistant message, not all of the run's text.
for (const [surface, mapper] of [["stock", stock], ["shared", shared]] as const) {
  const runId = "01900000-0000-7000-8000-000000000410";
  const interactionId = "01900000-0000-7000-8000-000000000411";
  const sessionId = "01900000-0000-7000-8000-000000000412";
  const owner = { runId, interactionId, sessionId, runtimeKey: "runtime", identity: "router:main" };
  const options = { renderInteractionStartsAsUser: true, renderTextDeltas: true, textMode: "markdown" as const };
  let sequence = 0;
  const live = (id: string, event: string, data: unknown) => frame(id, event, data, {
    ...owner, sourceKind: "console_event", cursor: `console:${++sequence}`, timestampMs: sequence,
  });
  const assistantText = (frames: ConsoleFrame[]) => mapper(null, frames, options)
    .filter(entry => entry.kind === "message" && entry.identity.role === "assistant")
    .flatMap(entry => entry.kind === "message" ? entry.blocks?.flatMap(block => block.type === "markdown" ? [block.source] : []) || [] : []);
  for (const textComplete of [true, false]) {
    for (const finalText of ["The answer is 42.", "Let me check."]) {
      test(`${surface}: multi-turn terminal joins the last occurrence with text_complete=${textComplete} and final=${finalText}`, () => {
        const frames = [
          live("preamble", "text_delta", { delta: "Let me check." }),
          ...(textComplete ? [live("preamble-complete", "text_complete", { content: "Let me check." })] : []),
          live("lookup", "tool_call_requested", { id: "lookup-1", name: "lookup", args: {} }),
          live("lookup-result", "tool_execution_completed", { id: "lookup-1", name: "lookup", content: [{ type: "text", text: "42" }], is_error: false, duration_ms: 10 }),
          live("answer", "text_delta", { delta: finalText }),
          ...(textComplete ? [live("answer-complete", "text_complete", { content: finalText })] : []),
          live("terminal", "interaction_complete", { result: finalText }),
        ];
        assert.deepEqual(assistantText(frames), ["Let me check.", finalText]);
        const saved = frame("saved-final", "text_complete", { text: finalText, result: finalText, message: {
          role: "block_assistant", identity: { run_id: runId, interaction_id: interactionId },
          stop_reason: "end_turn", blocks: [{ block_type: "text", data: { text: finalText } }],
        } }, { ...owner, sourceKind: "session_history", sourceCursor: `${sessionId}:4`, cursor: `console:${++sequence}`, timestampMs: sequence });
        assert.deepEqual(assistantText([...frames, saved]), ["Let me check.", finalText]);
      });
    }
  }
  test(`${surface}: two equal message completions without deltas remain separate occurrences`, () => {
    const frames = [
      live("first-complete", "text_complete", { content: "Ready." }),
      live("read", "tool_call_requested", { id: "read-1", name: "read_file", args: {} }),
      live("read-result", "tool_execution_completed", { id: "read-1", name: "read_file", content: [], is_error: false }),
      live("second-complete", "text_complete", { content: "Ready." }),
      live("last-terminal", "interaction_complete", { result: "Ready." }),
    ];
    assert.deepEqual(assistantText(frames), ["Ready.", "Ready."]);
  });
  for (const notices of [undefined, []]) {
    test(`${surface}: durable user append with notices=${JSON.stringify(notices)} has no transport system row`, () => {
      const entries = mapper(null, [
        live("steer", "user_input", { content: "Check the second source too." }),
        live("applied", "boundary_append_applied", { run_id: runId,
          input_id: "01900000-0000-7000-8000-000000000413", content: "Check the second source too.",
          append_count: 1, transcript_start: 3, ...(notices ? { notices } : {}),
        }),
      ], options);
      assert.deepEqual(entries.map(entry => [entry.identity.role, conversationEntryText(entry)]), [
        ["user", "Check the second source too."],
      ]);
    });
  }
}

for (const [surface, mapper] of [["stock", stock], ["shared", shared]] as const) {
  const owner = {
    runId: "01900000-0000-7000-8000-000000000610",
    interactionId: "01900000-0000-7000-8000-000000000611",
    sessionId: "01900000-0000-7000-8000-000000000612",
    runtimeKey: "runtime", identity: "router:main", sourceKind: "console_event",
  };
  const options = { renderInteractionStartsAsUser: true, renderTextDeltas: true, textMode: "markdown" as const };
  const sources = (frames: ConsoleFrame[]) => mapper(null, frames, options)
    .flatMap(entry => entry.kind === "message" && entry.identity.role === "assistant"
      ? entry.blocks?.flatMap(block => block.type === "markdown" ? [block.source] : []) || [] : []);

  for (const order of ["source-sequence", "console-cursor"] as const) {
    for (const lastDeltaTies of [false, true]) {
      test(`${surface}: equal timestamp completion precedes the tool boundary by ${order}, last delta tied=${lastDeltaTies}`, () => {
        const events = [
          ["u", "user_input", 1, { content: "What is it?" }],
          ["d0", "text_delta", 10, { delta: lastDeltaTies ? "Let me " : "Let me check." }],
          ...(lastDeltaTies ? [["d1", "text_delta", 20, { delta: "check." }] as const] : []),
          ["c1", "text_complete", 20, { content: "Let me check." }],
          ["t1", "tool_call_requested", 20, { id: "lookup", name: "lookup", args: {} }],
          ["t2", "tool_execution_completed", 30, { id: "lookup", name: "lookup", content: "42", is_error: false }],
          ["d2", "text_delta", 40, { delta: "The answer is 42." }],
          ["c2", "text_complete", 50, { content: "The answer is 42." }],
          ["done", "interaction_complete", 50, { result: "The answer is 42." }],
        ] as const;
        const frames = events.map(([id, event, timestampMs, data], index) => frame(id, event, {
          ...data, ...(order === "source-sequence" ? { source_sequence: index + 1 } : {}),
        }, { ...owner, timestampMs, cursor: `console:${order === "source-sequence" ? 20 - index : index + 1}` }));
        for (const input of [frames, [...frames].reverse()]) {
          assert.deepEqual(sources(input), ["Let me check.", "The answer is 42."]);
        }
      });
    }
  }

  test(`${surface}: source sequence never orders unrelated runtime or session streams`, () => {
    for (const conflict of [{ runtimeKey: "another-runtime" }, { sessionId: "another-session" }]) {
      const first = frame("first", "text_complete", { content: "First.", source_sequence: 50 }, { ...owner });
      const second = frame("second", "text_complete", { content: "Second.", source_sequence: 1 }, {
        ...owner, ...conflict, runId: "another-run", interactionId: "another-interaction",
      });
      assert.deepEqual(sources([first, second]), ["First.", "Second."]);
    }
  });

  for (const variant of ["split-text", "whitespace-block"] as const) {
    test(`${surface}: canonical ${variant} history reconciles and copies exact assistant source`, () => {
      const parts = variant === "split-text" ? ["Paris is the capital", " of France.\n"] : ["Alpha", "  \n", "Beta\n"];
      const source = parts.join("");
      const history = frame("saved-blocks", "text_complete", { text: source, result: source, message: {
        role: "block_assistant", identity: { run_id: owner.runId, interaction_id: owner.interactionId },
        blocks: parts.map(text => ({ block_type: "text", data: { text } })), stop_reason: "end_turn",
      } }, { ...owner, sourceKind: "session_history", timestampMs: 50, sourceCursor: `${owner.sessionId}:4` });
      const live = [
        frame("delta", "text_delta", { delta: source }, { ...owner, timestampMs: 10 }),
        frame("complete", "text_complete", { content: source }, { ...owner, timestampMs: 20 }),
      ];
      for (const input of [live.concat(history), [history, ...live], [history]]) {
        const entries = mapper(null, input, options).filter(entry => entry.kind === "message" && entry.identity.role === "assistant");
        assert.equal(entries.length, 1, "live/history handoff and cold reload each contain one assistant message");
        assert.equal(conversationEntryText(entries[0]), source, "copy preserves every authored whitespace byte");
        assert.deepEqual(sources(input), [source], "adjacent source blocks remain one Markdown document");
      }
    });
  }

  test(`${surface}: canonical server-tool split history does not repeat the live answer`, () => {
    const source = "Paris is the capital of France.";
    const events = [
      ["u", "user_input", { content: "Capital?" }],
      ["d1", "text_delta", { delta: "Paris is the capital" }],
      ["st", "server_tool_content", { id: "srv", name: "web_search" }],
      ["d2", "text_delta", { delta: " of France." }],
      ["c", "text_complete", { content: source }],
      ["done", "interaction_complete", { result: source }],
    ] as const;
    const live = events.map(([id, event, data], index) => frame(id, event, data, {
      ...owner, cursor: `console:${index + 1}`, timestampMs: index + 1,
    }));
    const history = frame("h", "text_complete", { text: source, result: source, message: {
      role: "block_assistant", identity: { run_id: owner.runId, interaction_id: owner.interactionId },
      stop_reason: "end_turn", blocks: [
        { block_type: "text", data: { text: "Paris is the capital" } },
        { block_type: "server_tool_use", data: { id: "srv", name: "web_search", input: {} } },
        { block_type: "text", data: { text: " of France." } },
      ],
    } }, { ...owner, sourceKind: "session_history", sourceCursor: `${owner.sessionId}:2`, cursor: "console:7", timestampMs: 7 });
    for (const input of [[...live, history], [history, ...live]]) {
      assert.equal(sources(input).join(""), source);
    }
    const reloaded = mapper(null, [history], options);
    assert.equal(reloaded.length, 1);
    assert.equal(conversationEntryText(reloaded[0]), source);
  });

  test(`${surface}: canonical assistant source preserves rich tool siblings and legacy formatting`, () => {
    const parts = ["Before ", "\n\nAfter.\n"];
    const source = parts.join("");
    const history = frame("saved-rich", "text_complete", { text: source, result: source, message: {
      role: "block_assistant", identity: { run_id: owner.runId, interaction_id: owner.interactionId },
      blocks: [
        { block_type: "text", data: { text: parts[0] } },
        { block_type: "tool_use", data: { id: "read-1", name: "read_file", args: { path: "a" } } },
        { block_type: "text", data: { text: parts[1] } },
      ], stop_reason: "tool_use",
    } }, { ...owner, sourceKind: "session_history", sourceCursor: `${owner.sessionId}:4` });
    const entry = mapper(null, [history], options)[0];
    assert(entry.kind === "message");
    assert.deepEqual(entry.blocks?.map(block => block.type), ["markdown", "tool-call", "markdown"]);
    assert.equal(conversationEntryText(entry), source, "message copy uses the owner's authored source, not a tool label or invented separator");
    const legacy = mapper(null, [history], { ...options, textMode: "legacy" })[0];
    assert(legacy.kind === "message");
    assert.equal(legacy.copyText, undefined, "legacy formatting stays on its existing rich block copy path");
    assert(legacy.blocks?.some(block => block.type === "tool-call"));
  });
}

for (const [surface, mapper] of [["stock", stock], ["shared", shared]] as const) {
  test(`${surface}: historical snapshot notices keep a new live document streaming and before its real tool boundary`, () => {
    const sessionId = "01900000-0000-7000-8000-000000000701";
    const oldRun = "01900000-0000-7000-8000-000000000702";
    const runId = "01900000-0000-7000-8000-000000000703";
    const interactionId = "01900000-0000-7000-8000-000000000704";
    const owner = { runtimeKey: "runtime", identity: "router:main", sessionId };
    const options = { renderInteractionStartsAsUser: true, renderTextDeltas: true, textMode: "markdown" as const };
    const history = (id: string, text: string, offset: number, timestampMs: number) => frame(id, "text_complete", {
      text, result: text, message: { role: "block_assistant", blocks: [{ block_type: "text", data: { text } }] },
    }, { ...owner, runId: oldRun, timestampMs, cursor: `console:${offset + 1}`,
      sourceKind: "session_history", sourceCursor: `${sessionId}:${offset}` });
    const snapshot = frame("old-snapshot", "runtime_notice_snapshot", {
      session_id: sessionId, complete: true, observed_through: "console:9", settled_attempts: [], notices: [{
        offset: 1, message: { role: "system_notice", kind: "generic", body: "Earlier peer review.", blocks: [],
          created_at: new Date(20).toISOString(), runtime_origin: { session_id: sessionId, run_id: oldRun,
            input_id: "01900000-0000-7000-8000-000000000705", append_ordinal: 0 } },
      }],
    }, { ...owner, sourceKind: "session_history", timestampMs: 40, cursor: "console:10" });
    const earlier = [history("old-before", "Earlier before.", 0, 10), history("old-after", "Earlier after.", 2, 30), snapshot];
    const live = (id: string, event: string, data: object, sequence: number, timestampMs: number) => frame(id, event,
      { ...data, source_sequence: sequence }, { ...owner, runId, interactionId, sourceKind: "console_event",
        timestampMs, cursor: `console:${sequence + 10}` });
    const start = live("new-start", "run_started", {}, 1, 100);
    const first = live("new-delta", "text_delta", { delta: "Preserve this exact selection: A\u030A, \u00e5 and \u{1f680}.\n\n" }, 2, 110);
    const next = live("new-delta-2", "text_delta", { delta: "More live source." }, 3, 120);
    for (const input of [[...earlier, start, first, next], [...earlier, start, first, next].reverse()]) {
      const entries = mapper(null, input, options);
      assert.deepEqual(entries.slice(0, 3).map(conversationEntryText), ["Earlier before.", "Earlier peer review.", "Earlier after."]);
      const current = entries.find(entry => entry.id === first.id);
      assert(current?.kind === "message");
      assert.deepEqual(current.blocks, [{ type: "markdown", id: `${first.id}:text:0`,
        source: "Preserve this exact selection: A\u030A, \u00e5 and \u{1f680}.\n\nMore live source.", streaming: true }]);
      assert.equal(entries.at(-1)?.id, first.id, "historical rows cannot migrate after a current stream");
    }
    const boundary = live("new-tool", "tool_call_requested", { id: "call-new", name: "lookup", args: {} }, 4, 130);
    const result = live("new-result", "tool_execution_completed", { id: "call-new", name: "lookup", content: "ok", is_error: false }, 5, 140);
    const answer = live("after-tool", "text_delta", { delta: "After the current tool." }, 6, 150);
    const entries = mapper(null, [...earlier, start, first, next, boundary, result, answer], options);
    const blocks = entries.flatMap(entry => entry.kind === "message" ? entry.blocks || [] : []);
    const beforeTool = blocks.find(block => block.type === "markdown" && block.id === `${first.id}:text:0`);
    const afterTool = blocks.find(block => block.type === "markdown" && block.id === `${answer.id}:text:0`);
    assert(beforeTool?.type === "markdown" && !beforeTool.streaming, "the current owner's actual tool boundary closes its preceding document");
    assert(afterTool?.type === "markdown" && afterTool.streaming);
    assert(blocks.findIndex(block => block.type === "tool-call") > blocks.indexOf(beforeTool));
    assert(blocks.indexOf(afterTool) > blocks.findIndex(block => block.type === "tool-call"));

    const lateSnapshot = { ...snapshot, id: "late-snapshot", timestampMs: 160, cursor: "console:30" };
    const lateHistory = mapper(null, [...earlier.slice(0, 2), start, first, next, lateSnapshot], options);
    const lateCurrent = lateHistory.at(-1);
    assert.equal(lateCurrent?.id, first.id, "snapshot publication after live deltas does not move its old authored notice");
    assert(lateCurrent?.kind === "message" && lateCurrent.blocks?.[0].type === "markdown"
      && lateCurrent.blocks[0].streaming);

    const currentNotice = { role: "system_notice", kind: "generic", body: "Current durable notice.", blocks: [],
      created_at: new Date(135).toISOString(), runtime_origin: { session_id: sessionId, run_id: runId,
        input_id: "01900000-0000-7000-8000-000000000706", append_ordinal: 0 } };
    const applied = live("current-append", "boundary_append_applied", { session_id: sessionId, run_id: runId,
      input_id: currentNotice.runtime_origin.input_id, append_count: 1, transcript_start: 3, notices: [currentNotice] }, 5, 135);
    const during = { ...lateSnapshot, data: { ...lateSnapshot.data as object,
      notices: [{ offset: 3, message: currentNotice }], observed_through: "console:25" } };
    const currentEntries = mapper(null, [boundary, applied, result, answer, during], options);
    const noticeIndex = currentEntries.findIndex(entry => conversationEntryText(entry) === currentNotice.body);
    const toolIndex = currentEntries.findIndex(entry => entry.kind === "message" && entry.blocks?.some(block => block.type === "tool-call"));
    assert(noticeIndex > toolIndex, "a mid-run durable notice stays after its actual live tool boundary");
    assert.equal(currentEntries.at(-1)?.id, answer.id, "later live text remains after the mid-run notice");
  });
}

// Meerkat 0.8.50 sends member-kickoff status as peer requests whose content
// is the full peer transport projection (peer_spec, pubkey, send_response
// coaching), and #1608 moves it to one-way lifecycle notices. That content is
// model-facing; no surface or text mode may show it. An ordinary peer
// request's content is authored text and still shows.
const KICKOFF_PROJECTION = "Peer request from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c"
  + " (display_name: mob/commander/incident-commander) (id: 964020b4-c9b6-4c31-ba6c-30598279b388)\n"
  + "Intent: INTENT\nParams: {\"peer\":\"incident-commander\",\"peer_spec\":{\"pubkey\":[20,129]}}\n"
  + "Request ID: 964020b4-c9b6-4c31-ba6c-30598279b388\n\n"
  + "This is a correlated peer request. Reply with send_response with arguments"
  + " {\"in_reply_to\":\"964020b4-c9b6-4c31-ba6c-30598279b388\",\"status\":\"completed\"}."
  + " Do not answer this request with send_message.";

function typedCommsNotice(kind: string, intent: string, content: string): ConsoleFrame {
  const summary = `${kind === "lifecycle" ? "Peer lifecycle" : "Peer request"}: ${intent}`;
  return frame("notice", "system_notice", { message: { role: "system_notice", kind: "comms", body: summary, blocks: [{
    type: "comms", kind, direction: "incoming",
    peer: { id: "6f6114cd-2cf7-590f-a172-0e36feacd12c", display_name: "mob/commander/incident-commander" },
    ...(kind === "request" ? { request_id: "964020b4-c9b6-4c31-ba6c-30598279b388" } : {}),
    intent, summary, payload: { peer: "incident-commander" },
    content: [{ type: "text", text: content }],
  }] } }, { sourceKind: "session_history" });
}

for (const [surface, mapper] of [["stock", stock], ["shared", shared]] as const) {
  for (const textMode of ["legacy", "markdown"] as const) {
    test(`${surface} ${textMode}: kickoff notices never show the peer transport projection`, () => {
      for (const [kind, intent] of [
        ["request", "mob.kickoff_started"], ["request", "mob.kickoff_failed"], ["request", "mob.kickoff_cancelled"],
        ["lifecycle", "mob.kickoff_started"],
      ] as const) {
        const rendered = JSON.stringify(mapper(null, [typedCommsNotice(kind, intent, KICKOFF_PROJECTION.replace("INTENT", intent))], { textMode }));
        assert.ok(!/send_response|pubkey|Do not answer this request|Peer request from peer_id/.test(rendered),
          `${surface} ${textMode} ${kind} ${intent} shows model-facing transport text`);
        assert.ok(rendered.includes(`"type":"member-kickoff","phase":"${intent.slice("mob.kickoff_".length)}"`),
          `${surface} ${textMode} ${kind} ${intent} renders as its typed kickoff status`);
      }
      const authored = JSON.stringify(mapper(null, [typedCommsNotice("request", "review.document", "Please review the release notes.")], { textMode }));
      assert.ok(authored.includes("Please review the release notes."), `${surface} ${textMode}: an ordinary request's authored content still shows`);
    });
  }
}

// Meerkat #1608: member-kickoff status arrives as a one-way typed lifecycle
// notice (older sessions keep the request form). Its content is model-facing
// notice text; neither surface may show it, and both render the typed status.
const LIFECYCLE_NOTICE_TEXT = "Peer lifecycle notice from peer_id 6f6114cd-2cf7-590f-a172-0e36feacd12c\n"
  + "Kind: KIND\nParams: {\"peer\":\"incident-commander\",\"role\":\"commander\"}\n\n"
  + "This is a one-way status notice, not a request. There is nothing to answer: do not call send_response or send_message for it.";

function commsNotice(kind: "lifecycle" | "request", intent: string): ConsoleFrame {
  const summary = `${kind === "lifecycle" ? "Peer lifecycle" : "Peer request"}: ${intent}`;
  return {
    id: `notice-${kind}-${intent}`,
    event: "system_notice",
    timestampMs: 1_000,
    sourceKind: "session_history",
    data: {
      message: {
        role: "system_notice",
        kind: "comms",
        body: summary,
        blocks: [{
          type: "comms",
          kind,
          direction: "incoming",
          peer: { id: "6f6114cd-2cf7-590f-a172-0e36feacd12c", display_name: "incident-command-center/commander/incident-commander" },
          ...(kind === "request" ? { request_id: "964020b4-c9b6-4c31-ba6c-30598279b388" } : {}),
          intent,
          summary,
          payload: { peer: "incident-commander", role: "commander" },
          content: [{ type: "text", text: LIFECYCLE_NOTICE_TEXT.replace("KIND", intent) }],
        }],
      },
    },
  };
}

for (const [surface, mapper] of [["stock", stock], ["shared", shared]] as const) {
  test(`${surface}: kickoff notices render as typed status and never show lifecycle notice text`, () => {
    // Meerkat #1608's serialized lifecycle notices, every phase.
    for (const message of MEERKAT_1608_KICKOFF_NOTICES) {
      const phase = message.blocks[0].intent.slice("mob.kickoff_".length);
      const entries = mapper(null, [frame(`real-${phase}`, "system_notice", { message: structuredClone(message) }, { sourceKind: "session_history" })]);
      const blocks = entries.flatMap((entry) => "blocks" in entry && Array.isArray(entry.blocks) ? entry.blocks : []);
      const kickoff = blocks.find((block) => block.type === "member-kickoff");
      assert.equal(kickoff?.type === "member-kickoff" ? kickoff.phase : undefined, phase, `${surface} lifecycle ${phase}`);
      assert.equal(kickoff?.type === "member-kickoff" ? kickoff.member : undefined, "delivery-lead", `${surface} lifecycle ${phase}`);
      assert.ok(!JSON.stringify(entries).includes("Peer lifecycle notice from"), `${surface} lifecycle ${phase}`);
    }
    {
      const entries = mapper(null, [commsNotice("request", "mob.kickoff_started")]);
      const blocks = entries.flatMap((entry) => "blocks" in entry && Array.isArray(entry.blocks) ? entry.blocks : []);
      const kickoff = blocks.find((block) => block.type === "member-kickoff");
      assert.equal(kickoff?.type === "member-kickoff" ? kickoff.phase : undefined, "started", `${surface} request started`);
    }
    const other = mapper(null, [commsNotice("lifecycle", "mob.member_paused")]);
    assert.ok(!JSON.stringify(other).includes("Peer lifecycle notice from"), `${surface}: other lifecycle notices show their summary`);
    assert.ok(JSON.stringify(other).includes("Peer lifecycle: mob.member_paused"), `${surface}: by its summary`);
  });
}
