import assert from "node:assert/strict";
import test from "node:test";
import * as core from "./index";
import type { ConsoleFrame } from "./runtime-types";

// Resolve through the public surface so the RED baseline asserts the missing
// consumer contract instead of failing to bundle an absent implementation.
const { assistantMessageId, assistantMessageKey, hasAssistantMessageIdCarrier } = core as unknown as {
  assistantMessageId: (frame: ConsoleFrame) => string | undefined;
  assistantMessageKey: (frame: ConsoleFrame) => string | undefined;
  hasAssistantMessageIdCarrier: (frame: ConsoleFrame) => boolean;
};
const { assistantHistorySnapshot, assistantMessageCursorSequence } = core as unknown as {
  assistantHistorySnapshot: (frame: ConsoleFrame) => { sessionId: string; observedThrough: bigint; assistantMessageIds: ReadonlySet<string> } | undefined;
  assistantMessageCursorSequence: (value: unknown) => bigint | undefined;
};
const frame = (data: unknown, sessionId: string | undefined = "session-a"): ConsoleFrame => ({
  id: "source-frame", event: "text_delta", data, sessionId,
});

test("assistant identity: the public helper contract exists", () => {
  assert.equal(typeof assistantMessageId, "function");
  assert.equal(typeof assistantMessageKey, "function");
  assert.equal(typeof hasAssistantMessageIdCarrier, "function");
});

test("assistant identity: absent and null carriers remain legacy", () => {
  for (const data of [undefined, null, false, "text", [], {}, { assistant_message_id: null }]) {
    assert.equal(assistantMessageId(frame(data)), undefined);
    assert.equal(assistantMessageKey(frame(data)), undefined);
    assert.equal(hasAssistantMessageIdCarrier(frame(data)), false);
  }
});

test("assistant identity: malformed and blank carriers cannot become keys or legacy matches", () => {
  for (const id of [undefined, false, 1, [], {}, "", " \t\n"]) {
    const input = frame({ assistant_message_id: id });
    assert.equal(assistantMessageId(input), undefined);
    assert.equal(assistantMessageKey(input), undefined);
    assert.equal(hasAssistantMessageIdCarrier(input), true);
  }
});

test("assistant identity: opaque IDs compare exactly without trimming, case folding or UUID parsing", () => {
  for (const id of ["0195a285-d5ef-7000-8000-000000000001", "OPAQUE/CaseSensitive", " legacy-id ", "A\u030A/🚀"]) {
    const input = frame({ assistant_message_id: id });
    assert.equal(assistantMessageId(input), id);
    assert.equal(assistantMessageKey(input), JSON.stringify(["session-a", id]));
    assert.equal(hasAssistantMessageIdCarrier(input), true);
  }
  assert.notEqual(assistantMessageKey(frame({ assistant_message_id: "A" })), assistantMessageKey(frame({ assistant_message_id: "a" })));
});

test("assistant identity: forks and tuple delimiter collisions cannot share keys", () => {
  const id = "inherited-id";
  assert.notEqual(assistantMessageKey(frame({ assistant_message_id: id }, "parent")), assistantMessageKey(frame({ assistant_message_id: id }, "fork")));
  assert.notEqual(assistantMessageKey(frame({ assistant_message_id: "b:c" }, "a")), assistantMessageKey(frame({ assistant_message_id: "c" }, "a:b")));
  assert.notEqual(assistantMessageKey(frame({ assistant_message_id: "id" }, "a ")), assistantMessageKey(frame({ assistant_message_id: "id" }, "a")));
});

test("assistant identity: only an actual session and the direct payload carrier authorize correspondence", () => {
  for (const sessionId of [undefined, "", " \t"]) {
    const input = { ...frame({ assistant_message_id: "id", session_id: "payload-session" }), sessionId, runId: "run", interactionId: "interaction", identity: "agent", runtimeKey: "runtime" };
    assert.equal(assistantMessageId(input), "id");
    assert.equal(assistantMessageKey(input), undefined);
    assert.equal(hasAssistantMessageIdCarrier(input), true);
  }
  for (const data of [
    { message: { assistant_message_id: "nested-message" } },
    { image: { assistant_message_id: "nested-image" }, id: "provider-item" },
    { frame: frame({ assistant_message_id: "nested-update" }) },
    Object.create({ assistant_message_id: "prototype" }),
  ]) {
    assert.equal(assistantMessageId(frame(data)), undefined);
    assert.equal(hasAssistantMessageIdCarrier(frame(data)), false);
  }
});

const snapshotFrame = (data: Record<string, unknown> = {}, extra: Partial<ConsoleFrame> = {}): ConsoleFrame => ({
  id: "snapshot-frame", event: "assistant_history_snapshot", sourceKind: "session_history",
  sessionId: "session-a", cursor: "console:101",
  data: { session_id: "session-a", complete: true, observed_through: "console:100", assistant_message_ids: ["CaseSensitive/ID"], ...data },
  ...extra,
});

test("assistant snapshot: console cursor parser preserves exact u64 ordering", () => {
  assert.equal(typeof assistantMessageCursorSequence, "function");
  for (const [value, expected] of [["console:0", 0n], ["console:10", 10n], ["console:9007199254740993", 9007199254740993n], ["console:18446744073709551615", 18446744073709551615n]] as const) {
    assert.equal(assistantMessageCursorSequence(value), expected);
  }
  for (const value of [undefined, null, 10, "10", "console:", "console:-1", "console:+1", "console:1.2", "console:1e2", "console:0x10", "console:18446744073709551616", "console:1\n", "console:1\r", "console:1\u2028", "console:1\u2029", " console:1", "console:1 ", "other:1"]) {
    assert.equal(assistantMessageCursorSequence(value), undefined, String(value));
  }
});

test("assistant snapshot: only a complete owner-scoped marker establishes absence", () => {
  assert.equal(typeof assistantHistorySnapshot, "function");
  const snapshot = assistantHistorySnapshot(snapshotFrame());
  assert.ok(snapshot);
  assert.equal(snapshot.sessionId, "session-a");
  assert.equal(snapshot.observedThrough, 100n);
  assert.deepEqual([...snapshot.assistantMessageIds], ["CaseSensitive/ID"]);
  assert.deepEqual([...assistantHistorySnapshot(snapshotFrame({ assistant_message_ids: [] }))!.assistantMessageIds], []);
  const exact = assistantHistorySnapshot(snapshotFrame({ session_id: "session-a ", assistant_message_ids: [" id ", "ID", "id"] }, { sessionId: "session-a " }));
  assert.deepEqual([...exact!.assistantMessageIds], [" id ", "ID", "id"]);
  assert.equal(exact!.sessionId, "session-a ");
});

test("assistant snapshot: malformed scope, completeness and identity arrays invalidate the whole marker", () => {
  for (const extra of [
    { event: "text_complete" }, { sourceKind: "console_event" }, { sourceKind: undefined },
    { sessionId: undefined }, { sessionId: "" }, { sessionId: " " },
    { sessionId: "other-session" }, { data: null }, { data: [] },
  ]) assert.equal(assistantHistorySnapshot(snapshotFrame({}, extra)), undefined);
  for (const data of [
    { session_id: undefined }, { session_id: "session-a " }, { complete: false }, { complete: "true" },
    { complete: undefined }, { assistant_message_ids: undefined }, { assistant_message_ids: null },
    { assistant_message_ids: "id" }, { assistant_message_ids: ["ok", ""] },
    { assistant_message_ids: ["ok", " \t"] }, { assistant_message_ids: ["ok", null] },
    { assistant_message_ids: ["ok", 23] }, { assistant_message_ids: ["duplicate", "duplicate"] },
  ]) assert.equal(assistantHistorySnapshot(snapshotFrame(data)), undefined, JSON.stringify(data));
});

test("assistant snapshot: covered frontier must precede its valid marker cursor", () => {
  for (const observed_through of [undefined, 100, "100", "console:101", "console:102", "console:-1", "console:1\n"]) {
    assert.equal(assistantHistorySnapshot(snapshotFrame({ observed_through })), undefined);
  }
  for (const cursor of [undefined, "", "console:100", "console:99", "other:101", "console:18446744073709551616"]) {
    assert.equal(assistantHistorySnapshot(snapshotFrame({}, { cursor })), undefined);
  }
  const snapshot = assistantHistorySnapshot(snapshotFrame({ observed_through: "console:9007199254740992" }, { cursor: "console:9007199254740993" }));
  assert.equal(snapshot?.observedThrough, 9007199254740992n);
});
