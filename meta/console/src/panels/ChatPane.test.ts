import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import type { ConversationTimelineEntry } from "@console-core";
import {
  ChatPane,
  TURN_RAIL_MAX_TICKS,
  TURN_RAIL_TICK_PX,
  __chatPaneTest,
  transcriptFindMatches,
  windowTurnRail,
} from "./ChatPane";
import { normalizePendingApproval, type PendingApprovalSnapshot } from "../../../packages/console-core/src/pending-approvals";
import type { LiveSpeechItem } from "../lib/voice-session";
import { mapFramesToTimelineEntries } from "../lib/adapters";
import type { ConsoleFrame } from "../../../packages/console-core/src/runtime-types";

const USER = { id: "user", label: "You", role: "user" as const };
const AGENT = { id: "agent", label: "Agent", role: "assistant" as const };

function message(args: {
  id: string;
  role: "user" | "assistant";
  createdAt: string;
  text: string;
  runId?: string;
  runDurationMs?: number;
}): ConversationTimelineEntry {
  return {
    id: args.id,
    kind: "message",
    variant: "plain",
    identity: args.role === "user" ? USER : AGENT,
    createdAt: args.createdAt,
    text: args.text,
    ...(args.runId ? { runId: args.runId } : {}),
    ...(args.runDurationMs !== undefined ? { runDurationMs: args.runDurationMs } : {}),
  };
}

test("find in transcript matches every loaded message's text, case-insensitively, in order", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({ id: "q1", role: "user", createdAt: "2026-05-20T04:58:01.000Z", text: "Where is the Retry policy?" }),
    message({ id: "a1", role: "assistant", createdAt: "2026-05-20T04:58:05.000Z", text: "The retry policy lives in config." }),
    message({ id: "q2", role: "user", createdAt: "2026-05-20T04:59:01.000Z", text: "Thanks." }),
  ]);
  const rows = (query: string) => transcriptFindMatches(messages, query);
  const rowOf = (id: string) => messages.find((entry) => entry.id === id)!.scrollRowId ?? id;
  assert.deepEqual(rows("retry policy"), [rowOf("q1"), rowOf("a1")]);
  assert.deepEqual(rows("  THANKS "), [rowOf("q2")]);
  assert.deepEqual(rows(""), []);
  assert.deepEqual(rows("absent"), []);
});

test("chat pane does not count spawn scaffolding as user work", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({
      id: "spawn",
      role: "user",
      createdAt: "2026-05-20T04:58:01.000Z",
      text: "You have been spawned as 'review:singleton' (role: review) in mob 'ops'.",
    }),
    message({
      id: "ready",
      role: "assistant",
      createdAt: "2026-05-20T06:43:02.000Z",
      text: "Ready.",
    }),
  ]);

  assert.equal(messages.find((entry) => entry.id === "ready")?.workedFor, undefined);
});

test("chat pane displays the supplied completed run duration", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({
      id: "operator",
      role: "user",
      createdAt: "2026-05-20T06:43:02.000Z",
      text: "Please review the PR.",
    }),
    message({
      id: "done",
      role: "assistant",
      createdAt: "2026-05-20T06:45:07.000Z",
      text: "Review complete.",
      runId: "review-run",
      runDurationMs: 125000,
    }),
  ]);

  assert.equal(messages.find((entry) => entry.id === "done")?.workedFor, "2m 5s");
});

test("chat pane omits work duration when only message timestamps are known", () => {
  const entries = [
    message({ id: "unknown-ask", role: "user", createdAt: "2026-09-26T05:19:44.485Z", text: "Review the release." }),
    message({ id: "unknown-answer", role: "assistant", createdAt: "2026-09-26T05:21:49.485Z", text: "Review complete." }),
  ];
  const messages = __chatPaneTest.buildChatMessages(entries);
  assert.equal(messages.find((entry) => entry.sourceEntryId === "unknown-answer")?.workedFor, undefined);
  assert.doesNotMatch(renderChat({ entries, phase: null }), /Worked for/);
  assert.doesNotMatch(__chatPaneTest.transcriptCopyText(messages), /Worked for/);
});

test("a delayed streamed answer cannot use its first text timestamp as run completion", () => {
  // The real acceptance run began at 44.485 and completed at 50.171. Its
  // retained streaming row is stamped at the first text delta, 44.838.
  const entries: ConversationTimelineEntry[] = [
    { ...message({ id: "delayed-ask", role: "user", createdAt: "2026-09-26T05:19:44.485Z", text: "Review the release." }), interactionId: "release-interaction" },
    { ...message({ id: "delayed-answer", role: "assistant", createdAt: "2026-09-26T05:19:44.838Z", text: "The full review is now complete." }), interactionId: "release-interaction", runId: "release-run" },
  ];
  const messages = __chatPaneTest.buildChatMessages(entries);
  assert.equal(messages.find((entry) => entry.sourceEntryId === "delayed-answer")?.workedFor, undefined);
  assert.doesNotMatch(renderChat({ entries, phase: null }), /Worked for/);
});

test("completed stream duration includes the time after its first text delta", () => {
  const entries = [
    message({ id: "timed-ask", role: "user", createdAt: "2026-09-26T05:19:44.485Z", text: "Review the release." }),
    message({ id: "timed-answer", role: "assistant", createdAt: "2026-09-26T05:19:44.838Z", text: "The full review is now complete.", runId: "release-run", runDurationMs: 5686 }),
  ];
  const messages = __chatPaneTest.buildChatMessages(entries);
  assert.equal(messages.find((entry) => entry.sourceEntryId === "timed-answer")?.workedFor, "6s");
  assert.match(renderChat({ entries, phase: null }), /Worked for 6s/);
  assert.doesNotMatch(renderChat({ entries, phase: null }), /Worked for under 1s/);
  assert.doesNotMatch(renderChat({ entries, phase: null }), /aria-label="Copy work time"/);
});

for (const duration of [Number.NaN, Number.POSITIVE_INFINITY, -1]) {
  test(`chat pane rejects invalid completed run duration ${duration}`, () => {
    const entries = [message({ id: "invalid-duration", role: "assistant", createdAt: "2026-09-26T05:19:44.838Z", text: "Done.", runId: "release-run", runDurationMs: duration })];
    assert.doesNotMatch(renderChat({ entries, phase: null }), /Worked for/);
  });
}

function durationNotice(id: string, createdAt: string): ConversationTimelineEntry {
  return {
    id, kind: "message", variant: "rich",
    identity: { id: "system", label: "Agent", role: "system" },
    interactionId: "review-interaction", runId: "review-run", runDurationMs: 500, createdAt,
    blocks: [{ type: "paragraph", text: "Background review has new evidence." }],
  };
}

const DURATION_REVIEW_ENTRIES: ConversationTimelineEntry[] = [
  { ...message({ id: "review-ask", role: "user", createdAt: "2026-05-20T06:43:02.000Z", text: "Please review the PR." }), interactionId: "review-interaction" },
  durationNotice("review-notice", "2026-05-20T06:43:02.500Z"),
  { ...message({ id: "review-answer", role: "assistant", createdAt: "2026-05-20T06:45:07.000Z", text: "Review complete." }), identity: { ...AGENT, label: "System" }, interactionId: "review-interaction", runId: "review-run", runDurationMs: 125000 },
];

test("system notice never receives or consumes the assistant work duration", () => {
  const messages = __chatPaneTest.buildChatMessages(DURATION_REVIEW_ENTRIES);
  const notice = messages.find((entry) => entry.sourceEntryId === "review-notice");
  const answer = messages.find((entry) => entry.sourceEntryId === "review-answer");
  assert.equal(notice?.source?.kind, "system");
  assert.equal(notice?.workedFor, undefined);
  assert.equal(answer?.source?.kind, "assistant");
  assert.equal(answer?.workedFor, "2m 5s");
  assert.equal(answer?.interactionId, "review-interaction");
  assert.equal(answer?.runId, "review-run");
  const working = renderChat({ entries: DURATION_REVIEW_ENTRIES, phase: "generating" });
  assert.doesNotMatch(working, /Worked for/);
  assert.match(working, /chat-typing:agent/);
  const done = renderChat({ entries: DURATION_REVIEW_ENTRIES, phase: null });
  assert.equal((done.match(/class="msg__worked"/g) || []).length, 1);
  assert.match(done, /Worked for 2m 5s/);
});

test("system task, tool, peer and reasoning rows preserve assistant timing", () => {
  const base: ConversationTimelineEntry = {
    id: "intervening", kind: "message", variant: "rich", identity: AGENT,
    createdAt: "2026-05-20T06:43:03.000Z",
    interactionId: "review-interaction", runId: "review-run",
  };
  const intervening: ConversationTimelineEntry[] = [
    { ...base, id: "task", taskKind: "progress", taskLabel: "Assistant", blocks: [{ type: "paragraph", text: "Checking evidence." }] },
    { ...base, id: "tool", blocks: [{ type: "tool-call", toolCallId: "check", name: "workgraph_ready", arguments: "{}", status: "success", result: '{"items":[]}' }] },
    { ...base, id: "peer", blocks: [{ type: "tool-call", toolCallId: "peer-check", name: "peer_message", arguments: "{}", status: "success", peerIncoming: true, peerIdentity: "reviewer", peerTarget: "Reviewer", peerBody: "Evidence received." }] },
    { ...base, id: "reasoning", blocks: [{ type: "thinking", text: "Reviewing the evidence." }] },
  ];
  const messages = __chatPaneTest.buildChatMessages([
    DURATION_REVIEW_ENTRIES[0], ...intervening, DURATION_REVIEW_ENTRIES[2],
  ]);
  assert.ok(messages.filter((entry) => intervening.some((source) => source.id === entry.sourceEntryId)).every((entry) => entry.workedFor === undefined));
  assert.equal(messages.find((entry) => entry.sourceEntryId === "review-answer")?.workedFor, "2m 5s");
});

test("a trailing system notice cannot expose a still-working assistant duration", () => {
  const entries = [...DURATION_REVIEW_ENTRIES, durationNotice("later-notice", "2026-05-20T06:45:08.000Z")];
  const working = renderChat({ entries, phase: "generating" });
  assert.match(working, /chat-typing:agent/);
  assert.doesNotMatch(working, /Worked for/);
  const done = renderChat({ entries, phase: null });
  assert.equal((done.match(/class="msg__worked"/g) || []).length, 1);
  assert.match(done, /Worked for 2m 5s/);
});

test("chat pane groups messages into user-addressable scroll turns", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({
      id: "ask-1",
      role: "user",
      createdAt: "2026-05-20T06:43:02.000Z",
      text: "First request.",
    }),
    message({
      id: "answer-1",
      role: "assistant",
      createdAt: "2026-05-20T06:43:07.000Z",
      text: "First response.",
    }),
    message({
      id: "ask-2",
      role: "user",
      createdAt: "2026-05-20T06:44:02.000Z",
      text: "Second request.",
    }),
    message({
      id: "answer-2",
      role: "assistant",
      createdAt: "2026-05-20T06:44:07.000Z",
      text: "Second response.",
    }),
  ]);

  const turns = __chatPaneTest.buildChatTurns(messages);

  assert.equal(turns.length, 2);
  assert.deepEqual(turns[0].messages.map((entry) => entry.id), ["ask-1", "answer-1"]);
  assert.deepEqual(turns[1].messages.map((entry) => entry.id), ["ask-2", "answer-2"]);
  assert.deepEqual(__chatPaneTest.chatTurnPreview(turns[0]), {
    title: "First request.",
    body: "First response.",
  });
});

test("chat pane renders turn rail markers when multiple turns are present", () => {
  const html = renderChat({
    entries: [
      message({ id: "ask-1", role: "user", createdAt: "2026-05-20T06:43:02.000Z", text: "First request." }),
      message({ id: "answer-1", role: "assistant", createdAt: "2026-05-20T06:43:07.000Z", text: "First response." }),
      message({ id: "ask-2", role: "user", createdAt: "2026-05-20T06:44:02.000Z", text: "Second request." }),
      message({ id: "answer-2", role: "assistant", createdAt: "2026-05-20T06:44:07.000Z", text: "Second response." }),
    ],
    phase: null,
  });

  assert.match(html, /aria-label="Conversation turns"/);
  assert.match(html, /data-testid="chat-turn:agent:0"/);
  assert.match(html, /data-testid="chat-turn:agent:1"/);
  assert.match(html, /data-testid="chat-turn-rail:agent:0"/);
  assert.match(html, /data-testid="chat-turn-rail:agent:1"/);
  assert.match(html, /First request/);
  assert.match(html, /First response/);
});

test("chat pane does not count peer update scaffolding as user work", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({
      id: "peer-update",
      role: "user",
      createdAt: "2026-05-20T06:43:02.000Z",
      text: "[PEER UPDATE] review:singleton is now idle.",
    }),
    message({
      id: "reply",
      role: "assistant",
      createdAt: "2026-05-20T06:45:07.000Z",
      text: "Ready.",
    }),
  ]);

  assert.equal(messages.find((entry) => entry.id === "reply")?.workedFor, undefined);
});

test("chat pane disables composer in read-only mode", () => {
  const html = renderToStaticMarkup(
    React.createElement(ChatPane, {
      agent: {
        agent_id: "agent",
        member_id: "agent",
        identity: "agent",
        label: "Agent",
        kind: "mob_agent",
        role: "worker",
        state: "active",
        model_capabilities: { image_input: true },
      },
      agentLabel: "Agent",
      identity: "agent",
      entries: [],
      phase: null,
      draft: "hello",
      sending: false,
      readOnly: true,
      staged: [],
      onDraftChange: () => undefined,
      onStagedChange: () => undefined,
      onSend: () => true,
    }),
  );

  assert.match(html, /disabled=""/);
  assert.match(html, /View-only console/);
  assert.match(html, /view only/);
});

test("chat pane offers Stop run only when handed an active run, and shows the receipt", () => {
  const props = {
    agent: {
      agent_id: "agent",
      member_id: "agent",
      identity: "agent",
      label: "Agent",
      kind: "mob_agent",
      role: "worker",
      state: "active",
      model_capabilities: { image_input: true },
    },
    agentLabel: "Agent",
    identity: "agent",
    entries: [],
    phase: "generating" as const,
    draft: "",
    sending: false,
    readOnly: false,
    staged: [],
    onDraftChange: () => undefined,
    onStagedChange: () => undefined,
    onSend: () => true,
  };
  const idle = renderToStaticMarkup(React.createElement(ChatPane, props));
  assert.doesNotMatch(idle, /conv-action:stop-run/);
  assert.doesNotMatch(idle, /run-stop-notice:agent/);

  const active = renderToStaticMarkup(
    React.createElement(ChatPane, {
      ...props,
      onStopRun: () => undefined,
      runStopNotice: "That run already ended; nothing was stopped.",
    }),
  );
  assert.match(active, /data-testid="conv-action:stop-run"/);
  assert.match(active, /Stop run/);
  assert.match(active, /data-testid="run-stop-notice:agent"/);
  assert.match(active, /That run already ended; nothing was stopped\./);
});

function renderChat(args: {
  entries: ConversationTimelineEntry[];
  phase: "waiting" | "tool-executing" | "generating" | null;
  isLoadingHistory?: boolean;
  liveSpeech?: readonly LiveSpeechItem[];
  activeVoiceScope?: { sessionId: string; channelId: string } | null;
  approvalSnapshot?: PendingApprovalSnapshot;
}): string {
  return renderToStaticMarkup(
    React.createElement(ChatPane, {
      agent: {
        agent_id: "agent",
        member_id: "agent",
        identity: "agent",
        label: "Agent",
        kind: "mob_agent",
        role: "worker",
        state: "active",
        model_capabilities: { image_input: true },
      },
      agentLabel: "Agent",
      identity: "agent",
      entries: args.entries,
      approvalSnapshot: args.approvalSnapshot,
      phase: args.phase,
      isLoadingHistory: args.isLoadingHistory ?? false,
      draft: "",
      sending: false,
      readOnly: false,
      staged: [],
      onDraftChange: () => undefined,
      onStagedChange: () => undefined,
      onSend: () => true,
      liveSpeech: args.liveSpeech,
      activeVoiceScope: args.activeVoiceScope ?? null,
    }),
  );
}

test("live speech renders as distinct provisional rows, never as transcript messages", () => {
  const html = renderChat({
    entries: WORK_ENTRIES,
    phase: null,
    activeVoiceScope: { sessionId: "voice-session", channelId: "voice-channel" },
    liveSpeech: [
      { itemId: "item-1", speaker: "user", text: "What is the vault phrase", startedAt: 1, final: true },
      { itemId: "item-2", speaker: "assistant", text: "The vault phrase is", startedAt: 2, final: false },
    ],
  });
  assert.match(html, /chat-live-speech:agent/);
  assert.match(html, /chat-live-row:agent:item-1/);
  assert.match(html, /msg--live msg--live-user/);
  assert.match(html, /msg--live msg--live-assistant/);
  assert.match(html, /msg__live-label[^>]*>live</);
  assert.match(html, /data-live-final="false"/);
  // Provisional rows carry no copy affordance and are not counted as turns.
  const liveBlock = html.slice(html.indexOf("chat-live-speech:agent"));
  assert.doesNotMatch(liveBlock, /Copy (message|turn)/);
  // The two canonical entries still render as ordinary message rows; the live
  // rows are outside every turn container.
  assert.equal((html.match(/class="msg msg--(user|agent)"/g) || []).length, 2);
});

test("canonical voice rows replace only their exact provisional items during a call", () => {
  const origin = { sessionId: "voice-session", channelId: "voice-channel", canonicalRowSequence: 1, providerItemIds: ["spoken-a", "spoken-b"] };
  const canonical = { ...message({ id: "spoken-row", role: "assistant", createdAt: "2026-05-20T07:00:09.000Z", text: "The canonical spoken answer." }), realtimeOrigin: origin };
  const entries = [
    ...WORK_ENTRIES,
    message({ id: "typed-q", role: "user", createdAt: "2026-05-20T07:00:05.000Z", text: "Check the graph while we talk." }),
    message({ id: "typed-a", role: "assistant", createdAt: "2026-05-20T07:00:08.000Z", text: "The graph is ready." }),
    canonical,
  ];
  const liveSpeech: LiveSpeechItem[] = [
    { itemId: "spoken-a", speaker: "assistant", text: "Discarded first partial", startedAt: 1, final: false },
    { itemId: "spoken-b", speaker: "assistant", text: "Discarded late delta", startedAt: 2, final: true },
    { itemId: "spoken-next", speaker: "assistant", text: "An unfinished next answer", startedAt: 3, final: false },
    { itemId: "spoken-a", speaker: "user", text: "Unpaired user speech", startedAt: 4, final: true },
  ];
  const activeVoiceScope = { sessionId: origin.sessionId, channelId: origin.channelId };
  const during = renderChat({ entries, phase: null, activeVoiceScope, liveSpeech });
  assert.match(during, /Check the graph while we talk/);
  assert.match(during, /The graph is ready/);
  assert.match(during, /The canonical spoken answer/);
  assert.doesNotMatch(during, /Discarded (first partial|late delta)/);
  assert.match(during, /An unfinished next answer/);
  assert.match(during, /Unpaired user speech/);
  assert.match(during, /chat-live-row:agent:spoken-next/);
  const after = renderChat({ entries, phase: null });
  assert.match(after, /The canonical spoken answer/);
  assert.doesNotMatch(after, /chat-live-speech:agent/);
});

test("a committed row's realtime_origin.provider_item_ids retires the assistant caption of that segment", () => {
  // One provider turn segment is one committed row; its caption was keyed by the
  // segment item id Meerkat also records on the row. The frame goes through the
  // console's own history adapter, exactly as the timeline stream delivers it.
  const frame: ConsoleFrame = {
    id: "spoken-row", event: "text_complete", identity: "agent", runtimeKey: "runtime", sessionId: "voice-session",
    sourceKind: "session_history", timestampMs: 10,
    data: { text: "The vault phrase is amber.", result: "The vault phrase is amber.", message: {
      role: "block_assistant", blocks: [{ block_type: "transcript", data: { text: "The vault phrase is amber." } }],
      identity: { realtime_origin: {
        session_id: "voice-session", channel_id: "voice-channel", canonical_row_sequence: 4,
        provider_item_ids: ["segment-1"],
      } },
    } },
  };
  const entries = mapFramesToTimelineEntries(
    { agent_id: "agent", member_id: "agent", identity: "agent", label: "Agent", kind: "mob_agent" },
    [frame],
    { textMode: "markdown", renderInteractionStartsAsUser: true },
  );
  const liveSpeech: LiveSpeechItem[] = [
    { itemId: "segment-1", speaker: "assistant", text: "The vault phrase is", startedAt: 1, final: false },
    { itemId: "segment-2", speaker: "assistant", text: "Still speaking the next turn", startedAt: 2, final: false },
  ];
  const activeVoiceScope = { sessionId: "voice-session", channelId: "voice-channel" };
  const html = renderChat({ entries, phase: null, activeVoiceScope, liveSpeech });
  assert.match(html, /The vault phrase is amber\./);
  assert.doesNotMatch(html, /chat-live-row:agent:segment-1/);
  assert.match(html, /chat-live-row:agent:segment-2/);
  const before = renderChat({ entries: [], phase: null, activeVoiceScope, liveSpeech });
  assert.match(before, /chat-live-row:agent:segment-1/, "the caption shows until its row commits");
  const otherChannel = renderChat({ entries, phase: null, activeVoiceScope: { ...activeVoiceScope, channelId: "old-channel" }, liveSpeech });
  assert.match(otherChannel, /chat-live-row:agent:segment-1/, "a row from another channel never retires the caption");
});

test("voice pairing never uses timestamps, text, another scope or a missing provider key", () => {
  const speech: LiveSpeechItem = { itemId: "same-item", speaker: "assistant", text: "Same words", startedAt: 10, final: true };
  const origin = { sessionId: "voice-session", channelId: "voice-channel", canonicalRowSequence: 1, providerItemIds: [speech.itemId] };
  for (const realtimeOrigin of [undefined, { ...origin, providerItemIds: [] }, { ...origin, sessionId: "other-session" }, { ...origin, channelId: "old-channel" }]) {
    const entries = [{ ...message({ id: "canonical", role: "assistant", createdAt: "2026-05-20T07:00:09.000Z", text: speech.text }), realtimeOrigin }];
    const html = renderChat({ entries, phase: null, activeVoiceScope: { sessionId: "voice-session", channelId: "voice-channel" }, liveSpeech: [speech] });
    assert.match(html, /chat-live-row:agent:same-item/);
    assert.equal((html.match(/Same words/g) || []).length >= 2, true, "uncorrelated text is not an identity witness");
  }
  for (const activeVoiceScope of [undefined, { sessionId: "", channelId: origin.channelId }]) {
    const entries = [{ ...message({ id: "canonical", role: "assistant", createdAt: "invalid", text: "Canonical without a timestamp" }), realtimeOrigin: origin }];
    const html = renderChat({ entries, phase: null, activeVoiceScope, liveSpeech: [speech] });
    assert.match(html, /chat-live-row:agent:same-item/);
    assert.match(html, /Canonical without a timestamp/);
  }
});

test("typed realtime provenance also pairs a canonical user row when the host supplies it", () => {
  const entries = [{
    ...message({ id: "user-row", role: "user", createdAt: "invalid", text: "Canonical spoken question" }),
    realtimeOrigin: { sessionId: "voice-session", channelId: "voice-channel", canonicalRowSequence: 2, providerItemIds: ["user-item"] },
  }];
  const html = renderChat({ entries, phase: null, activeVoiceScope: { sessionId: "voice-session", channelId: "voice-channel" },
    liveSpeech: [{ itemId: "user-item", speaker: "user", text: "Provisional question", startedAt: 1, final: true }] });
  assert.match(html, /Canonical spoken question/);
  assert.doesNotMatch(html, /Provisional question|chat-live-speech:agent/);
});

const WORK_ENTRIES: ConversationTimelineEntry[] = [
  message({ id: "ask", role: "user", createdAt: "2026-05-20T06:43:02.000Z", text: "Please review the PR." }),
  message({ id: "answer", role: "assistant", createdAt: "2026-05-20T06:45:07.000Z", text: "Review complete.", runId: "review-run", runDurationMs: 125000 }),
];

test("chat pane shows the working indicator XOR the worked-for summary, never both", () => {
  // While the latest turn is still working, its "Worked for" summary must be
  // suppressed (otherwise it renders alongside the working indicator).
  const working = renderChat({ entries: WORK_ENTRIES, phase: "waiting" });
  assert.match(working, /chat-typing:agent/);
  assert.doesNotMatch(working, /Worked for/);

  // Once the turn is done (phase null) the summary shows and the indicator is gone.
  const done = renderChat({ entries: WORK_ENTRIES, phase: null });
  assert.doesNotMatch(done, /chat-typing:agent/);
  assert.match(done, /Worked for 2m 5s/);
});

test("chat pane shows a loading indicator while an empty session history is fetched", () => {
  const loading = renderChat({ entries: [], phase: null, isLoadingHistory: true });
  assert.match(loading, /Loading conversation/);
  assert.doesNotMatch(loading, /No messages yet/);

  const empty = renderChat({ entries: [], phase: null, isLoadingHistory: false });
  assert.match(empty, /No messages yet/);
  assert.doesNotMatch(empty, /Loading conversation/);
});

// ── WorkGraph inline card ───────────────────────────────────────────────────

const WORKGRAPH_ENTRY: ConversationTimelineEntry = {
  kind: "workgraph",
  id: "workgraph:goal-1",
  identity: AGENT,
  createdAt: "2026-05-20T06:44:00.000Z",
  rootId: "goal-1",
  title: "Release 0.7.30",
  objective: "Ship WorkGraph end to end",
  status: "active",
  progress: { completed: 1, total: 3 },
  items: [
    {
      itemId: "goal-1",
      title: "Release 0.7.30",
      status: "in_progress",
      priority: null,
      ownerLabel: null,
      revision: 4,
      depth: 0,
      parentId: null,
      description: "Ship WorkGraph end to end",
    },
    {
      itemId: "child-1",
      title: "Console card",
      status: "completed",
      priority: null,
      ownerLabel: "Planner",
      revision: 2,
      depth: 1,
      parentId: "goal-1",
    },
    {
      itemId: "child-2",
      title: "SDK parity",
      status: "open",
      priority: "high",
      ownerLabel: null,
      revision: 1,
      depth: 1,
      parentId: "goal-1",
    },
  ],
  attention: [
    {
      bindingId: "attention-1",
      mode: "pursue",
      statusLabel: "active",
      targetLabel: "sess-42",
      revision: 7,
    },
  ],
};

test("chat pane flattens workgraph entries into a dedicated card message", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({ id: "ask", role: "user", createdAt: "2026-05-20T06:43:02.000Z", text: "Plan the release." }),
    WORKGRAPH_ENTRY,
    message({ id: "answer", role: "assistant", createdAt: "2026-05-20T06:45:07.000Z", text: "On it." }),
  ]);

  const card = messages.find((entry) => entry.kind === "workgraph");
  assert.ok(card);
  assert.equal(card?.workGraphEntry?.rootId, "goal-1");
  // Copy/transcript surfaces get the textual projection.
  assert.match(card?.text || "", /Release 0\.7\.30 \(1\/3\)/);
});

test("chat pane renders the workgraph card inline without action buttons when no callbacks are provided", () => {
  const html = renderChat({
    entries: [
      message({ id: "ask", role: "user", createdAt: "2026-05-20T06:43:02.000Z", text: "Plan the release." }),
      WORKGRAPH_ENTRY,
    ],
    phase: null,
  });

  assert.match(html, /data-work-graph-card/);
  assert.match(html, /data-root-id="goal-1"/);
  assert.match(html, /data-status="active"/);
  assert.match(html, /data-testid="workgraph-card:goal-1"/);
  assert.match(html, /Release 0\.7\.30/);
  assert.match(html, /1\/3/);
  assert.match(html, /Console card/);
  assert.match(html, /pursue/);
  // Undefined-handler convention: no callbacks, no operator buttons.
  assert.doesNotMatch(html, /workgraph-action:/);
  assert.doesNotMatch(html, /workgraph-attention:/);
});

test("chat pane renders workgraph operator buttons only for provided callbacks", () => {
  const html = renderToStaticMarkup(
    React.createElement(ChatPane, {
      agent: {
        agent_id: "agent",
        member_id: "agent",
        identity: "agent",
        label: "Agent",
        kind: "mob_agent",
        role: "worker",
        state: "active",
        model_capabilities: { image_input: true },
      },
      agentLabel: "Agent",
      identity: "agent",
      entries: [WORKGRAPH_ENTRY],
      phase: null,
      draft: "",
      sending: false,
      readOnly: false,
      staged: [],
      onDraftChange: () => undefined,
      onStagedChange: () => undefined,
      onSend: () => true,
      workGraphActions: {
        onClaim: () => undefined,
        onAttentionPause: () => undefined,
      },
    }),
  );

  // Claim renders only on the open, unowned item.
  assert.match(html, /data-testid="workgraph-action:child-2:claim"/);
  assert.doesNotMatch(html, /workgraph-action:child-1:claim/);
  // Close callback was not provided — no Done buttons anywhere.
  assert.doesNotMatch(html, /:close"/);
  // Pause renders on the active binding.
  assert.match(html, /data-testid="workgraph-attention:attention-1:pause"/);
  assert.doesNotMatch(html, /workgraph-attention:attention-1:resume/);
});

test("windowTurnRail keeps short conversations un-windowed", () => {
  assert.deepEqual(windowTurnRail(1, 560), { start: 0, overflow: 0 });
  assert.deepEqual(windowTurnRail(20, 560), { start: 0, overflow: 0 });
  // Exactly at the height budget: still no windowing.
  const budget = Math.floor(560 / TURN_RAIL_TICK_PX);
  assert.deepEqual(windowTurnRail(Math.min(budget, TURN_RAIL_MAX_TICKS), 560), {
    start: 0,
    overflow: 0,
  });
});

test("windowTurnRail collapses long-running agents to the measured band", () => {
  // 200 turns in a 560px band: newest turns keep ticks, the rest collapse.
  const windowed = windowTurnRail(200, 560);
  assert.ok(windowed.overflow > 0, "long history must window");
  const visible = 200 - windowed.start;
  const budget = Math.min(TURN_RAIL_MAX_TICKS, Math.floor(560 / TURN_RAIL_TICK_PX));
  assert.ok(
    visible + 1 <= budget,
    `visible ticks (${visible}) + overflow slot must fit the budget (${budget})`,
  );
  assert.equal(windowed.start, windowed.overflow);
});

test("windowTurnRail respects tiny panes but never drops below the floor", () => {
  const tiny = windowTurnRail(200, 40); // 4 slots by height -> floor of 6 applies
  assert.ok(200 - tiny.start >= 5, "at least five recent turns stay railed");
  assert.ok(200 - tiny.start <= 6, "tiny panes stay tightly bounded");
});

test("windowTurnRail treats an unmeasured band as the hard ceiling", () => {
  const unmeasured = windowTurnRail(500, null);
  const visible = 500 - unmeasured.start;
  assert.ok(
    visible + 1 <= TURN_RAIL_MAX_TICKS,
    "without a measurement the hard ceiling caps the rail",
  );
});

test("long histories render a windowed rail with the overflow jump tick", () => {
  const entries: ConversationTimelineEntry[] = [];
  for (let index = 0; index < 120; index += 1) {
    entries.push(
      message({
        id: `u-${index}`,
        role: "user",
        createdAt: new Date(1720000000000 + index * 60000).toISOString(),
        text: `question ${index}`,
      }),
      message({
        id: `a-${index}`,
        role: "assistant",
        createdAt: new Date(1720000000000 + index * 60000 + 1000).toISOString(),
        text: `answer ${index}`,
      }),
    );
  }
  const html = renderToStaticMarkup(
    React.createElement(ChatPane, {
      agent: {
        agent_id: "long-agent",
        member_id: "long-agent",
        identity: "long-agent",
        label: "Long Agent",
        kind: "mob_agent",
        role: "worker",
        state: "active",
        model_capabilities: { image_input: false },
      },
      agentLabel: "Long Agent",
      identity: "long-agent",
      entries,
      phase: null,
      draft: "",
      sending: false,
      readOnly: true,
      staged: [],
      onDraftChange: () => undefined,
      onStagedChange: () => undefined,
      onSend: () => true,
    }),
  );
  const tickCount = (html.match(/chat-turn-rail:long-agent:\d+/g) ?? []).length;
  assert.ok(
    tickCount + 1 <= TURN_RAIL_MAX_TICKS,
    `rendered ticks (${tickCount}) must fit the ceiling`,
  );
  assert.ok(
    html.includes("chat-turn-rail:long-agent:overflow"),
    "the collapsed history exposes the overflow jump tick",
  );
  assert.ok(
    html.includes("earlier turns"),
    "the overflow preview names the collapsed count",
  );
});

test("transcript rows carry a typed source header, compact event rows, and day separators", () => {
  const peerEvent: ConversationTimelineEntry = {
    id: "peer-ingested",
    kind: "message",
    variant: "meta",
    identity: { id: "system", label: "System", role: "system" },
    createdAt: "2026-05-19T21:04:24.000Z",
    text: "Received a message from triage:main (example mob).",
    runtimeEvent: {
      eventType: "peer_content_ingested",
      kind: "message",
      peer: { id: "978419a8-69f6-5103-8d31-4482e1f76b52", displayName: "example/triage/mk--triage_cmain" },
      senderTaint: "tainted",
      payload: { kind: "message", sender_taint: "tainted", type: "peer_content_ingested" },
    },
  };
  const probe: ConversationTimelineEntry = {
    ...message({
      id: "probe",
      role: "user",
      createdAt: "2026-05-20T01:00:29.000Z",
      text: "Operator gate probe. Reply with exactly the token gate-turn-proof-1 and nothing else.",
    }),
  };
  const reply = message({ id: "reply", role: "assistant", createdAt: "2026-05-20T01:00:31.000Z", text: "gate-turn-proof-1" });
  const html = renderToStaticMarkup(
    React.createElement(ChatPane, {
      agent: null,
      agentLabel: "Triage",
      identity: "agent",
      entries: [peerEvent, probe, reply],
      phase: null,
      draft: "",
      sending: false,
      staged: [],
      onDraftChange: () => undefined,
      onStagedChange: () => undefined,
      onSend: () => true,
      peerLabels: new Map([["triage:main", "Triage"]]),
    }),
  );
  // The runtime event reads as a sentence with the roster label, a taint
  // badge, and its raw payload only inside the details disclosure.
  assert.match(html, /Received a message from Triage \(example mob\)\./);
  assert.match(html, /untrusted source/);
  assert.doesNotMatch(html, /peer_content_ingested: \{/);
  const beforeDetails = html.slice(0, html.indexOf("<details"));
  assert.doesNotMatch(beforeDetails, /sender_taint/);
  assert.match(html, /<summary>Event details<\/summary>/);
  // The probe has no typed origin, so it is a plain user message: never
  // classified by its text.
  assert.match(html, /class="msg__source">User message</);
  assert.match(html, /class="msg__source">Assistant</);
  // Icon copy buttons, no bare glyph in the text flow.
  assert.doesNotMatch(html, /⎘/);
  assert.match(html, /aria-label="Copy message"/);
  assert.match(html, /data-icon="copy"/);
  // Day separators at the first row and at the local day change.
  const days = html.match(/data-testid="chat-day:agent:[0-9-]+"/g) || [];
  const expectedDays = new Set([
    "2026-05-19T21:04:24.000Z",
    "2026-05-20T01:00:29.000Z",
  ].map((iso) => {
    const d = new Date(iso);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  }));
  assert.equal(days.length, expectedDays.size);
});

test("consecutive rows from the same assistant share one header", () => {
  const messages = __chatPaneTest.buildChatMessages([
    message({ id: "q", role: "user", createdAt: "2026-05-20T06:43:02.000Z", text: "Question" }),
    message({ id: "a1", role: "assistant", createdAt: "2026-05-20T06:43:05.000Z", text: "Part one" }),
    message({ id: "a2", role: "assistant", createdAt: "2026-05-20T06:43:09.000Z", text: "Part two" }),
  ]);
  assert.deepEqual(
    messages.map((m) => [m.id, m.showHeader]),
    [["q", true], ["a1", true], ["a2", false]],
  );
});

test("distinct assistant interactions retain equal replies and their own headers", () => {
  const base = message({ id: "first", role: "assistant", createdAt: "2026-05-20T06:43:05.000Z", text: "Ready." });
  for (const secondText of ["Ready.", "Acknowledged."]) {
    const messages = __chatPaneTest.buildChatMessages([
      { ...base, interactionId: "11111111-1111-4111-8111-111111111111", runId: "run-first" },
      { ...base, id: "second", text: secondText, interactionId: "22222222-2222-4222-8222-222222222222", runId: "run-second" },
    ]);
    assert.deepEqual(messages.map(row => [row.id, row.showHeader]), [["first", true], ["second", true]]);
  }
});

test("distinct assistant runs in one interaction retain equal replies and headers", () => {
  const base = { ...message({ id: "first", role: "assistant", createdAt: "2026-05-20T06:43:05.000Z", text: "Ready." }), interactionId: "11111111-1111-4111-8111-111111111111" };
  const messages = __chatPaneTest.buildChatMessages([
    { ...base, runId: "run-first" },
    { ...base, id: "second", runId: "run-second" },
  ]);
  assert.deepEqual(messages.map(row => [row.id, row.showHeader]), [["first", true], ["second", true]]);
});

test("Markdown copy and transcript export retain exact source whitespace", () => {
  const source = "  # A heading\n\nA line with a hard break  \nNext line\n\n";
  const entries: ConversationTimelineEntry[] = [{
    id: "raw-markdown", kind: "message", variant: "rich", identity: AGENT,
    blocks: [{ type: "markdown", id: "document", source, streaming: true }],
  }];
  const rows = __chatPaneTest.buildChatMessages(entries);
  assert.equal(__chatPaneTest.msgCopyText(rows[0]), source);
  assert.equal(__chatPaneTest.transcriptCopyText(rows), `Assistant - Agent: ${source}`);
  assert.equal(rows[0].scrollRowId, "raw-markdown");
});

test("Markdown rows remain distinct when whitespace carries source meaning", () => {
  const entries: ConversationTimelineEntry[] = ["one  \ntwo", "one two"].map((source, i) => ({
    id: `raw-${i}`, kind: "message", variant: "rich", identity: AGENT,
    blocks: [{ type: "markdown", id: `document-${i}`, source, streaming: false }],
  }));
  assert.equal(__chatPaneTest.buildChatMessages(entries).length, 2);
});

test("compact stock header retains target actions and destination", () => {
  const html = renderToStaticMarkup(React.createElement(ChatPane, { agent: null, agentLabel: "Agent", identity: "canonical-agent", entries: [], phase: null, draft: "", sending: false, staged: [], onDraftChange: () => {}, onStagedChange: () => {}, onSend: () => true, onInspect: () => {}, headerVariant: "compact" }));
  assert.match(html, /conv__head--compact/);
  assert.match(html, /title="canonical-agent"/);
  assert.match(html, /conv-action:details/);
  assert.match(html, /To:/);
  assert.doesNotMatch(html, /class="conv__identity"/);
});

test("stock folds consecutive proven generic completion but keeps unknown visible", () => {
  const makeTool = (id: string, name: string, known: boolean): ConversationTimelineEntry => ({ id, kind: "message", variant: "rich", identity: AGENT, blocks: [{ type: "tool-call", toolCallId: id, name, arguments: "{}", status: known ? "success" : "pending", completionEvidence: { outcome: known ? "success" : "unknown", source: known ? "session-history" : "unknown", toolCallId: id } }] });
  const html = renderToStaticMarkup(React.createElement(ChatPane, { agent: null, agentLabel: "Agent", identity: "agent", entries: [makeTool("one", "read_file", true), makeTool("two", "list_files", true), makeTool("three", "read_file", false)], phase: null, draft: "", sending: false, staged: [], onDraftChange: () => {}, onStagedChange: () => {}, onSend: () => true }));
  assert.match(html, /2 completed tool calls/);
  assert.match(html, /Completion unknown/);
  assert.match(html, /data-conversation-row-id="one"/);
  assert.match(html, /data-conversation-row-id="two"/);
});

test("stock chat renders one approval for a run continued after a user steer", () => {
  const entries = [
    { ...message({ id: "ask", role: "user", createdAt: "2026-09-26T05:00:00Z", text: "Start review" }), interactionId: "original" },
    { ...message({ id: "work", role: "assistant", createdAt: "2026-09-26T05:00:01Z", text: "Reviewing" }), interactionId: "original" },
    { ...message({ id: "steer", role: "user", createdAt: "2026-09-26T05:00:02Z", text: "Include deployment" }), interactionId: "steer" },
    { ...message({ id: "continue", role: "assistant", createdAt: "2026-09-26T05:00:03Z", text: "Deployment needs approval" }), interactionId: "original" },
  ];
  const request = normalizePendingApproval({ pending_id: "deployment", action: "Deploy reviewed change", action_id: "deploy", origin: { identity: "agent", interaction_id: "original" } })!;
  const html = renderChat({ entries, phase: null, approvalSnapshot: { scopeKey: "scope", status: "ready", requests: [request], decisions: {}, readOnly: false } });
  assert.equal((html.match(/data-testid="gating-pending:deployment"/g) || []).length, 1);
  const turnStart = html.indexOf('data-testid="chat-turn:agent:1"');
  assert.ok(turnStart >= 0 && html.indexOf('data-testid="gating-pending:deployment"') > turnStart);
});

test("stock chat retains typed local refusal and separate audit diagnostic cards", () => {
  const entries = [
    { kind: "operation-refused" as const, title: "Action refused", detail: "This action was refused under the current authorization, so it did not run." },
    { kind: "audit-unavailable" as const, title: "Audit update unavailable", detail: "Actual result unchanged.", operationId: "op-1" },
  ].map((operationFeedback, index): ConversationTimelineEntry => ({
    id: `feedback-${index}`, kind: "message", variant: "meta", identity: AGENT,
    createdAt: "2026-10-01T12:00:00Z", text: operationFeedback.detail, operationFeedback,
  }));
  const html = renderChat({ entries, phase: null });
  assert.match(html, /Action refused/);
  assert.match(html, /Audit update unavailable/);
  assert.match(html, /data-feedback-kind="operation-refused"/);
  assert.match(html, /data-feedback-kind="audit-unavailable"/);
  assert.doesNotMatch(html, /Event details/);
});
