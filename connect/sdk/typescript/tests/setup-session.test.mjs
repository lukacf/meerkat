import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { Session, EventStream, MeerkatClient } from "@rkat/sdk";
// Only test transport plumbing uses this installed SDK internal queue. The
// product imports public types and calls public Session methods exclusively.
import { AsyncQueue } from "../node_modules/@rkat/sdk/dist/streaming.js";
import { SetupConversationError } from "@rkat/toolkit/setup-session";
import { NativeSetupConversation } from "@rkat/toolkit/setup-session";
import { mountSetupConversation, mountToolkitView } from "@rkat/toolkit/ui";
import { JSDOM } from "jsdom";
import { fixture } from "../examples/management-fixture.mjs";

const SESSION = "11111111-1111-4111-8111-111111111111";
const OTHER = "22222222-2222-4222-8222-222222222222";
const RUN = "33333333-3333-4333-8333-333333333333";
const tick = () => new Promise(resolve => setImmediate(resolve));
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const rawResult = (sessionId = SESSION) => ({ session_id: sessionId, text: "not an install receipt", turns: 1,
  tool_calls: 0, usage: { input_tokens: 1, output_tokens: 1 } });
const row = (role, content) => MeerkatClient.parseSessionMessage({ role,
  created_at: "2026-10-06T00:00:00Z", content });
function harness(options = {}) {
  const invalidation = new AbortController(), sends = [], stops = [], reads = [], streams = [];
  let messages = options.messages ?? [row("user", "Configure Gmail")];
  const client = {
    async readSessionHistory(id, page) {
      reads.push({ id, ...page });
      if (options.history) return options.history(id, page);
      return { sessionId: id, messageCount: messages.length, offset: page.offset,
        hasMore: page.offset + page.limit < messages.length, messages: messages.slice(page.offset, page.offset + page.limit) };
    },
    _startTurnStreaming(id, message, turnOptions, session) {
      sends.push({ id, message, turnOptions });
      const queue = new AsyncQueue(), response = deferred();
      let seq = 0;
      const control = {
        event(payload) { queue.put({ event_id: "44444444-4444-4444-8444-444444444444",
          source: { type: "session", session_id: SESSION }, seq: seq++, timestamp_ms: 1, payload }); },
        finish(result = rawResult()) { response.resolve(result); queue.put(null); },
        fail() { response.reject(new Error("RAW_PROVIDER_SECRET")); },
      };
      streams.push(control);
      return new EventStream({ sessionId: id, eventQueue: queue, responsePromise: response.promise,
        parseResult: MeerkatClient.parseRunResult, session });
    },
    async _stopRun(id, runId, reason) {
      stops.push({ id, runId, reason });
      return options.stop ? options.stop(id, runId, reason)
        : { session_id: id, receipt: { outcome: "stopped", run_id: runId, contributors: [] } };
    },
    _interrupt() { throw new Error("Broad interrupt must never be called"); },
  };
  const session = new Session(client, MeerkatClient.parseRunResult(rawResult()));
  const conversation = new NativeSetupConversation({ session, scope: "admitted-viewer",
    invalidated: invalidation.signal, revalidate: options.revalidate ?? (() => true) });
  return { conversation, session, invalidation, sends, stops, reads, streams,
    setMessages(value) { messages = value; } };
}
function started(stream, sessionId = SESSION, runId = RUN) {
  stream.event({ type: "run_started", session_id: sessionId,
    input: { kind: "content", content: "Native input" }, identity: { run_id: runId } });
}
function mount(h) {
  const dom = new JSDOM("<main></main><aside></aside>");
  const container = dom.window.document.querySelector("main");
  const view = mountSetupConversation(container, { scope: "admitted-viewer", conversation: h.conversation });
  return { dom, container, view,
    button(text) { return [...container.querySelectorAll("button")].find(b => b.textContent === text); },
    send(message) { container.querySelector("textarea").value = message;
      container.querySelector("form").dispatchEvent(new dom.window.Event("submit", { bubbles: true, cancelable: true })); },
    close() { view.dispose(); dom.window.close(); } };
}

// A display-only host bridge fixture. It supplies public getters, not a native
// Session, private notice body, dismissal operation or new lifecycle owner.
function privateNoticePort(options = {}) {
  const listeners = new Set(), unavailable = new Set();
  const h = { reads: 0, sends: [], stops: [], messages: [], history: undefined, send: undefined };
  const conversation = {
    scope: "admitted-viewer", available: true, canSend: true, canStop: false,
    outcomeUncertain: false, privateNoticePending: false, ...options,
    onStateChange(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    onUnavailable(listener) { unavailable.add(listener); return () => unavailable.delete(listener); },
    async history() {
      h.reads += 1;
      return h.history ? h.history() : { messages: h.messages, offset: 0, total: h.messages.length, hasMore: false };
    },
    send(message, observe) { h.sends.push(message); return h.send(message, observe); },
    async stopObservedRun() { h.stops.push(true); return "not_current"; },
  };
  return Object.assign(h, { conversation, listeners,
    change(values) { Object.assign(conversation, values); for (const listener of listeners) listener(); },
    revoke() { conversation.available = false; conversation.canSend = false; conversation.canStop = false;
      for (const listener of unavailable) listener(); },
  });
}

test("blocked send without a notice projection uses neutral status on mount and state change", async t => {
  const h = privateNoticePort({ canSend: false });
  // A bridge may omit the optional display projection while its completed
  // turn waits for a private decision. Send availability is not run evidence.
  delete h.conversation.privateNoticePending;
  h.messages = [{ role: "assistant", text: "The turn finished. Review the protected panel." }];
  const ui = mount(h); t.after(() => ui.close()); await tick();
  const status = () => ui.container.querySelector('[role="status"]').textContent;
  const paused = "Sending is paused. Check the protected setup panel or current host status.";
  assert.equal(status(), paused);
  assert.doesNotMatch(status(), /still being handled|responding/);
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.button("Stop this agent run").disabled, true);
  ui.container.querySelector("textarea").value = "Keep my follow-up";
  h.change({ canSend: true }); await tick();
  assert.equal(ui.button("Send message").disabled, false);
  h.change({ canSend: false }); // The second generic status path, without a read.
  assert.equal(status(), paused);
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.button("Stop this agent run").disabled, true);
  assert.equal(ui.container.querySelector("textarea").readOnly, false);
  assert.equal(ui.container.querySelector("textarea").value, "Keep my follow-up");
  assert.equal(h.reads, 2);
  assert.deepEqual(h.sends, []);
  assert.deepEqual(h.stops, []);
});

test("private notice wait is not an active model turn and dismissal preserves the draft", async t => {
  const h = privateNoticePort({ canSend: false, privateNoticePending: true });
  h.messages = [{ role: "assistant", text: "The host has a review link." }];
  const first = mount(h); t.after(() => first.close()); await tick();
  const status = () => first.container.querySelector('[role="status"]').textContent;
  assert.match(status(), /private setup notice/);
  assert.doesNotMatch(status(), /still being handled|responding/);
  assert.equal(first.button("Send message").disabled, true);
  assert.equal(first.button("Stop this agent run").disabled, true);
  assert.equal(first.container.querySelector("textarea").readOnly, false);
  first.send("My next question");
  h.change({ privateNoticePending: true }); // Between two retained notices.
  assert.equal(h.reads, 1); assert.equal(h.sends.length, 0);
  h.change({ privateNoticePending: false, canSend: true });
  assert.match(status(), /You can send a message/);
  assert.equal(first.button("Send message").disabled, false);
  assert.equal(first.container.querySelector("textarea").value, "My next question");
  assert.equal(h.reads, 1); assert.equal(h.stops.length, 0);
  first.view.dispose(); assert.equal(h.listeners.size, 0);
  h.change({ privateNoticePending: true, canSend: false });
  const reopened = mount(h); t.after(() => reopened.close()); await tick();
  assert.match(reopened.container.querySelector('[role="status"]').textContent, /private setup notice/);
  h.revoke();
  assert.equal(reopened.container.querySelector('[aria-label="Saved conversation"]').children.length, 0);
  assert.match(reopened.container.querySelector('[role="status"]').textContent, /no longer available/);
  assert.equal(h.sends.length, 0); assert.equal(h.stops.length, 0);
});

test("remounted active send refreshes canonical history when it settles into a private notice wait", async t => {
  const h = privateNoticePort(), completion = deferred();
  h.send = (_message, observe) => {
    h.change({ canSend: false, canStop: true }); observe({ kind: "run_observed" }); return completion.promise;
  };
  const first = mount(h); t.after(() => first.close()); await tick();
  first.send("Prepare this frontend"); await tick(); first.view.dispose();
  const reopened = mount(h); t.after(() => reopened.close()); await tick();
  assert.match(reopened.container.querySelector('[role="status"]').textContent, /Sending is paused/);
  reopened.container.querySelector("textarea").value = "Keep my follow-up";
  h.messages = [{ role: "user", text: "Prepare this frontend" }, { role: "assistant", text: "Saved final response" }];
  // Native publishes this coherent state after joining the exact completion,
  // before the first private display request. The bridge resolves that send.
  h.change({ canSend: false, canStop: false, privateNoticePending: true }); completion.resolve();
  await tick(); await tick();
  assert.match(reopened.container.querySelector('[aria-label="Saved conversation"]').textContent, /Saved final response/);
  assert.match(reopened.container.querySelector('[role="status"]').textContent, /private setup notice/);
  assert.equal(h.reads, 3);
  h.change({ privateNoticePending: true });
  assert.equal(h.reads, 3);
  h.change({ privateNoticePending: false, canSend: true });
  assert.match(reopened.container.querySelector('[role="status"]').textContent, /You can send a message/);
  assert.equal(reopened.container.querySelector("textarea").value, "Keep my follow-up");
  assert.equal(reopened.button("Stop this agent run").disabled, true);
  assert.deepEqual(h.sends, ["Prepare this frontend"]); assert.equal(h.stops.length, 0);
});

test("private notice dismissal cannot repair a failed history read or clear uncertainty", async t => {
  const h = privateNoticePort({ canSend: false, privateNoticePending: true });
  h.history = () => { throw new SetupConversationError("history_unavailable"); };
  const ui = mount(h); t.after(() => ui.close()); await tick();
  const status = () => ui.container.querySelector('[role="status"]').textContent;
  assert.match(status(), /history is unavailable/); assert.match(status(), /private setup notice/);
  ui.container.querySelector("textarea").value = "Keep this draft";
  h.change({ privateNoticePending: false, canSend: true });
  assert.match(status(), /history is unavailable/);
  assert.doesNotMatch(status(), /private setup notice/);
  assert.equal(ui.button("Send message").disabled, true);
  h.history = undefined;
  h.change({ canSend: false, outcomeUncertain: true, privateNoticePending: true });
  await ui.view.refresh();
  assert.match(status(), /outcome still requires host reconciliation/);
  h.change({ privateNoticePending: false });
  assert.match(status(), /outcome still requires host reconciliation/);
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.container.querySelector("textarea").readOnly, true);
  assert.equal(ui.container.querySelector("textarea").value, "Keep this draft");
  assert.equal(h.sends.length, 0); assert.equal(h.stops.length, 0);
});

test("original sending view shows the notice wait after its joined completion without clearing later edits", async t => {
  const h = privateNoticePort(), completion = deferred();
  h.send = (_message, observe) => {
    h.change({ canSend: false, canStop: true }); observe({ kind: "run_observed" }); return completion.promise;
  };
  const ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("Inspect this frontend"); await tick();
  const input = ui.container.querySelector("textarea");
  assert.equal(input.value, "");
  input.value = "A later question"; input.dispatchEvent(new ui.dom.window.Event("input"));
  h.messages = [{ role: "user", text: "Inspect this frontend" }, { role: "assistant", text: "Saved exact response" }];
  h.change({ canStop: false, privateNoticePending: true }); completion.resolve();
  await tick(); await tick();
  assert.match(ui.container.querySelector('[role="status"]').textContent, /private setup notice/);
  assert.match(ui.container.querySelector('[aria-label="Saved conversation"]').textContent, /Saved exact response/);
  assert.equal(ui.container.querySelector('[aria-label="Message accepted for current run"]').children.length, 0);
  assert.equal(input.value, "A later question");
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.button("Stop this agent run").disabled, true);
  h.change({ privateNoticePending: false, canSend: true });
  assert.equal(h.reads, 2); assert.equal(ui.button("Send message").disabled, false);
  assert.deepEqual(h.sends, ["Inspect this frontend"]); assert.equal(h.stops.length, 0);
});

test("component controls use exact released SDK Session, parser and EventStream", async () => {
  const metadata = JSON.parse(await readFile(new URL("../node_modules/@rkat/sdk/package.json", import.meta.url)));
  assert.equal(metadata.version, "0.9.4");
  const h = harness(), events = [];
  assert.ok(h.session instanceof Session);
  const message = "  Set up Gmail and a Slack frontend.\nKeep my complete request.  ";
  const sending = h.conversation.send(message, e => events.push(e)); await tick();
  started(h.streams[0]);
  h.streams[0].event({ type: "text_delta", delta: "Which account?", assistant_message_id: "message-one" });
  h.streams[0].finish(); await sending;
  assert.equal(h.sends[0].message, message);
  assert.deepEqual(events, [{ kind: "run_observed" }, { kind: "text_delta", text: "Which account?", messageId: "message-one" }]);
  assert.equal(h.conversation.canSend, true);
  assert.equal(h.session.lastResult.text, "not an install receipt");
});

test("native block_assistant history projects text and pages latest without tool or reasoning data", async () => {
  const assistant = MeerkatClient.parseSessionMessage({ role: "block_assistant", created_at: "now", blocks: [
    { block_type: "text", data: { text: "Choose the account" } },
    { block_type: "reasoning", data: { text: "PRIVATE_REASONING" } },
  ], results: [], raw: { credential: "PRIVATE_RAW" } });
  const messages = Array.from({ length: 51 }, (_, i) => row("user", `Message ${i}`));
  messages.push(assistant, row("tool", "PRIVATE_TOOL"));
  const h = harness({ messages });
  const page = await h.conversation.history();
  assert.deepEqual(h.reads.map(r => r.offset), [0, 3]);
  assert.equal(page.offset, 3); assert.equal(page.total, 53); assert.equal(page.hasMore, false);
  assert.equal(page.messages.at(-1).text, "Choose the account");
  assert.ok(!JSON.stringify(page).includes("PRIVATE"));
  const earlier = await h.conversation.history(0);
  assert.equal(earlier.hasMore, true); assert.equal(earlier.messages[0].text, "Message 0");
  assert.equal(h.sends.length, 0);
});

test("host denial before send and delayed history revalidation expose no data", async () => {
  let admitted = true;
  const pending = deferred();
  const h = harness({ revalidate: () => admitted, history: () => pending.promise });
  const reading = h.conversation.history(); await tick(); admitted = false;
  pending.resolve({ sessionId: SESSION, messageCount: 1, offset: 0, hasMore: false,
    messages: [row("user", "LATE_SECRET")] });
  await assert.rejects(reading, { code: "unavailable" });
  await assert.rejects(h.conversation.send("request", () => {}), { code: "unavailable" });
  assert.equal(h.sends.length, 0);
});

test("initial history failure preserves the draft and refresh never sends it", async t => {
  let historyUnavailable = true;
  const h = harness({ history(id, page) {
    if (historyUnavailable) throw new Error("RAW_HISTORY_SECRET");
    return { sessionId: id, messageCount: 0, offset: page.offset, hasMore: false, messages: [] };
  } });
  const ui = mount(h); t.after(() => ui.close()); await tick();
  assert.equal(ui.container.querySelector('[role="status"]').textContent,
    "Conversation history is unavailable. Refresh history to try again.");
  assert.ok(!ui.container.textContent.includes("RAW_HISTORY_SECRET"));
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.button("Refresh history").disabled, false);
  ui.send("Prepare a Slack frontend");
  assert.equal(h.sends.length, 0);
  historyUnavailable = false;
  await ui.view.refresh();
  assert.equal(ui.container.querySelector("textarea").value, "Prepare a Slack frontend");
  assert.equal(ui.button("Send message").disabled, false);
  assert.equal(h.sends.length, 0);
});

test("successful send followed by failed history refresh is not presented as an unsent message", async t => {
  let historyUnavailable = false, messages = [];
  const h = harness({ history(id, page) {
    if (historyUnavailable) throw new Error("RAW_HISTORY_SECRET");
    return { sessionId: id, messageCount: messages.length, offset: page.offset, hasMore: false, messages };
  } });
  const ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("Prepare a Slack frontend"); await tick(); started(h.streams[0]);
  h.streams[0].event({ type: "text_complete", content: "Select the persistent host." });
  historyUnavailable = true;
  h.streams[0].finish(); await tick(); await tick();
  assert.match(ui.container.querySelector('[role="status"]').textContent,
    /Your message was sent, but saved conversation could not be refreshed/);
  assert.ok(!ui.container.textContent.includes("No message was sent"));
  assert.ok(!ui.container.textContent.includes("RAW_HISTORY_SECRET"));
  assert.equal(ui.container.querySelector("textarea").value, "");
  assert.equal(ui.container.querySelector('[aria-label="Agent response in progress"]').textContent,
    "Select the persistent host.");
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.button("Refresh history").disabled, false);
  assert.equal(h.conversation.outcomeUncertain, false);
  assert.equal(h.sends.length, 1);
  historyUnavailable = false;
  messages = [row("user", "Prepare a Slack frontend"), row("assistant", "Select the persistent host.")];
  await ui.view.refresh();
  assert.match(ui.container.querySelector('[aria-label="Saved conversation"]').textContent,
    /Prepare a Slack frontend.*Select the persistent host\./);
  assert.equal(ui.container.querySelector('[aria-label="Agent response in progress"]').textContent, "");
  assert.equal(ui.button("Send message").disabled, false);
  assert.equal(h.sends.length, 1);
  assert.equal(h.stops.length, 0);
});

test("one local reservation precedes asynchronous admission", async () => {
  const admission = deferred(); const h = harness({ revalidate: () => admission.promise });
  const first = h.conversation.send("first", () => {});
  await assert.rejects(h.conversation.send("second", () => {}), { code: "busy" });
  admission.resolve(true); await tick(); h.streams[0].finish(); await first;
  assert.equal(h.sends.length, 1);
});

test("exact run stop preserves not_current receipt and never targets a replacement", async () => {
  const h = harness({ stop: (id, runId) => ({ session_id: id,
    receipt: { outcome: "not_current", run_id: runId, current_run_id: "replacement" } }) });
  const sending = h.conversation.send("request", () => {}); await tick();
  await assert.rejects(h.conversation.stopObservedRun("early"), { code: "stop_unavailable" });
  started(h.streams[0]); await tick();
  const a = h.conversation.stopObservedRun("chosen reason"), b = h.conversation.stopObservedRun("duplicate");
  assert.equal(a, b); assert.equal(await a, "not_current");
  assert.deepEqual(h.stops, [{ id: SESSION, runId: RUN, reason: "chosen reason" }]);
  h.streams[0].finish(); await sending;
});

test("crossed primary session or run cannot become a stop target or successful turn", async () => {
  for (const cross of ["session", "run"]) {
    const h = harness(), events = [];
    const sending = h.conversation.send("request", e => events.push(e)); await tick();
    started(h.streams[0]);
    started(h.streams[0], cross === "session" ? OTHER : SESSION, cross === "run" ? "other-run" : RUN);
    h.streams[0].event({ type: "text_delta", delta: "CROSSED" }); await tick();
    assert.equal(h.conversation.canStop, false);
    h.streams[0].finish(); await assert.rejects(sending, { code: "turn_uncertain" });
    assert.ok(!JSON.stringify(events).includes("CROSSED")); assert.equal(h.stops.length, 0);
  }
});

test("stream invalidation clears display, discards late events and does not stop native work", async t => {
  const h = harness(), ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("request"); await tick(); started(h.streams[0]);
  h.streams[0].event({ type: "text_delta", delta: "Visible before revoke" }); await tick();
  assert.match(ui.container.textContent, /Visible before revoke/);
  h.invalidation.abort();
  h.streams[0].event({ type: "text_delta", delta: "LATE_SECRET" });
  h.streams[0].finish(); await tick(); await tick();
  assert.ok(!ui.container.textContent.includes("Visible before revoke"));
  assert.ok(!ui.container.textContent.includes("LATE_SECRET"));
  assert.equal(ui.container.querySelector("textarea").value, "");
  assert.equal(h.stops.length, 0); assert.equal(h.session.lastResult.text, "not an install receipt");
});

test("uncertain stream failure is value-free and cannot replay after history refresh or remount", async t => {
  let historyUnavailable = false;
  const h = harness({ history(id, page) {
    if (historyUnavailable) throw new Error("RAW_HISTORY_SECRET");
    return { sessionId: id, messageCount: 0, offset: page.offset, hasMore: false, messages: [] };
  } });
  const ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("one request"); await tick(); h.streams[0].fail(); await tick(); await tick();
  assert.equal(h.conversation.outcomeUncertain, true);
  assert.match(ui.container.textContent, /outcome is unconfirmed/);
  assert.ok(!ui.container.textContent.includes("RAW_PROVIDER_SECRET"));
  historyUnavailable = true;
  await ui.view.refresh();
  assert.match(ui.container.querySelector('[role="status"]').textContent,
    /history is unavailable.*outcome still requires host reconciliation.*has not been retried/);
  assert.ok(!ui.container.textContent.includes("No message was sent"));
  assert.ok(!ui.container.textContent.includes("Your message was sent"));
  assert.ok(!ui.container.textContent.includes("RAW_HISTORY_SECRET"));
  assert.equal(ui.button("Send message").disabled, true);
  assert.equal(ui.container.querySelector("textarea").readOnly, true);
  assert.equal(h.sends.length, 1);
  historyUnavailable = false;
  await ui.view.refresh(); assert.equal(ui.button("Send message").disabled, true);
  ui.view.dispose(); const second = mount(h); t.after(() => second.close()); await tick();
  assert.equal(second.button("Send message").disabled, true);
  await assert.rejects(h.conversation.send("one request", () => {}), { code: "turn_uncertain" });
  assert.equal(h.sends.length, 1);
});

test("provider re-poll resets exact live message and native history replaces drafts", async t => {
  const h = harness(), ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("request"); await tick(); started(h.streams[0]);
  const turn = { type: "turn_started", turn_number: 1, assistant_message_id: "same-message" };
  h.streams[0].event(turn); h.streams[0].event({ type: "text_delta", delta: "old", assistant_message_id: "same-message" });
  h.streams[0].event(turn); h.streams[0].event({ type: "text_delta", delta: "new", assistant_message_id: "same-message" });
  await tick(); assert.equal(ui.container.querySelector('[aria-label="Agent response in progress"]').textContent, "new");
  h.setMessages([row("user", "request"), row("assistant", "Saved reply")]);
  h.streams[0].finish(); await tick(); await tick();
  assert.match(ui.container.textContent, /Saved reply/);
  assert.equal(ui.container.querySelector('[aria-label="Agent response in progress"]').textContent, "");
  assert.ok(!ui.container.textContent.includes("not an install receipt"));
});

test("view disposal does not stop or resend, and remount reads native persisted messages", async t => {
  const h = harness(), ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("request"); await tick(); started(h.streams[0]); await tick(); ui.view.dispose();
  h.setMessages([row("user", "request"), row("assistant", "Native saved response")]);
  h.streams[0].finish(); await tick(); await tick();
  const second = mount(h); t.after(() => second.close()); await tick();
  assert.match(second.container.textContent, /Native saved response/);
  assert.equal(h.sends.length, 1); assert.equal(h.stops.length, 0);
});

test("pending conversation remount follows the original send to saved completion", async t => {
  const h = harness(), first = mount(h); t.after(() => first.close()); await tick();
  first.send("Prepare a frontend"); await tick(); started(h.streams[0]); await tick();
  first.view.dispose();
  const reopened = mount(h); t.after(() => reopened.close()); await tick();
  assert.equal(reopened.button("Send message").disabled, true);
  assert.equal(reopened.button("Stop this agent run").disabled, false);
  reopened.container.querySelector("textarea").value = "Keep this follow-up draft";
  h.setMessages([row("user", "Prepare a frontend"), row("assistant", "Review the prepared frontend.")]);
  h.streams[0].finish(); await tick(); await tick();
  assert.match(reopened.container.querySelector('[aria-label="Saved conversation"]').textContent,
    /Review the prepared frontend\./);
  assert.equal(reopened.button("Send message").disabled, false);
  assert.equal(reopened.button("Stop this agent run").disabled, true);
  assert.equal(reopened.container.querySelector("textarea").value, "Keep this follow-up draft");
  assert.equal(h.reads.length, 3);
  assert.equal(h.sends.length, 1); assert.equal(h.stops.length, 0);
});

test("pending remount observes a late run and shares one exact Stop", async t => {
  const stopped = deferred();
  const h = harness({ stop: () => stopped.promise }), first = mount(h);
  t.after(() => first.close()); await tick();
  first.send("Investigate a service"); await tick(); first.view.dispose();
  const reopened = mount(h); t.after(() => reopened.close()); await tick();
  assert.equal(reopened.button("Stop this agent run").disabled, true);
  started(h.streams[0]); await tick();
  assert.equal(reopened.button("Stop this agent run").disabled, false);
  reopened.button("Stop this agent run").click();
  reopened.button("Stop this agent run").click(); await tick();
  assert.equal(reopened.button("Stop this agent run").disabled, true);
  assert.equal(h.stops.length, 1);
  assert.equal(h.stops[0].id, SESSION); assert.equal(h.stops[0].runId, RUN);
  assert.equal(h.reads.length, 2);
  stopped.resolve({ session_id: SESSION, receipt: { outcome: "stopped", run_id: RUN, contributors: [] } });
  await tick();
  assert.equal(reopened.button("Send message").disabled, true);
  assert.equal(h.reads.length, 2);
  h.setMessages([row("user", "Investigate a service"), row("assistant", "The original run stopped.")]);
  h.streams[0].finish(); await tick(); await tick();
  assert.match(reopened.container.textContent, /The original run stopped\./);
  assert.equal(reopened.button("Send message").disabled, false);
  assert.equal(h.sends.length, 1); assert.equal(h.stops.length, 1);
});

test("pending remount preserves uncertain settlement without replay", async t => {
  const h = harness(), first = mount(h); t.after(() => first.close()); await tick();
  first.send("Prepare an integration"); await tick(); started(h.streams[0]); await tick();
  first.view.dispose(); const reopened = mount(h); t.after(() => reopened.close()); await tick();
  h.streams[0].fail(); await tick(); await tick();
  assert.match(reopened.container.querySelector('[role="status"]').textContent,
    /earlier message outcome still requires host reconciliation/);
  assert.equal(reopened.button("Send message").disabled, true);
  assert.equal(reopened.button("Stop this agent run").disabled, true);
  assert.equal(reopened.container.querySelector("textarea").readOnly, true);
  assert.ok(!reopened.container.textContent.includes("RAW_PROVIDER_SECRET"));
  await reopened.view.refresh();
  reopened.send("Prepare an integration");
  assert.equal(h.sends.length, 1); assert.equal(h.stops.length, 0);
});

test("remount settlement supersedes an older history read and revocation wins", async t => {
  for (const outcome of ["completed", "history_failed", "revoked"]) {
    const oldPage = deferred(), currentPage = deferred(); let reads = 0;
    const page = text => ({ sessionId: SESSION, messageCount: 1, offset: 0,
      hasMore: false, messages: [row("assistant", text)] });
    const h = harness({ history: () => ++reads === 1 ? page("Initial history")
      : reads === 2 ? oldPage.promise : currentPage.promise });
    const first = mount(h); t.after(() => first.close()); await tick();
    first.send("Prepare a frontend"); await tick(); started(h.streams[0]); await tick();
    first.view.dispose(); const reopened = mount(h); t.after(() => reopened.close()); await tick();
    assert.equal(reads, 2);
    h.streams[0].finish(); await tick(); await tick();
    assert.equal(reads, 3);
    if (outcome === "revoked") h.invalidation.abort();
    if (outcome === "history_failed") currentPage.reject(new Error("RAW_HISTORY_SECRET"));
    else currentPage.resolve(page("Current saved answer"));
    await tick(); await tick();
    oldPage.resolve(page("STALE_SAVED_ANSWER")); await tick(); await tick();
    assert.ok(!reopened.container.textContent.includes("STALE_SAVED_ANSWER"));
    assert.ok(!reopened.container.textContent.includes("RAW_HISTORY_SECRET"));
    if (outcome === "completed") {
      assert.match(reopened.container.textContent, /Current saved answer/);
      assert.equal(reopened.button("Send message").disabled, false);
    } else {
      assert.ok(!reopened.container.textContent.includes("Current saved answer"));
      assert.equal(reopened.button("Send message").disabled, true);
      assert.match(reopened.container.querySelector('[role="status"]').textContent,
        outcome === "revoked" ? /no longer available/ : /history is unavailable/);
    }
    assert.equal(h.sends.length, 1); assert.equal(h.stops.length, 0);
  }
});

test("state invalidation has no payload and detached or throwing listeners cannot control a send", async () => {
  const h = harness(), notifications = [];
  const detach = h.conversation.onStateChange((...args) => notifications.push(args));
  const detachThrowing = h.conversation.onStateChange(() => { throw new Error("PRIVATE_LISTENER_ERROR"); });
  const sending = h.conversation.send("One original message", () => {}); await tick();
  assert.ok(notifications.length > 0);
  started(h.streams[0]); await tick();
  assert.ok(notifications.every(args => args.length === 0));
  const detachedCount = notifications.length; detach(); detachThrowing();
  h.streams[0].finish(); await sending;
  assert.equal(notifications.length, detachedCount);
  assert.equal(h.conversation.canSend, true);
  assert.equal(h.sends.length, 1); assert.equal(h.stops.length, 0);
});

test("form text stays inert and management service/frontend observations remain separate", async t => {
  const h = harness({ messages: [row("assistant", '<img src="https://invalid.test/">')] });
  const ui = mount(h); t.after(() => ui.close());
  const management = mountToolkitView(ui.dom.window.document.querySelector("aside"), {
    scope: "fixture-viewer", dispatch: async () => {},
  }); t.after(() => management.dispose()); management.update(fixture());
  await tick(); assert.equal(ui.container.querySelectorAll("img,script,iframe").length, 0);
  assert.ok(ui.container.querySelector("textarea").closest("label"));
  assert.equal(ui.container.querySelector('[role="status"]').getAttribute("aria-live"), "polite");
  assert.match(ui.container.textContent, /does not approve an installation/);
  assert.match(ui.dom.window.document.querySelector("aside").textContent, /frontend setup pending/);
  assert.match(ui.dom.window.document.querySelector("aside").textContent, /Access ready for this task/);
});


test("setup transcript renders assistant Markdown safely and keeps exact source copy", async t => {
  const source = '### Choices\n\n**Read first**, then `inspect`.\n\n- One\n- Two\n\n[Docs](https://example.test/help)\n\n<script>ALERT</script>\n\n[Bad](javascript:alert(1)) ![Remote](https://example.test/tracker.png)\n\n```js\nconst x = "<tag>";\n```';
  const h = harness({ messages: [row("user", "**Keep my literal request**"), row("assistant", source)] });
  const ui = mount(h); t.after(() => ui.close()); await tick();
  const history = ui.container.querySelector('[aria-label="Saved conversation"]');
  assert.equal(history.querySelector("h3").textContent, "Choices");
  assert.equal(history.querySelector("li:first-child p").textContent, "**Keep my literal request**");
  assert.equal(history.querySelector("ul").children.length, 2);
  assert.equal(history.querySelector("pre code").textContent, 'const x = "<tag>";');
  assert.equal(history.querySelector('a[href="https://example.test/help"]').rel, "noopener noreferrer");
  assert.equal(history.querySelectorAll("script,img,iframe,style").length, 0);
  assert.equal(history.querySelectorAll("a").length, 1);
  const copied = [];
  Object.defineProperty(ui.dom.window.navigator, "clipboard", { value: { writeText: async text => copied.push(text) } });
  const buttons = [...history.querySelectorAll("button")];
  buttons.at(-1).click(); await tick();
  assert.deepEqual(copied, [source]);
});

test("empty public assistant rows are hidden without changing native page coordinates", async t => {
  const toolOnly = MeerkatClient.parseSessionMessage({ role: "block_assistant", created_at: "now",
    blocks: [{ block_type: "tool_use", data: { id: "tool-only", name: "PRIVATE_TOOL", args: {} } }], results: [] });
  const h = harness({ messages: [row("assistant", "  \n "), toolOnly, row("assistant", "Visible reply")] });
  const page = await h.conversation.history();
  assert.equal(page.total, 3); assert.equal(page.offset, 0);
  const ui = mount(h); t.after(() => ui.close()); await tick();
  assert.equal(ui.container.querySelector('[aria-label="Saved conversation"]').children.length, 1);
  assert.match(ui.container.textContent, /Visible reply/);
});

test("run observation clears only the accepted draft before completion and preserves the next draft", async t => {
  const h = harness(), ui = mount(h); t.after(() => ui.close()); await tick();
  const input = ui.container.querySelector("textarea");
  ui.send("First request"); await tick();
  assert.equal(input.value, "First request");
  const admitted = ui.container.querySelector('[aria-label="Message accepted for current run"]');
  assert.equal(admitted.children.length, 0);
  assert.equal(ui.button("Send message").disabled, true);
  started(h.streams[0]); await tick();
  assert.equal(input.value, "");
  assert.equal(admitted.querySelector("p").textContent, "First request");
  assert.equal(input.readOnly, false);
  assert.equal(ui.button("Send message").disabled, true);
  input.value = "A follow-up draft"; input.dispatchEvent(new ui.dom.window.Event("input"));
  started(h.streams[0]); await tick();
  assert.equal(input.value, "A follow-up draft");
  assert.equal(admitted.querySelectorAll("p").length, 1);
  h.setMessages([row("user", "First request"), row("assistant", "Saved result")]);
  h.streams[0].finish(); await tick(); await tick();
  assert.equal(input.value, "A follow-up draft");
  assert.equal(admitted.children.length, 0);
  assert.equal(ui.container.querySelectorAll('.toolkit-plain-message').length, 1);
  assert.equal(h.sends.length, 1);
});

test("a draft edited before run observation is not cleared even if it returns to identical text", async t => {
  const h = harness(), ui = mount(h); t.after(() => ui.close()); await tick();
  const input = ui.container.querySelector("textarea");
  ui.send("Same text"); await tick();
  input.value = "Edited"; input.dispatchEvent(new ui.dom.window.Event("input"));
  input.value = "Same text"; input.dispatchEvent(new ui.dom.window.Event("input"));
  started(h.streams[0]); await tick();
  assert.equal(input.value, "Same text");
  h.streams[0].finish(); await tick(); await tick();
  assert.equal(input.value, "Same text");
});

test("uncertain unobserved sends retain their draft and live Markdown resets do not leave empty placeholders", async t => {
  const h = harness(), ui = mount(h); t.after(() => ui.close()); await tick();
  ui.send("Retain if unconfirmed"); await tick(); h.streams[0].fail(); await tick(); await tick();
  assert.equal(ui.container.querySelector("textarea").value, "Retain if unconfirmed");
  const h2 = harness(), second = mount(h2); t.after(() => second.close()); await tick();
  second.send("Request"); await tick(); started(h2.streams[0]);
  h2.streams[0].event({ type: "turn_started", turn_number: 1, assistant_message_id: "m" }); await tick();
  const live = second.container.querySelector('[aria-label="Agent response in progress"]');
  assert.equal(live.children.length, 0);
  h2.streams[0].event({ type: "text_delta", delta: "### Working\n\n**Result**", assistant_message_id: "m" }); await tick();
  assert.equal(live.querySelector("h3").textContent, "Working");
  h2.streams[0].event({ type: "turn_started", turn_number: 2, assistant_message_id: "m" }); await tick();
  assert.equal(live.children.length, 0);
  h2.streams[0].finish(); await tick(); await tick();
});


test("Markdown entities, tables, nested lists and inert destinations follow the supported profile", async t => {
  const source = '## Plan &amp; details\n\nA &copy; &copy &notit; &#x1F680; &lt;img src=x&gt; \\*literal\\*.\n\n`&amp; <b>`\n\n> Quoted *choice*\n\n3. Third\n   - Child\n4. Fourth\n\n| Key | Value |\n| --- | --- |\n| **A** | `B` |\n\n- [x] Finished\n- [ ] Waiting\n\n<svg onload=alert(1)><script>alert(1)</script></svg>\n<style>body{display:none}</style>\n<iframe src="https://example.test/"></iframe>\n\n[one](javascript:alert(1)) [two](jav&#x61;script:alert(1)) [three](data:text/html,test) [four](vbscript:evil) [five](//example.test/x) [six](/local) [seven](file:///private/a) [eight](https://name:password@example.test) [safe](https://example.test/a?x=1&amp;y=2)';
  const h = harness({ messages: [row("assistant", source)] }), ui = mount(h); t.after(() => ui.close()); await tick();
  const body = ui.container.querySelector(".toolkit-markdown");
  assert.equal(body.querySelector("h3").textContent, "Plan & details");
  assert.match(body.textContent, /A © &copy &notit; 🚀 <img src=x> \*literal\*/);
  assert.equal(body.querySelector("code").textContent, "&amp; <b>");
  assert.equal(body.querySelector("blockquote em").textContent, "choice");
  assert.equal(body.querySelector("ol").start, 3);
  assert.equal(body.querySelector("ol ul li").textContent, "Child");
  assert.equal(body.querySelectorAll('th[scope="col"]').length, 2);
  assert.equal(body.querySelector("td strong").textContent, "A");
  assert.equal(body.querySelector('[aria-label="Completed task: "]').textContent, "[x] ");
  assert.equal(body.querySelectorAll("svg,script,style,iframe,img,input").length, 0);
  assert.deepEqual([...body.querySelectorAll("a")].map(a => a.href), ["https://example.test/a?x=1&y=2"]);
  assert.ok(!body.querySelector("[onload],[onclick],[src]"));
});

test("oversized Markdown remains complete plain text and copy failure is local to the control", async t => {
  const source = "**complete** ".repeat(10_000);
  const h = harness({ messages: [row("assistant", source)] }), ui = mount(h); t.after(() => ui.close()); await tick();
  const body = ui.container.querySelector(".toolkit-markdown");
  assert.equal(body.querySelector("pre").textContent, source);
  Object.defineProperty(ui.dom.window.navigator, "clipboard", { value: { writeText() { throw new Error("PRIVATE_COPY_FAILURE"); } } });
  ui.button("Copy message").click(); await tick();
  assert.equal(ui.button("Select text to copy").disabled, false);
  assert.ok(!ui.container.textContent.includes("PRIVATE_COPY_FAILURE"));
  assert.equal(ui.button("Send message").disabled, false);
});

test("admission refusal preserves the unsent draft and completion fallback never clears an edit", async t => {
  for (const outcome of ["refused", "completed"]) {
    const result = deferred(); let sends = 0;
    const conversation = { scope: "admitted-viewer", available: true, canSend: true, canStop: false, outcomeUncertain: false,
      onUnavailable: () => () => {}, onStateChange: () => () => {},
      history: async () => ({ messages: [], offset: 0, total: 0, hasMore: false }),
      send: async () => { sends += 1; await result.promise; }, stopObservedRun: async () => "not_current" };
    const ui = mount({ conversation }); t.after(() => ui.close()); await tick();
    ui.send("Original"); await tick();
    const input = ui.container.querySelector("textarea");
    if (outcome === "refused") result.reject(new SetupConversationError("busy"));
    else { input.value = "New draft"; input.dispatchEvent(new ui.dom.window.Event("input")); result.resolve(); }
    await tick(); await tick();
    assert.equal(input.value, outcome === "refused" ? "Original" : "New draft");
    assert.equal(sends, 1);
  }
});
