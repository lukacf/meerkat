"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { createHash } = require("node:crypto");
const { assertAssistantIdentity, assertProvisionalPresentation, assertProductionAssets, source } = require("./real-assistant-identity.cjs");

function fixture() {
  const rows = ["a", "b", "c"].map(assistant_message_id => ({
    role: "block_assistant", assistant_message_id,
    identity: { run_id: "run", interaction_id: "input" },
    blocks: [{ block_type: "text", data: { text: source } }],
  }));
  let next = 0;
  const frame = (kind, payload) => ({
    id: `event-${++next}`, kind, payload, source: { kind: "console_event" },
    session_id: "session", run_id: "run", interaction_id: "input",
  });
  const frames = rows.flatMap(row => [
    frame("turn_started", { assistant_message_id: row.assistant_message_id }),
    frame("text_delta", { assistant_message_id: row.assistant_message_id, delta: source }),
    frame("text_complete", { assistant_message_id: row.assistant_message_id, content: source }),
    frame("turn_completed", { assistant_message_id: row.assistant_message_id }),
  ]);
  frames.push(frame("interaction_complete", { assistant_message_id: "c", source_event_type: "run_completed" }));
  return { history: { session_id: "session", has_more: false, messages: rows }, frames,
    owner: { session_id: "session", run_id: "run", interaction_id: "input" } };
}

test("identity acceptance rejects missing, duplicated, cross-session and misbound occurrences", () => {
  const good = fixture();
  assert.deepEqual(assertAssistantIdentity(good.history, good.frames, good.owner), ["a", "b", "c"]);
  const missing = structuredClone(good); delete missing.history.messages[1].assistant_message_id;
  assert.throws(() => assertAssistantIdentity(missing.history, missing.frames, missing.owner), /identity/);
  const duplicated = structuredClone(good); duplicated.history.messages[1].assistant_message_id = "a";
  assert.throws(() => assertAssistantIdentity(duplicated.history, duplicated.frames, duplicated.owner), /distinct/);
  const wrongSession = structuredClone(good); wrongSession.frames[1].session_id = "fork";
  assert.throws(() => assertAssistantIdentity(wrongSession.history, wrongSession.frames, wrongSession.owner), /session/);
  const wrongFinal = structuredClone(good); wrongFinal.frames.at(-1).payload.assistant_message_id = "a";
  assert.throws(() => assertAssistantIdentity(wrongFinal.history, wrongFinal.frames, wrongFinal.owner), /final/);
  const damaged = structuredClone(good); damaged.history.messages[1].blocks[0].data.text += " ";
  assert.throws(() => assertAssistantIdentity(damaged.history, damaged.frames, damaged.owner), /exact/);
  const lostDelta = structuredClone(good); lostDelta.frames = lostDelta.frames.filter(frame => !(frame.kind === "text_delta" && frame.payload.assistant_message_id === "b"));
  assert.throws(() => assertAssistantIdentity(lostDelta.history, lostDelta.frames, lostDelta.owner), /live text/);
});

test("streaming acceptance cannot pass on completed history or a later identical reply", () => {
  const good = { streaming: true, source: source.slice(0, 32), assistantMessageId: "first",
    firstTextComplete: false, documentId: "live-document", quoteId: "live-frame" };
  assert.doesNotThrow(() => assertProvisionalPresentation(good));
  for (const invalid of [
    { ...good, streaming: false },
    { ...good, source },
    { ...good, source: "Release evidence" },
    { ...good, firstTextComplete: true },
    { ...good, assistantMessageId: undefined },
  ]) assert.throws(() => assertProvisionalPresentation(invalid));
});

test("production identity evidence rejects a fixture host or missing, stale and failed browser assets", () => {
  const baseUrl = "http://127.0.0.1:12345";
  const expected = Object.fromEntries(["console-app.js", "console-app.css"].map(file => {
    const bytes = Buffer.from(`current production ${file}`);
    return [file, { sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length }];
  }));
  const assets = Object.entries(expected).map(([file, value]) => ({
    url: `${baseUrl}/console/assets/${file}?v=${value.sha256.slice(0, 12)}`,
    status: 200, ...value,
  }));
  assert.doesNotThrow(() => assertProductionAssets(`${baseUrl}/console`, baseUrl, assets, expected));
  for (const route of ["/scoped", "/shared"]) {
    assert.throws(() => assertProductionAssets(baseUrl + route, baseUrl, assets, expected), /shipping browser entry/);
  }
  assert.throws(() => assertProductionAssets("http://127.0.0.1:54321/console", baseUrl, assets, expected), /fixture origin/);
  for (const invalid of [
    assets.slice(1),
    [...assets, assets[0]],
    [{ ...assets[0], sha256: "outdated build" }, assets[1]],
    [{ ...assets[0], bytes: 0 }, assets[1]],
    [{ ...assets[0], status: 404 }, assets[1]],
    [{ ...assets[0], error: "response body unavailable" }, assets[1]],
    [{ ...assets[0], url: `${baseUrl}/console/assets/console-app.js?v=outdated` }, assets[1]],
    [{ ...assets[0], url: assets[0].url.replace("12345", "54321") }, assets[1]],
  ]) assert.throws(() => assertProductionAssets(`${baseUrl}/console`, baseUrl, invalid, expected));
});
