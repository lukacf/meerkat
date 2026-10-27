#!/usr/bin/env node

// PAID, opt-in acceptance: npm run build && npm run e2e:voice:live
// Offline evidence/fixture checks: node voice-e2e-live.cjs --self-test
// Native local WebRTC clock check: node voice-e2e-live.cjs --self-test-audio
// Optional META_VOICE_GATEWAY_BIN uses an already-built openai-live gateway.
// No provider/RPC mocks, human microphone, external demo, native text injection,
// output ACK fabrication, or default-CI calls. Only getUserMedia is substituted.
// Typed input goes to the existing background agent; it is NOT Live user text:
// https://developers.openai.com/api/docs/guides/live-delegation#accept-typed-input

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const readline = require("node:readline");
const { spawn, spawnSync } = require("node:child_process");
const { setTimeout: sleep } = require("node:timers/promises");

const ROOT = path.resolve(__dirname, "..");
const ASSETS = path.join(__dirname, "test-assets", "voice-live");
const PRIMARY = "voice-primary";
const KEEPER = "voice-keeper";
const LABEL = "Voice Acceptance Agent";
const COVERAGE = {
  closeLoad: "Keeper's real foreground shell HTTP request is blocked at a test-owned external service through close/reopen; NOT inferred pending mirror/ACK",
  ownerAckBoundary: "Separate upstream deterministic owner tests gate append after generated authorization and before provider ACK",
  appendObservability: "No public provider-append count; this lane checks peer sends, canonical finals and native spoken-result duplicates",
};
const HELP = `PAID Meta console voice acceptance (gpt-live-1 + original gpt-5.5 agent)

  npm --prefix console run e2e:voice:live
  node console/voice-e2e-live.cjs --self-test
  node console/voice-e2e-live.cjs --self-test-audio
  node console/voice-e2e-live.cjs --time-to-talk [--runs=3] [--seed-turns=40] [--seed-words=40] [--hold-ms=0]
  node console/voice-e2e-live.cjs --help

Prepare the embedded console with npm --prefix console run build first.
The runner does not modify generated bundles. It builds meta_gateway using
scripts/repo-cargo --locked, --features openai-live and CARGO_INCREMENTAL=0.
META_VOICE_GATEWAY_BIN=/absolute/path selects an already-built current gateway.
OPENAI_API_KEY or OPENAI_API_KEY_OLD is required for the paid lane; absence fails.
META_VOICE_KEEP_ARTIFACTS=1 retains private temporary diagnostics after success.

Only microphone capture is substituted, using portable original WAV fixtures.
Provider, RPC, runtime authority and peer model calls are real. No human mic,
existing demo, invented playback ACK or direct Live user-text command is used.
The synthetic microphone continuously emits zero PCM between WAV utterances.
--self-test-audio verifies actual local WebRTC RTP before/after speech and while
muted, decoded speech, and clock cleanup; it makes no provider calls.
--time-to-talk is a PAID diagnostic: it seeds the original agent's session with
N typed turns, then times click -> able to talk stage by stage (browser marks,
each control-plane RPC, status polls, WebRTC connected, microphone enabled) for
the requested number of open/close runs and prints per-run and median/max
tables. --seed-words sets the requested reply length per seeded turn and
--hold-ms keeps each call open until context preparation settles (bounded), so
the concurrent summary's duration is observed too.
META_VOICE_ALLOW_STALE_BUNDLE=1 accepts an override gateway whose embedded
console differs from the current build (older commits under test), and
META_VOICE_TTT_RUST_LOG overrides the gateway's RUST_LOG for that run.

Coverage: speech-triggered real keeper verification and keeper-only knowledge;
typed exact-value recall; delayed peer results;
close with BACKGROUND work pending; same-agent reopen; one old ordinary backend
operation held across reopen, released afterwards and spoken as current canonical
context; stale receipt/request controls must leave the new call working.
The keeper alone enables the existing shell tool. A foreground curl request
waits at a test-owned loopback service (240s maximum); native shell-start plus
the live held HTTP request prove in-flight work. Cleanup cancels the service.

Coverage complement: ${COVERAGE.ownerAckBoundary}.
This paid lane neither runs nor attests that upstream gate. Public status is
lifecycle-only; no public/trusted projection exposes pending context-outbox ACKs.
${COVERAGE.appendObservability}.
Old-origin ordinary facts are allowed in the current canonical context; old-call
receipts are not allowed to control the new call. No default CI paid calls.
`;
const WORDS = ["amber", "birch", "coral", "delta", "elm", "fern", "gold", "hazel",
  "iris", "jade", "kiwi", "lemon", "maple", "north", "opal", "pine"];
const PHRASES = {
  reverse: "Please get the keeper's verification code for these three words.",
  finish: "Have the keeper reverse the words and report its code with the verified answer.",
  recall: "What is the exact current console value? Say all four words in order.",
  peer: "Please get the launch code from the keeper. Tell me when it is verified.",
  overlap: "Please start the overlap voice check with the keeper. Tell me when the result arrives.",
};
const normalize = value => String(value).toLowerCase().replace(/[^a-z0-9]/g, "");
const contains = (text, expected) => normalize(text).includes(normalize(expected));
const randomWords = count => WORDS.map(word => ({ word, n: crypto.randomBytes(8).toString("hex") }))
  .sort((a, b) => a.n.localeCompare(b.n)).slice(0, count).map(row => row.word);
const log = (phase, values = {}) => console.log(JSON.stringify({ phase, ...values }));
let secrets = [];
const redact = value => secrets.reduce((text, secret) => text.split(secret).join("[REDACTED]"), String(value))
  .replace(/Bearer\s+[A-Za-z0-9._~-]+/gi, "Bearer [REDACTED]")
  .replace(/\bsk-[A-Za-z0-9_-]+/g, "[REDACTED]");
function childEnv() {
  const env = { ...process.env };
  delete env.OPENAI_API_KEY;
  delete env.OPENAI_API_KEY_OLD;
  return env;
}
async function poll(label, probe, timeout = 90_000, interval = 200) {
  const deadline = Date.now() + timeout;
  do {
    const result = await probe();
    if (result) return result;
    await sleep(interval);
  } while (Date.now() < deadline);
  throw new Error(`Timed out waiting for ${label} (${timeout}ms)`);
}
function text(value) {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(text).join(" ");
  if (!value || typeof value !== "object") return "";
  return text(value.text ?? value.result ?? value.content ?? "");
}
// A tool's output travels on its result event. The execution fact for the
// same call carries no body; a session-history result row keeps the body
// under the execution fact's kind.
function toolResultWithBody(frame) {
  return ["tool_result_received", "tool_execution_completed"].includes(frame.kind) &&
    (frame.payload?.result !== undefined || frame.payload?.content !== undefined);
}
function providerFinalTextBlock(block) {
  return block.block_type === "text" && typeof block.data?.text === "string" &&
    block.data.meta?.provider === "open_ai_assistant_message" && block.data.meta.phase === "final_answer" &&
    typeof block.data.meta.response_id === "string" && block.data.meta.response_id.length > 0;
}
function finalText(frame, expectedScope = {}) {
  // Recognize a provider final message committed to canonical history.
  // text_complete alone is not final, and this does not assert run terminality.
  // Native member progress and tool/close witnesses retain that responsibility.
  const message = frame.payload?.message;
  if (frame.kind !== "text_complete" || frame.source?.kind !== "session_history" || frame.status !== "completed" ||
    frame.payload?.is_error === true || message?.role !== "block_assistant" || message.stop_reason !== "end_turn" ||
    typeof message.assistant_message_id !== "string" || !message.assistant_message_id ||
    frame.payload.assistant_message_id !== message.assistant_message_id || !Array.isArray(message.blocks)) return "";
  for (const key of ["identity", "session_id", "run_id"]) {
    if (typeof frame[key] !== "string" || !frame[key]) return "";
  }
  for (const key of ["identity", "session_id", "run_id", "interaction_id"]) {
    if (Object.hasOwn(expectedScope, key) && (typeof expectedScope[key] !== "string" || !expectedScope[key] ||
      frame[key] !== expectedScope[key])) return "";
  }
  if (message.identity?.run_id !== frame.run_id ||
    (message.identity?.interaction_id ?? null) !== (frame.interaction_id ?? null) ||
    (frame.interaction_id != null && (typeof frame.interaction_id !== "string" || !frame.interaction_id))) return "";
  return message.blocks.filter(providerFinalTextBlock).map(block => block.data.text).join("");
}
function canonicalInputText(message) {
  if (message.role === "system_notice") return typeof message.body === "string" ? message.body : null;
  if (typeof message.content === "string") return message.content;
  if (Array.isArray(message.content) &&
    message.content.every(block => block.type === "text" && typeof block.text === "string")) {
    return message.content.map(block => block.text).join("");
  }
  return null;
}
function assertCanonicalTypedUser(snapshot, sessionId, interactionId, content) {
  assert.equal(snapshot.sessionId, sessionId, "Typed input must be persisted on the ORIGINAL canonical session");
  assert.ok(typeof interactionId === "string" && interactionId.length > 0, "Console acceptance must name its exact interaction");
  assert.ok(Array.isArray(snapshot.messages), "Committed canonical messages must be available");
  const candidates = snapshot.messages.filter(message =>
    ["user", "system_notice"].includes(message.role) &&
    (canonicalInputText(message) === content || message.identity?.interaction_id === interactionId));
  assert.equal(candidates.length, 1, "The typed request must occur exactly once as canonical input, not only as a visible send frame");
  const message = candidates[0];
  assert.equal(message.role, "user", "SystemNotice ExternalEvent cannot substitute for genuine canonical Message::User");
  // Meerkat omits transcript_role for its serde-default Conversational role.
  assert.ok(message.transcript_role === undefined || message.transcript_role === "conversational",
    "Typed console input must be conversational, not injected context or a compaction summary");
  assert.equal(message.identity?.interaction_id, interactionId, "Canonical User must carry the EXACT console interaction ID");
  assert.equal(canonicalInputText(message), content, "Canonical User must preserve the complete typed request exactly");
  return message;
}
// Admission and canonical persistence are different witnesses. History twins
// remain in the public timeline and must not inflate the admission count.
function assertTypedSendAdmission(frames, scope, content) {
  assert.ok(typeof scope.interaction_id === "string" && scope.interaction_id.length > 0,
    "Typed admission must name its exact interaction");
  const admissions = frames.filter(frame => frame.kind === "user_input" &&
    frame.source?.kind === "send" && frame.interaction_id === scope.interaction_id);
  assert.equal(admissions.length, 1, "Typed request must have exactly one console send admission");
  const admission = admissions[0];
  for (const key of ["runtime_key", "identity", "session_id", "interaction_id"]) {
    assert.ok(typeof scope[key] === "string" && scope[key].length > 0, "Typed admission scope must be explicit");
    assert.equal(admission[key], scope[key], "Typed admission must preserve exact " + key);
  }
  assert.equal(admission.payload?.content, content, "Typed admission must preserve the complete typed request exactly");
  return admission;
}
function readCommittedTypedSource(directory, sessionId) {
  // The timeline retains send admission and committed-history projections.
  // Prove canonical persistence from the fixture's committed whole-blob
  // authority, never a provisional tail or the legacy compatibility table.
  const database = path.join(directory, "state", "runtime.sqlite");
  fs.accessSync(database, fs.constants.R_OK);
  const result = spawnSync("python3", ["-c", `
import json, sqlite3, sys
from pathlib import Path
database, session_id = sys.argv[1:]
connection = sqlite3.connect(Path(database).as_uri() + "?mode=ro", uri=True, timeout=5)
try:
    rows = connection.execute("""
        SELECT authority.store_revision, authority.blob_sha256, bodies.session_snapshot
        FROM runtime_whole_blob_authority AS authority
        JOIN runtime_whole_blob_bodies AS bodies ON bodies.blob_sha256 = authority.blob_sha256
        WHERE authority.session_id = ?
    """, (session_id,)).fetchall()
    if len(rows) != 1:
        raise RuntimeError("Expected exactly one committed whole-blob authority for the fixture session; no legacy/provisional fallback")
    revision, digest, data = rows[0]
    session = json.loads(data)
    if session.get("id") != session_id or not isinstance(session.get("messages"), list):
        raise RuntimeError("Committed session envelope identity/messages mismatch")
    print(json.dumps({
        "sessionId": session_id, "storeRevision": revision, "blobSha256": digest,
        "messages": [message for message in session["messages"] if message.get("role") in ("user", "system_notice")]
    }))
finally:
    connection.close()
`, database, sessionId], { env: childEnv(), encoding: "utf8", timeout: 10_000, maxBuffer: 4 * 1024 * 1024 });
  assert.equal(result.status, 0, `Read-only canonical-source probe failed: ${redact(result.error?.message ?? result.stderr)}`);
  return JSON.parse(result.stdout);
}
function hasPendingResults(frames, expected) {
  return expected.some(value => !frames.some(frame => contains(finalText(frame), value)));
}
function bootstrapMemberIdle(identity, status, member) {
  // response_phase is a console interaction projection and can retain
  // "waiting" after durable completion. Machine progress owns quiescence.
  return status.identity === identity && status.state === "active" &&
    typeof status.session_id === "string" && member.current_session_id === status.session_id &&
    member.error === null && member.progress?.run_state === "idle" && member.progress.in_flight_work === 0 &&
    (!member.kickoff || (member.kickoff.phase === "started" && !member.kickoff.error));
}
function memberObservationPending(method, response) {
  // The HTTP projection preserves this exact read-only admission refusal
  // (upstream LifecycleOperationAdmissionPending, authority_retained=false).
  // It is not a queued mutation, so a later observation may safely retry.
  const message = "member_status failed: mob lifecycle operation admission is still pending at observation_lane_saturated: member_status_observation";
  return method === "meta/member_status" && !response.result && response.error?.code === -32000 &&
    response.error.message === message && response.error.data?.error === "internal_error" &&
    response.error.data.detail === message;
}
function verifiedWordReply(frame, words, code) {
  const answer = finalText(frame);
  return contains(answer, [...words].reverse().join(" ")) && contains(answer, code);
}
function successfulSends(frames, expected) {
  const calls = frames.filter(frame => ["tool_call_requested", "tool_execution_started"].includes(frame.kind) &&
    ["send_message", "send_request", "send_response"].includes(frame.payload?.name) &&
    typeof (frame.payload.tool_call_id ?? frame.payload.id) === "string" &&
    (frame.payload.tool_call_id ?? frame.payload.id).length > 0 &&
    contains(JSON.stringify(frame.payload.args ?? frame.payload.arguments), expected));
  const successful = calls.filter(call => frames.some(result => {
    if (!toolResultWithBody(result) || result.payload.is_error !== false ||
      (result.payload.tool_call_id ?? result.payload.id) !== (call.payload.tool_call_id ?? call.payload.id)) return false;
    const sent = JSON.parse(text(result.payload));
    return sent.status === "sent" && typeof sent.receipt?.envelope_id === "string" &&
      (sent.receipt.delivery === "queued" || sent.receipt.delivery?.durably_resolved?.outcome === "accepted");
  }));
  return [...new Map(successful.map(call => [call.payload.tool_call_id ?? call.payload.id, call])).values()];
}
function successfulSend(frames, expected) {
  return successfulSends(frames, expected)[0];
}
function backgroundResponseIds(frames, expected) {
  return new Set(frames.filter(frame => contains(finalText(frame), expected)).flatMap(frame =>
    frame.payload.message.blocks.filter(providerFinalTextBlock)
      .map(block => block.data.meta.response_id)));
}
function occurrences(value, expected) {
  const needle = normalize(expected);
  assert.ok(needle);
  return normalize(value).split(needle).length - 1;
}
function pendingGateTool(frames, command, gate) {
  if (gate.phase !== "waiting" || !gate.connected || !Number.isFinite(gate.startedAt)) return null;
  return frames.find(frame => frame.kind === "tool_call_requested" &&
    frame.payload?.name === "shell" &&
    (frame.payload.args ?? frame.payload.arguments)?.command === command &&
    !(frame.payload.args ?? frame.payload.arguments)?.background &&
    frames.some(start => start.kind === "tool_execution_started" && start.payload?.name === "shell" &&
      (start.payload.tool_call_id ?? start.payload.id) === (frame.payload.tool_call_id ?? frame.payload.id)) &&
    !frames.some(result => result.kind === "tool_execution_completed" &&
      (result.payload?.tool_call_id ?? result.payload?.id) === (frame.payload.tool_call_id ?? frame.payload.id))) ?? null;
}
function decodeAgentSse(identity, onEvent) {
  let buffered = "";
  return chunk => {
    buffered += chunk;
    for (;;) {
      const boundary = /\r?\n\r?\n/.exec(buffered);
      if (!boundary) return;
      const frame = buffered.slice(0, boundary.index);
      buffered = buffered.slice(boundary.index + boundary[0].length);
      const data = [];
      let id;
      let event;
      for (const line of frame.split(/\r?\n/)) {
        if (line.startsWith(":")) continue;
        const separator = line.indexOf(":");
        const field = separator === -1 ? line : line.slice(0, separator);
        const value = separator === -1 ? "" : line.slice(separator + 1).replace(/^ /, "");
        if (field === "data") data.push(value);
        else if (field === "id") id = value;
        else if (field === "event") event = value;
      }
      if (!data.length) continue;
      const payload = JSON.parse(data.join("\n"));
      assert.equal(typeof payload?.type, "string", "Native agent SSE payload requires its event discriminator");
      assert.equal(event, payload.type, "SSE name must agree with the native event discriminator");
      assert.ok(typeof id === "string" && id.startsWith(`${identity}:`), "Native agent SSE id must match the subscribed member");
      onEvent({ id, kind: event, payload, source: "native_agent_sse", receivedAt: Date.now() });
    }
  };
}

async function readSseErrorBody(response, limit = 8192) {
  if (!response.body) return "";
  const reader = response.body.getReader();
  const chunks = [];
  let bytes = 0;
  try {
    while (bytes < limit) {
      const { value, done } = await reader.read();
      if (done) break;
      const chunk = value.subarray(0, limit - bytes);
      chunks.push(Buffer.from(chunk));
      bytes += chunk.byteLength;
    }
    return Buffer.concat(chunks).toString("utf8");
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
}
async function subscribeNativeAgent(url, identity, diagnostics) {
  const abort = new AbortController();
  const timeout = setTimeout(() => abort.abort(), 20_000);
  let response;
  const requestPath = `/agents/${encodeURIComponent(identity)}/events`;
  Object.assign(diagnostics, { identity, requestPath, startedAt: Date.now() });
  try {
    response = await fetch(`${url}${requestPath}`, { signal: abort.signal });
    diagnostics.status = response.status;
    if (response.status !== 200) {
      diagnostics.body = redact(await readSseErrorBody(response));
      throw new Error(`Native agent SSE ${requestPath} HTTP ${response.status}: ${diagnostics.body}`);
    }
    assert.ok(response.headers.get("content-type")?.startsWith("text/event-stream"));
  } catch (error) {
    abort.abort();
    throw error;
  } finally {
    clearTimeout(timeout);
  }
  const events = [];
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  const consume = decodeAgentSse(identity, event => events.push(event));
  let stopping = false;
  let failure;
  const task = (async () => {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) throw new Error(`Native agent event stream ${requestPath} ended unexpectedly`);
      consume(decoder.decode(value, { stream: true }));
    }
  })().catch(error => {
    if (!stopping) {
      failure = error;
      diagnostics.streamError = redact(error.message);
    }
  }).finally(() => reader.releaseLock());
  return {
    events,
    check() { if (failure) throw failure; },
    async stop() {
      stopping = true;
      abort.abort();
      await task;
    },
  };
}
function receipts(value) {
  if (!value || typeof value !== "object") return [];
  return Object.entries(value).flatMap(([key, item]) =>
    key.endsWith("_receipt") && typeof item === "string" ? [item] : receipts(item));
}
function assertReopenedScope(previous, current, requests, events, previousDelegations) {
  assert.notEqual(current.requestId, previous.requestId);
  assert.notEqual(current.channelId, previous.channelId);
  assert.ok(current.receipts.length && previous.receipts.length, "Both calls must carry real opaque receipts");
  assert.ok(current.receipts.every(receipt => !previous.receipts.includes(receipt)), "Reopen must receive fresh receipts");
  assert.ok(!requests.some(row => row.params.request_id === previous.requestId ||
    row.params.channel_id === previous.channelId ||
    receipts(row.params).some(receipt => previous.receipts.includes(receipt))),
  "Reopen must not reuse old request, channel, or playback receipts");
  assert.ok(!events.some(event => previousDelegations.includes(event.delegation_id) ||
    previousDelegations.includes(event.delegation?.id)), "New native events must not reference old-call delegation IDs");
  assert.ok(!events.some(event => event.type === "error"), "Late delivery must not cause a current-provider error");
}
function assertRetiredChannelGuard(response) {
  assert.equal(response.result, undefined, "A retired console channel must not remain dispatchable");
  assert.equal(response.error?.code, -32000);
  assert.equal(response.error?.data?.kind, "voice_request_conflict",
    "Only the exact retired-request custody guard is expected, not arbitrary status errors");
  assert.equal(response.error.message, "Voice request conflicts with its existing owner");
}

// A transcript is not audio; RTP counters are not decoded audio; an old peak
// does not prove a new answer. Require several current, non-silent analyser
// samples beside the matching native transcript, on the current peer.
function spokenEvidence(state, mark, expected) {
  const needle = normalize(expected);
  assert.ok(needle, "Spoken evidence requires nonempty expected text");
  const events = state.events.filter(event => event.peer === mark.peer && event.at >= mark.at &&
    event.type === "session.output_transcript.delta");
  let transcript = "";
  const segments = [];
  for (const [eventIndex, event] of events.entries()) {
    const previousLength = transcript.length;
    transcript += normalize(event.delta ?? "");
    segments.push({ start: previousLength, end: transcript.length, event });
    let offset = transcript.indexOf(needle, Math.max(0, previousLength - needle.length + 1));
    while (offset !== -1) {
      const end = offset + needle.length;
      const matching = segments.filter(segment => segment.end > offset && segment.start < end);
      // Only a newly completed occurrence can introduce an audio window.
      // An unrelated later delta must never revive an earlier silent match.
      if (end > previousLength && matching.length) {
        const first = matching[0].event;
        const last = matching.at(-1).event;
        const windowEnd = Math.min(last.at + 1500, events[eventIndex + 1]?.at ?? Infinity);
        const contiguous = matching.every((segment, index) =>
          index === 0 || segment.event.at - matching[index - 1].event.at <= 2000);
        const samples = state.samples.filter(sample => sample.peer === mark.peer && sample.kind === "speaker" &&
          sample.at >= Math.max(mark.at, first.at) && sample.at < windowEnd &&
          Number.isFinite(sample.rms) && sample.rms > 0.005);
        // Collect this bounded evidence window; this is NOT a response-end or
        // playback acknowledgement. A later unrelated transcript ends it early.
        if (Number.isFinite(state.observedAt) && state.observedAt >= windowEnd && contiguous && samples.length >= 4) return {
          transcript: matching.map(segment => segment.event.delta).join(""),
          transcriptStartAt: first.at, transcriptEndAt: last.at, audioWindowEnd: windowEnd, audibleSamples: samples.length,
        };
      }
      offset = transcript.indexOf(needle, offset + 1);
    }
  }
  return null;
}
function wavInfo(bytes) {
  assert.equal(bytes.toString("ascii", 0, 4), "RIFF");
  assert.equal(bytes.toString("ascii", 8, 12), "WAVE");
  let format;
  let data;
  for (let offset = 12; offset + 8 <= bytes.length;) {
    const name = bytes.toString("ascii", offset, offset + 4);
    const size = bytes.readUInt32LE(offset + 4);
    const chunk = bytes.subarray(offset + 8, offset + 8 + size);
    if (name === "fmt ") format = chunk;
    if (name === "data") data = chunk;
    offset += 8 + size + (size % 2);
  }
  assert.ok(format && data, "WAV requires fmt and data chunks");
  assert.equal(format.readUInt16LE(0), 1, "Portable PCM WAV required");
  assert.equal(format.readUInt16LE(2), 1);
  assert.equal(format.readUInt16LE(14), 16);
  let sum = 0;
  for (let i = 0; i + 1 < data.length; i += 2) sum += (data.readInt16LE(i) / 32768) ** 2;
  const rms = Math.sqrt(sum / (data.length / 2));
  assert.ok(rms > 0.01, "Speech fixture must be non-silent PCM");
  return { duration: data.length / 2 / format.readUInt32LE(4), rms };
}
function selfTest() {
  const mark = { peer: 1, at: 1000 };
  const event = { peer: 1, at: 1200, type: "session.output_transcript.delta", delta: "amber maple" };
  const samples = [1200, 1240, 1280, 1320].map(at => ({ peer: 1, at, kind: "speaker", rms: 0.02 }));
  const good = { observedAt: 12_000, events: [event], samples };
  const typedContent = "CURRENT_VALUE amber maple jade birch";
  const canonicalUser = { role: "user", content: [{ type: "text", text: typedContent }],
    identity: { interaction_id: "exact-console-interaction" } };
  const canonical = { sessionId: "original-session", messages: [canonicalUser] };
  const checkCanonical = snapshot => assertCanonicalTypedUser(snapshot, "original-session", "exact-console-interaction", typedContent);
  assert.equal(checkCanonical(canonical), canonicalUser);
  assert.doesNotThrow(() => checkCanonical({ ...canonical, messages: [
    { ...canonicalUser, content: typedContent, transcript_role: "conversational" },
  ] }));
  const externalNotice = { role: "system_notice", kind: "external_event", body: typedContent,
    blocks: [{ type: "external_event", source: "rpc", body: typedContent }] };
  assert.throws(() => checkCanonical({ ...canonical, messages: [externalNotice] }), /SystemNotice/);
  assert.throws(() => checkCanonical({ ...canonical, messages: [
    { ...canonicalUser, transcript_role: "injected_context" },
  ] }), /conversational/);
  assert.throws(() => checkCanonical({ ...canonical, messages: [{ ...canonicalUser, identity: undefined }] }), /EXACT/);
  assert.throws(() => checkCanonical({ ...canonical, messages: [
    { ...canonicalUser, identity: { interaction_id: "different-interaction" } },
  ] }), /EXACT/);
  assert.throws(() => checkCanonical({ ...canonical, messages: [canonicalUser, canonicalUser] }), /exactly once/);
  assert.throws(() => checkCanonical({ ...canonical, messages: [canonicalUser, externalNotice] }), /exactly once/);
  assert.throws(() => checkCanonical({ ...canonical, sessionId: "replacement-session" }), /ORIGINAL/);
  assert.throws(() => checkCanonical({ ...canonical, messages: [{ ...canonicalUser, content: "only an echo" }] }), /complete typed request/);
  const typedScope = { runtime_key: "default", identity: PRIMARY, session_id: canonical.sessionId };
  const admittedInput = { ...typedScope, id: "send-input", kind: "user_input", source: { kind: "send" },
    interaction_id: canonicalUser.identity.interaction_id, payload: { content: typedContent }, status: "delivered" };
  const committedInput = { ...admittedInput, id: "committed-input", source: { kind: "session_history" },
    run_id: "canonical-run", payload: { content: canonicalUser.content, message: canonicalUser }, status: "completed" };
  const checkAdmission = frames => assertTypedSendAdmission(frames,
    { ...typedScope, interaction_id: admittedInput.interaction_id }, typedContent);
  assert.equal(checkAdmission([admittedInput]), admittedInput);
  // Admission and committed history intentionally retain different frame IDs.
  // They prove one send and one canonical input, not two authored requests.
  assert.equal(checkAdmission([admittedInput, committedInput]), admittedInput);
  assert.equal(checkAdmission([committedInput, admittedInput]), admittedInput);
  assert.throws(() => checkAdmission([committedInput]), /exactly one console send admission/);
  assert.throws(() => checkAdmission([]), /exactly one console send admission/);
  assert.throws(() => checkAdmission([admittedInput, { ...admittedInput, id: "duplicate-send" }]), /exactly one console send admission/);
  for (const key of ["runtime_key", "identity", "session_id"]) {
    assert.throws(() => checkAdmission([{ ...admittedInput, [key]: `wrong-${key}` }]), /preserve exact/);
  }
  for (const key of ["runtime_key", "identity", "session_id", "interaction_id"]) {
    assert.throws(() => assertTypedSendAdmission([admittedInput],
      { ...typedScope, interaction_id: admittedInput.interaction_id, [key]: undefined }, typedContent), /explicit|exact interaction/);
  }
  assert.throws(() => checkAdmission([{ ...admittedInput, interaction_id: "another-interaction" }]), /exactly one console send admission/);
  assert.throws(() => checkAdmission([{ ...admittedInput, source: { kind: "console_event" } }]), /exactly one console send admission/);
  assert.throws(() => checkAdmission([{ ...admittedInput, payload: { content: `${typedContent} ` } }]), /complete typed request/);
  assert.throws(() => checkAdmission([{ ...admittedInput, payload: { content: canonicalUser.content } }]), /complete typed request/,
    "The send admission must preserve the exact submitted string, not a different content shape");
  assert.equal(checkAdmission([admittedInput, { ...admittedInput, id: "other-input", interaction_id: "another-interaction" }]), admittedInput,
    "A distinct interaction with identical content is not this request's duplicate");
  assert.ok(spokenEvidence(good, mark, "amber maple"));
  assert.equal(spokenEvidence({ ...good, samples: [] }, mark, "amber maple"), null);
  assert.equal(spokenEvidence({ ...good, samples: samples.map(s => ({ ...s, rms: 0 })) }, mark, "amber maple"), null);
  assert.equal(spokenEvidence({ ...good, samples: samples.map(s => ({ ...s, rms: undefined })) }, mark, "amber maple"), null);
  assert.equal(spokenEvidence(good, { ...mark, at: 1300 }, "amber maple"), null);
  assert.equal(spokenEvidence(good, { ...mark, peer: 2 }, "amber maple"), null);
  assert.equal(spokenEvidence(good, mark, "stale first answer"), null);
  assert.equal(spokenEvidence({ ...good, samples: samples.map(s => ({ ...s, kind: "microphone" })) }, mark, "amber maple"), null);
  assert.equal(spokenEvidence({ ...good, events: [{ ...event, at: 10_000 }], samples }, mark, "amber maple"), null);
  const laterAudio = samples.map(sample => ({ ...sample, at: sample.at + 8800 }));
  const unrelated = { ...event, at: 10_000, delta: "Hello, how can I help?" };
  assert.equal(spokenEvidence({ ...good, events: [event, unrelated], samples: laterAudio }, mark, "amber maple"), null,
    "A later audible greeting must not revive an earlier silent expected phrase");
  assert.equal(spokenEvidence({ ...good, events: [
    { ...event, delta: "amber" }, { ...event, at: 1400, delta: " maple" }, unrelated,
  ], samples: laterAudio }, mark, "amber maple"), null);
  const repeated = spokenEvidence({ ...good, events: [event, unrelated, { ...event, at: 10_200 }],
    samples: samples.map(sample => ({ ...sample, at: sample.at + 9000 })) }, mark, "amber maple");
  assert.equal(repeated.transcriptStartAt, 10_200, "A genuinely repeated audible phrase gets its own window");
  assert.equal(spokenEvidence({ ...good, events: [event, unrelated, { ...event, at: 10_200 }],
    samples: laterAudio }, mark, "amber maple"), null, "A silent repeat cannot borrow earlier greeting audio");
  assert.ok(spokenEvidence({ ...good, events: [{ ...event, delta: "amber" }, { ...event, at: 1400, delta: " maple" }],
    samples }, mark, "amber maple"), "A phrase split over adjacent native deltas must still match");
  assert.equal(spokenEvidence({ ...good, events: [{ ...event, delta: "amber" }, { ...event, at: 10_000, delta: " maple" }],
    samples: laterAudio }, mark, "amber maple"), null, "Distant fragments cannot manufacture a continuous spoken phrase");
  assert.equal(spokenEvidence({ ...good, events: [event, { ...unrelated, at: 1600 }],
    samples: samples.map(sample => ({ ...sample, at: sample.at + 500 })) }, mark, "amber maple"), null,
  "Even an unrelated greeting inside the old tolerance window cannot supply the matching phrase's audio");
  assert.equal(spokenEvidence({ ...good, observedAt: undefined }, mark, "amber maple"), null);
  assert.equal(finalText({ kind: "user_input", payload: { content: "amber" } }), "");
  assert.equal(finalText({ kind: "interaction_complete", payload: { result: "amber", is_error: true } }), "");
  assert.equal(finalText({ kind: "interaction_complete", status: "completed", payload: { result: "amber" } }), "",
    "Mirrored Live transcript must not masquerade as a real background result");
  // Exact canonical frame retained from the 2026-09-28 acceptance bootstrap.
  // It proves a provider final message in history, not committed run terminality.
  const recordedFinal = JSON.parse(fs.readFileSync(path.join(ASSETS, "committed-assistant-final.json")));
  const recordedText = "KEEPER READY 0158d5c0-fa97-4cb9-85b2-f6896c049adc";
  assert.equal(finalText(recordedFinal), recordedText,
    "The recorded committed provider final must be accepted as text_complete");
  const scope = Object.fromEntries(["identity", "session_id", "run_id", "interaction_id"]
    .map(key => [key, recordedFinal[key]]));
  assert.equal(finalText(recordedFinal, scope), recordedText);
  for (const key of Object.keys(scope)) {
    assert.equal(finalText(recordedFinal, { ...scope, [key]: "wrong-scope" }), "", `Reject wrong expected ${key}`);
    assert.equal(finalText(recordedFinal, { ...scope, [key]: undefined }), "", `Reject missing expected ${key}`);
  }
  const rejectFinal = (label, change) => {
    const frame = structuredClone(recordedFinal);
    change(frame);
    assert.equal(finalText(frame), "", label);
  };
  rejectFinal("Stream-only text is not committed history", frame => { frame.source.kind = "console_event"; });
  rejectFinal("Absent provenance is not committed history", frame => { delete frame.source; });
  rejectFinal("A legacy interaction_complete is not the canonical assistant history frame", frame => { frame.kind = "interaction_complete"; });
  rejectFinal("A running frame is not a completed assistant message", frame => { frame.status = "running"; });
  rejectFinal("An errored final must not count", frame => { frame.payload.is_error = true; });
  rejectFinal("A user message is not an assistant reply", frame => { frame.payload.message.role = "user"; });
  rejectFinal("Tool-use text does not finish the provider response", frame => { frame.payload.message.stop_reason = "tool_use"; });
  rejectFinal("Truncated text is not a final reply", frame => { frame.payload.message.stop_reason = "max_tokens"; });
  rejectFinal("An intermediate provider block is not a final answer", frame => { frame.payload.message.blocks[0].data.meta.phase = "commentary"; });
  rejectFinal("Only the configured provider's actual final counts", frame => { frame.payload.message.blocks[0].data.meta.provider = "live_mirror"; });
  rejectFinal("Provider final metadata is mandatory", frame => { delete frame.payload.message.blocks[0].data.meta; });
  rejectFinal("A nonempty provider response ID is mandatory", frame => { frame.payload.message.blocks[0].data.meta.response_id = ""; });
  rejectFinal("Flattened result cannot substitute for canonical authored text", frame => { delete frame.payload.message.blocks[0].data.text; });
  rejectFinal("Tool-only history cannot count as a final text reply", frame => { frame.payload.message.blocks[0].block_type = "tool_use"; });
  rejectFinal("History must name its assistant message", frame => { delete frame.payload.message.assistant_message_id; });
  rejectFinal("Projection must name the same assistant message", frame => { frame.payload.assistant_message_id = "wrong-message"; });
  for (const key of ["identity", "session_id", "run_id"]) {
    rejectFinal(`History must name its ${key}`, frame => { delete frame[key]; });
  }
  for (const key of ["run_id", "interaction_id"]) {
    rejectFinal(`Canonical message must retain the same ${key}`, frame => { frame.payload.message.identity[key] = "wrong-scope"; });
  }
  rejectFinal("An interaction cannot disappear from its projection", frame => { delete frame.interaction_id; });
  const uncorrelatedFinal = structuredClone(recordedFinal);
  delete uncorrelatedFinal.interaction_id;
  delete uncorrelatedFinal.payload.message.identity.interaction_id;
  assert.equal(finalText(uncorrelatedFinal), recordedText, "System-origin replies need not have a console interaction");
  assert.equal(finalText(uncorrelatedFinal, scope), "", "Uncorrelated replies cannot satisfy an exact accepted input");
  const misleadingMirror = structuredClone(recordedFinal);
  misleadingMirror.payload.result = misleadingMirror.payload.text = "unverified mirror text";
  assert.equal(finalText(misleadingMirror), recordedText, "Read the provider final block, never the flattened mirror");
  const mixedBlocks = structuredClone(recordedFinal);
  mixedBlocks.payload.message.blocks.unshift({ block_type: "text", data: {
    text: "intermediate text", meta: { provider: "open_ai_assistant_message", phase: "commentary", response_id: "intermediate" },
  } });
  assert.equal(finalText(mixedBlocks), recordedText, "Intermediate blocks cannot borrow final-answer provenance");
  const reply = value => {
    const frame = structuredClone(recordedFinal);
    frame.payload.result = frame.payload.text = frame.payload.message.blocks[0].data.text = value;
    return frame;
  };
  const backgroundFinal = reply("amber");
  assert.equal(finalText(backgroundFinal), "amber");
  const wordReply = reply("maple amber");
  assert.equal(verifiedWordReply(wordReply, ["amber", "maple"], "jade birch"), false,
    "Trivial reversed words without keeper-only knowledge cannot satisfy phase one");
  const completeWordReply = reply("maple amber; code jade birch");
  assert.equal(verifiedWordReply(completeWordReply, ["amber", "maple"], "jade birch"), true);
  assert.equal(verifiedWordReply({ ...completeWordReply, payload: { ...completeWordReply.payload, message: undefined } },
    ["amber", "maple"], "jade birch"), false, "A Live-only answer cannot masquerade as a verified background reply");
  assert.equal(finalText({ ...backgroundFinal, payload: { ...backgroundFinal.payload, is_error: true } }), "",
    "Even a provider-backed final must not count when explicitly marked as an error");
  assert.equal(backgroundResponseIds([backgroundFinal, backgroundFinal], "amber").size, 1,
    "Live/history projections of one real model response are not duplicate executions");
  const duplicateFinal = JSON.parse(JSON.stringify(backgroundFinal));
  duplicateFinal.payload.message.blocks[0].data.meta.response_id = "second-real-response";
  assert.equal(backgroundResponseIds([backgroundFinal, duplicateFinal], "amber").size, 2);
  assert.equal(occurrences("amber maple. Amber, maple.", "amber maple"), 2);
  const gateCall = { kind: "tool_call_requested", payload: { id: "gate-call", name: "shell",
    args: { command: "curl test-gate", background: false } } };
  const gateStarted = { kind: "tool_execution_started", payload: { id: "gate-call", name: "shell" } };
  const waitingGate = { phase: "waiting", connected: true, startedAt: 123 };
  assert.ok(pendingGateTool([gateCall, gateStarted], "curl test-gate", waitingGate));
  assert.equal(pendingGateTool([gateCall], "curl test-gate", waitingGate), null, "Requested is not native execution-started");
  assert.equal(pendingGateTool([gateCall, gateStarted], "curl test-gate", { ...waitingGate, connected: false }), null);
  assert.equal(pendingGateTool([gateCall, gateStarted], "curl test-gate", { ...waitingGate, phase: "released" }), null);
  assert.equal(pendingGateTool([gateCall, gateStarted, { kind: "tool_execution_completed", payload: { id: "gate-call" } }],
    "curl test-gate", waitingGate), null, "Already-completed shell work must not satisfy close-under-load");
  assert.equal(pendingGateTool([], "curl test-gate", waitingGate), null, "A service request without a native tool start is insufficient");
  const parsed = [];
  const consume = decodeAgentSse("keeper", event => parsed.push(event));
  const nativeFrames = ': keepalive\r\n\r\nid: keeper:0\r\nevent: tool_call_requested\r\ndata: {"type":"tool_call_requested",\r\ndata: "id":"gate-call","name":"shell","args":{"command":"curl test-gate","background":false}}\r\n\r\n' +
    'id: keeper:1\nevent: tool_execution_started\ndata: {"type":"tool_execution_started","id":"gate-call","name":"shell"}\n\n';
  for (const char of nativeFrames) consume(char);
  assert.equal(parsed.length, 2);
  assert.ok(pendingGateTool(parsed, "curl test-gate", waitingGate));
  assert.equal(pendingGateTool([{ ...parsed[0], payload: { ...parsed[0].payload,
    args: { command: "curl test-gate", background: true } } }, parsed[1]], "curl test-gate", waitingGate), null,
  "Detached shell operation requires its own operation-lifecycle evidence, not the foreground gate contract");
  assert.throws(() => decodeAgentSse("keeper", () => {})(
    'id: keeper:2\nevent: tool_execution_started\ndata: {"type":"tool_execution_completed"}\n\n'));
  assert.throws(() => decodeAgentSse("keeper", () => {})(
    'id: wrong-member:2\nevent: tool_execution_started\ndata: {"type":"tool_execution_started"}\n\n'));
  assert.throws(() => decodeAgentSse("keeper", () => {})('id: keeper:2\nevent: tool_execution_started\ndata: not-json\n\n'));
  const publicIdentityFrames = [];
  decodeAgentSse(KEEPER, event => publicIdentityFrames.push(event))(
    `id: ${KEEPER}:0\nevent: tool_execution_started\ndata: {"type":"tool_execution_started","id":"real-shell-call","name":"shell"}\n\n`);
  assert.equal(publicIdentityFrames[0].id, "voice-keeper:0");
  assert.throws(() => decodeAgentSse(KEEPER, () => {})(
    `id: rt:${KEEPER}:0:0\nevent: tool_execution_started\ndata: {"type":"tool_execution_started","id":"real-shell-call","name":"shell"}\n\n`),
  "The SSE prefix must match the public identity actually subscribed, not runtime bookkeeping");
  assert.equal(hasPendingResults([backgroundFinal], ["amber"]), false, "A result already in canonical history cannot be reported as absent");
  assert.equal(hasPendingResults([backgroundFinal], ["amber", "jade"]), true);
  const readyIdentity = { identity: "primary", state: "active", response_phase: null, session_id: "canonical" };
  const idleMember = { current_session_id: "canonical", error: null,
    progress: { run_state: "idle", in_flight_work: 0 }, kickoff: { phase: "started" } };
  assert.ok(bootstrapMemberIdle("primary", readyIdentity, idleMember));
  assert.equal(bootstrapMemberIdle("primary", readyIdentity, { ...idleMember, progress: undefined }), false);
  assert.equal(bootstrapMemberIdle("primary", readyIdentity, { ...idleMember,
    progress: { run_state: "idle", in_flight_work: 1 } }), false);
  assert.ok(bootstrapMemberIdle("primary", { ...readyIdentity, response_phase: "waiting" }, idleMember));
  assert.equal(bootstrapMemberIdle("primary", readyIdentity, { ...idleMember,
    progress: { run_state: "run_open", in_flight_work: 0 } }), false);
  assert.equal(bootstrapMemberIdle("primary", readyIdentity, { ...idleMember, current_session_id: "different" }), false);
  assert.equal(bootstrapMemberIdle("primary", readyIdentity, { ...idleMember, kickoff: { phase: "starting" } }), false);
  const pendingMessage = "member_status failed: mob lifecycle operation admission is still pending at observation_lane_saturated: member_status_observation";
  const pendingObservation = { error: { code: -32000, message: pendingMessage,
    data: { error: "internal_error", detail: pendingMessage } } };
  assert.ok(memberObservationPending("meta/member_status", pendingObservation));
  assert.equal(memberObservationPending("meta/console/send", pendingObservation), false);
  assert.equal(memberObservationPending("meta/member_status", { error: { ...pendingObservation.error, code: -32600 } }), false);
  assert.equal(memberObservationPending("meta/member_status", { error: { ...pendingObservation.error, message: "Unauthorized" } }), false);
  assert.equal(memberObservationPending("meta/member_status", { error: { ...pendingObservation.error, data: undefined } }), false);
  const call = { kind: "tool_call_requested", payload: { name: "send_message", id: "a", args: { content: "launch" } } };
  const result = { kind: "tool_result_received", payload: { id: "a", is_error: false,
    result: JSON.stringify({ status: "sent", receipt: { envelope_id: "native-envelope", delivery: "queued" } }) } };
  assert.ok(successfulSend([call, result], "launch"));
  const executionFact = { kind: "tool_execution_completed", payload: { id: "a", is_error: false, duration_ms: 3 } };
  assert.equal(successfulSend([call, executionFact], "launch"), undefined, "The execution fact carries no receipt");
  assert.ok(successfulSend([call, executionFact, result], "launch"));
  assert.ok(successfulSend([call, { ...result, kind: "tool_execution_completed" }], "launch"),
    "A session-history result row keeps the body under the execution fact's kind");
  assert.equal(successfulSends([call, call, result], "launch").length, 1);
  assert.equal(successfulSends([call, result, { ...call, payload: { ...call.payload, id: "b" } },
    { ...result, payload: { ...result.payload, id: "b" } }], "launch").length, 2);
  assert.equal(successfulSend([call, { ...result, payload: { ...result.payload, is_error: true } }], "launch"), undefined);
  assert.equal(successfulSend([call, { ...result, payload: { id: "a", result: "sent" } }], "launch"), undefined);
  assert.equal(successfulSend([call, { ...result, payload: { ...result.payload,
    result: JSON.stringify({ status: "failed", receipt: { envelope_id: "native-envelope", delivery: "queued" } }) } }], "launch"), undefined);
  assert.deepEqual(receipts({ handle: { activation_receipt: "opaque" }, phase: "active" }), ["opaque"]);
  const oldScope = { requestId: "old-request", channelId: "old-channel", receipts: ["old-receipt"] };
  const newScope = { requestId: "new-request", channelId: "new-channel", receipts: ["new-receipt"] };
  const legitimateLateFact = [{ type: "session.output_transcript.delta", delta: "A fact from the old operation is now verified." }];
  assert.doesNotThrow(() => assertReopenedScope(oldScope, newScope, [], legitimateLateFact, ["old-delegation"]),
    "Old-origin facts may legitimately arrive through the current canonical context");
  assert.throws(() => assertReopenedScope(oldScope, newScope,
    [{ params: { activation_receipt: "old-receipt" } }], legitimateLateFact, []));
  assert.throws(() => assertReopenedScope(oldScope, newScope,
    [{ params: { request_id: "old-request" } }], [], []));
  assert.throws(() => assertReopenedScope(oldScope, newScope,
    [{ params: { channel_id: "old-channel" } }], [], []));
  assert.throws(() => assertReopenedScope(oldScope, newScope, [],
    [{ type: "session.commentary.appended", delegation_id: "old-delegation" }], ["old-delegation"]));
  assert.throws(() => assertReopenedScope(oldScope, newScope, [], [{ type: "error" }], []));
  const retiredGuard = { error: { code: -32000, data: { kind: "voice_request_conflict" },
    message: "Voice request conflicts with its existing owner" } };
  assert.doesNotThrow(() => assertRetiredChannelGuard(retiredGuard));
  assert.throws(() => assertRetiredChannelGuard({ result: { phase: "closed" } }));
  assert.throws(() => assertRetiredChannelGuard({ error: { ...retiredGuard.error, code: -32600 } }));
  assert.throws(() => assertRetiredChannelGuard({ error: { ...retiredGuard.error, data: { kind: "voice_host_failed" } } }));
  assert.throws(() => assertRetiredChannelGuard({ error: { ...retiredGuard.error, message: "Unexpected error" } }));
  for (const name of [...WORDS, ...Object.keys(PHRASES)]) wavInfo(fs.readFileSync(path.join(ASSETS, `${name}.wav`)));
  const manifest = JSON.parse(fs.readFileSync(path.join(ASSETS, "manifest.json")));
  assert.deepEqual(manifest.phrases, PHRASES);
  assert.deepEqual(manifest.word_fixtures, WORDS);
  log("offline-pass", { fixtures: WORDS.length + Object.keys(PHRASES).length,
    checks: "transcript/audio windows, canonical provider final and scope, native send success, duplicate projections/executions, receipt isolation" });
}

async function startPeerGate(facts) {
  const route = `/wait/${crypto.randomUUID()}`;
  const state = { phase: "idle", connected: false, startedAt: null, releasedAt: null, operationId: facts.voiceOperation };
  let held;
  let timeout;
  const server = http.createServer((request, response) => {
    if (request.method !== "GET" || request.url !== route ||
      request.headers.host !== `127.0.0.1:${server.address().port}`) {
      response.writeHead(404).end("Unknown test operation");
      return;
    }
    if (state.phase !== "idle") {
      response.writeHead(409).end("Operation already started");
      return;
    }
    held = response;
    Object.assign(state, { phase: "waiting", connected: true, startedAt: Date.now() });
    response.on("close", () => {
      state.connected = false;
      if (state.phase === "waiting") state.phase = "disconnected";
      clearTimeout(timeout);
    });
    timeout = setTimeout(() => {
      state.phase = "expired";
      response.writeHead(504).end("External verification deadline exceeded");
    }, 240_000);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const url = `http://127.0.0.1:${server.address().port}${route}`;
  return {
    url,
    command: `curl --silent --show-error --fail --max-time 240 '${url}'`,
    snapshot: () => ({ ...state }),
    release() {
      assert.equal(state.phase, "waiting", "Only an actively blocked external request can be released");
      assert.equal(state.connected, true);
      clearTimeout(timeout);
      Object.assign(state, { phase: "released", releasedAt: Date.now() });
      held.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({
        operation_id: facts.voiceOperation, verified_code: facts.oldVoice,
      }));
    },
    async stop() {
      clearTimeout(timeout);
      try {
        if (state.phase === "waiting") {
          state.phase = "cancelled";
          held.writeHead(503).end("Test owner cancelled external verification");
          await poll("external request cancellation", () => !state.connected, 1000, 20);
        }
      } finally {
        server.closeAllConnections();
        await new Promise(resolve => server.close(resolve));
      }
    },
  };
}

async function selfTestPeerGate() {
  assert.equal(await readSseErrorBody(new Response('{"error":"internal_server_error"}')),
    '{"error":"internal_server_error"}');
  assert.equal(await readSseErrorBody(new Response("bounded-error-body"), 7), "bounded");
  const facts = { voiceOperation: "offline-operation", oldVoice: "offline verified value" };
  const gate = await startPeerGate(facts);
  try {
    let completed = false;
    const pending = fetch(gate.url).then(async response => {
      const result = { status: response.status, body: await response.json() };
      completed = true;
      return result;
    });
    await poll("offline external request start", () => gate.snapshot().connected, 2000, 20);
    await sleep(100);
    assert.equal(completed, false, "External gate must truly hold the response");
    assert.equal((await fetch(gate.url)).status, 409, "Duplicate work must not acquire another gate");
    gate.release();
    assert.deepEqual(await pending, { status: 200,
      body: { operation_id: facts.voiceOperation, verified_code: facts.oldVoice } });
  } finally {
    await gate.stop();
  }
  const cancelled = await startPeerGate(facts);
  const pending = fetch(cancelled.url);
  try {
    await poll("offline cancellable request start", () => cancelled.snapshot().connected, 2000, 20);
    await cancelled.stop();
    const response = await pending;
    assert.equal(response.status, 503);
    await response.text();
    assert.equal(cancelled.snapshot().phase, "cancelled");
  } finally {
    if (cancelled.snapshot().phase !== "cancelled") await cancelled.stop();
  }
  log("offline-peer-gate-pass", { providerCalls: 0, holdsRealHttpResponse: true, cancellationVerified: true });
}

function buildGateway() {
  const override = process.env.META_VOICE_GATEWAY_BIN;
  if (override) {
    const binary = path.resolve(override);
    fs.accessSync(binary, fs.constants.X_OK);
    return binary;
  }
  for (const name of ["console-app.js", "console-app.css", "index.html"]) {
    assert.ok(fs.readFileSync(path.join(__dirname, "dist", name))
      .equals(fs.readFileSync(path.join(ROOT, "crates", "meerkat-meta", "console-dist", name))),
    "Prepare the current embedded console first: cd console && npm run build");
  }
  log("build", { binary: "meta_gateway", features: "openai-live", incremental: false });
  const run = spawnSync(path.join(ROOT, "scripts", "repo-cargo"),
    ["build", "--locked", "-p", "meerkat-meta", "--bin", "meta_gateway", "--features", "openai-live", "--message-format=json"],
    { cwd: ROOT, env: { ...childEnv(), CARGO_INCREMENTAL: "0" }, encoding: "utf8", timeout: 1_200_000, maxBuffer: 32 * 1024 * 1024 });
  assert.equal(run.status, 0, `Gateway build failed: ${redact(run.stderr)}`);
  const artifact = run.stdout.split("\n").filter(Boolean).map(line => JSON.parse(line))
    .find(row => row.reason === "compiler-artifact" && row.target?.name === "meta_gateway" && row.executable);
  assert.ok(artifact, "Cargo did not report the built gateway");
  return artifact.executable;
}

async function launchGateway(binary, apiKey, facts, directory, options = {}) {
  const workspace = path.join(directory, "workspace");
  fs.mkdirSync(path.join(workspace, "config"), { recursive: true, mode: 0o700 });
  const principal = "voice-acceptance@localhost";
  const realm = "voice-acceptance";
  const signingKey = crypto.randomBytes(32).toString("base64url");
  secrets.push(signingKey);
  const encode = value => Buffer.from(JSON.stringify(value)).toString("base64url");
  const unsigned = `${encode({ alg: "HS256", typ: "JWT" })}.${encode({
    iss: "http://127.0.0.1/meta-gateway", aud: "persistent-gateway",
    sub: principal, email: principal, exp: Math.floor(Date.now() / 1000) + 3600,
  })}`;
  const jwt = `${unsigned}.${crypto.createHmac("sha256", signingKey).update(unsigned).digest("base64url")}`;
  secrets.push(jwt);
  fs.writeFileSync(path.join(workspace, "config", "mob.toml"), `
[mob]
id = "voice-paid-acceptance"
[profiles.primary]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"
peer_description = "Original background agent for the voice acceptance conversation."
[profiles.primary.tools]
comms = true
[profiles.keeper]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"
peer_description = "Keeper verifies word requests immediately and holds launch/overlap replies until operator release."
[profiles.keeper.tools]
comms = true
shell = true
[wiring]
role_wiring = [{ a = "primary", b = "keeper" }]
`);
  fs.writeFileSync(path.join(workspace, "config", "console.toml"), `
title = "Paid Voice Acceptance"
[layout]
initial_preset = "single"
initial_agent = "${PRIMARY}"
[rail]
visible = false
`);
  const configPath = path.join(workspace, "host.toml");
  fs.writeFileSync(configPath, `
[shell]
program = "sh"
timeout_secs = 250
[realm.${realm}]
default_binding = "openai"
[realm.${realm}.backend.openai_api]
provider = "openai"
backend_kind = "openai_api"
[realm.${realm}.auth.openai_key]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }
[realm.${realm}.binding.openai]
backend_profile = "openai_api"
auth_profile = "openai_key"
provider_default = true
`);
  let backend;
  const proxy = http.createServer((req, res) => {
    const origin = `http://127.0.0.1:${proxy.address().port}`;
    if (req.headers.host !== new URL(origin).host || (req.headers.origin && req.headers.origin !== origin) ||
      !req.url.startsWith("/") || req.url.startsWith("//")) {
      res.writeHead(403).end("Loopback origin required");
      return;
    }
    if (!backend) { res.writeHead(503).end("Starting"); return; }
    const headers = { ...req.headers, host: backend.host, authorization: `Bearer ${jwt}` };
    delete headers.connection;
    const upstream = http.request({ hostname: backend.hostname, port: backend.port, path: req.url,
      method: req.method, headers }, response => {
      const outgoing = { ...response.headers };
      delete outgoing.connection;
      delete outgoing["transfer-encoding"];
      res.writeHead(response.statusCode, outgoing);
      response.pipe(res);
    });
    upstream.on("error", () => {
      if (!res.headersSent) res.writeHead(502);
      res.end("Gateway unavailable");
    });
    res.on("close", () => upstream.destroy());
    req.pipe(upstream);
  });
  await new Promise((resolve, reject) => { proxy.once("error", reject); proxy.listen(0, "127.0.0.1", resolve); });
  const url = `http://127.0.0.1:${proxy.address().port}`;
  const gateway = spawn(binary, [], { cwd: workspace,
    env: { ...childEnv(), OPENAI_API_KEY: apiKey, XDG_STATE_HOME: path.join(directory, "xdg"),
      RUST_LOG: options.rustLog ?? "warn,meerkat_meta=info,meerkat::experimental_gpt_live=info,meerkat::session_runtime::live_orchestration=info,meerkat_openai::public_live=info" },
    stdio: ["pipe", "pipe", "pipe"] });
  const stderr = readline.createInterface({ input: gateway.stderr });
  const logPath = path.join(directory, "gateway.log");
  fs.writeFileSync(logPath, "", { mode: 0o600 });
  stderr.on("line", line => fs.appendFileSync(logPath, `${redact(line)}\n`));
  const stdout = readline.createInterface({ input: gateway.stdout });
  let init;
  stdout.on("line", line => {
    try {
      const reply = JSON.parse(line);
      if (reply.id === "voice-e2e-init") init = reply;
    } catch { fs.appendFileSync(logPath, "Gateway emitted a non-JSON stdout line\n"); }
  });
  let launchError;
  gateway.on("error", error => { launchError = error; });
  const stop = async () => {
    proxy.closeAllConnections();
    await new Promise(resolve => proxy.close(resolve));
    if (gateway.exitCode === null && gateway.signalCode === null && gateway.pid) {
      const exited = new Promise(resolve => gateway.once("exit", resolve));
      gateway.stdin.end();
      const timer = setTimeout(() => gateway.kill("SIGKILL"), 12_000);
      await exited;
      clearTimeout(timer);
    }
    stderr.close();
    stdout.close();
  };
  try {
    gateway.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id: "voice-e2e-init", method: "meta/init", params: {
      workspace_root: workspace, runtime_root: path.join(directory, "runtime"), store_path: path.join(directory, "state"),
      persistent_sessions: true, identity_first: true, realm, meerkat_config_path: configPath,
      http_listen: "127.0.0.1:0", http_public_base_url: url,
      auth_config: { provider: "jwt", shared_secret: signingKey, email_allowlist: [principal] },
      console_voice: { principal, realm, auth_binding: { realm, binding: "openai" }, voice: "marin",
        session_instructions: "Be concise. Use client delegation to the existing background agent for EVERY substantive task, including word reversal and keeper checks. Never solve those tasks yourself. Say verified results and exact code words clearly. When a later background or peer result updates a pending task, announce the NEW result promptly, not just the initial pending answer. Announce each verified operation result once; do not spontaneously repeat its code. No unsolicited recaps or greetings on reopen. Never present commentary as user input." },
      identity_roster: [
        { identity: PRIMARY, profile: "primary", display_name: LABEL, addressability: "addressable",
          labels: { display_name: LABEL },
          additional_instructions: [
            "You are the ORIGINAL gpt-5.5 background agent, also executing client-delegated voice work. Use your existing session, never spawn a replacement or delegate worker. Ignore roster notices and task-completion receipts: do not call any comms tools for them. No unsolicited messages or reply loops. Keep final answers short.",
            "You ARE the user's background agent. For typed CURRENT_VALUE commands store and confirm the exact four words. For recall return only the most recent CURRENT_VALUE, never earlier values.",
            "When the user asks for the keeper's verification code for three words, this REQUIRES a real keeper lookup: you do not know the code. Call peers, then send_message to the keeper with VERIFY_WORDS and the exact three words. Never answer by reversing them yourself. The keeper replies immediately for this task, without operator release. Send only once; if its reply has not arrived, report Verification requested. When the keeper replies, give a final answer containing its reversed words AND its exact verification code. Do not acknowledge the keeper with another peer message.",
            "For launch, overlap voice, and overlap typed checks: call peers then send_message with the relevant task to the keeper using genuine comms. Use the ONE-WAY send_message tool, NOT send_request or send_response. Immediately give final answer WAITING FOR KEEPER (plus the task name); do not block waiting. The keeper holds the result until operator release. When a keeper result arrives later, provide a NEW final answer including the exact verified code. Do not reply to the keeper.",
            `Use operation ID ${facts.voiceOperation} for the overlap voice check and ${facts.typedOperation} for the overlap typed check. Include the corresponding operation ID in peer requests and final results. These checks are independent of CURRENT_VALUE; their late results must never overwrite the stored CURRENT_VALUE.`,
          ] },
        { identity: KEEPER, profile: "keeper", display_name: "Code Keeper", addressability: "addressable",
          labels: { display_name: "Code Keeper" },
          additional_instructions: [
            "You are a real peer of the primary background agent. Ignore roster notices and task-completion receipts: do not call any comms tools for them. Use peers and one-way send_message for genuine peer replies, NOT send_request or send_response. No reply loops.",
            `For VERIFY_WORDS requests, reverse the supplied three words and IMMEDIATELY use send_message to reply to the requester with the reversed words and your exact verification code: ${facts.wordVerification}. Then provide a local final answer with those same reversed words and code. This task is never HELD and requires no operator release. Never send this code unsolicited; only a genuine VERIFY_WORDS request authorizes it.`,
            "On a peer request for launch or overlap code: remember the sender and task; do NOT send a reply yet. Your local final answer is HELD LAUNCH or HELD OVERLAP VOICE or HELD OVERLAP TYPED, including the operation ID if given. RELEASE LAUNCH releases only launch. RELEASE OPERATION followed by an exact operation ID releases ONLY that operation, never other held work. Use genuine send_message exactly once for the released task, including its operation ID and verified code, then final-confirm what you sent. Never send codes before their specific release.",
            "BEGIN EXTERNAL VERIFICATION for a held operation overrides RELEASE for that operation: execute the provided exact curl command ONCE using shell with background=false and timeout_secs=250. This is a real foreground request to a test-owned loopback service. Wait for its successful result; never background, cancel, retry, or invent a result. Its JSON supplies operation_id and verified_code. Send that exact result once to the original requester, then final-confirm it. On a timeout or HTTP error report failure, never a verification code. Do not run any other shell command.",
            `Verified launch code: ${facts.peer}. Verified overlap typed code: ${facts.oldTyped}. The overlap voice code is available ONLY from the external verification service.`,
          ] },
      ],
    } })}\n`);
    await poll("authenticated gateway initialization", () => {
      if (launchError) throw launchError;
      assert.equal(gateway.exitCode, null, "Gateway exited during initialization");
      return init;
    }, 90_000);
    assert.ok(!init.error, `Gateway init failed: ${redact(JSON.stringify(init.error))}`);
    backend = new URL(init.result.http_base_url);
    assert.equal(backend.hostname, "127.0.0.1");
    assert.notEqual(backend.port, "54040");
    assert.notEqual(new URL(url).port, "54040");
    const startupDeadline = Date.now() + 30_000;
    const preflights = [];
    const preflight = async (label, target) => {
      const entry = { label, started: Date.now(), budgetMs: Math.max(1, startupDeadline - Date.now()) };
      preflights.push(entry);
      const persist = () => fs.writeFileSync(path.join(directory, "startup.json"), JSON.stringify(preflights, null, 2), { mode: 0o600 });
      persist();
      log("startup-preflight", { label, budgetMs: entry.budgetMs });
      try {
        const response = await fetch(target, { signal: AbortSignal.timeout(entry.budgetMs) });
        const bytes = Buffer.from(await response.arrayBuffer());
        Object.assign(entry, { elapsedMs: Date.now() - entry.started, status: response.status });
        persist();
        log("startup-preflight-complete", entry);
        return { status: response.status, bytes };
      } catch (error) {
        Object.assign(entry, { elapsedMs: Date.now() - entry.started, error: redact(error.message) });
        persist();
        throw new Error(`Startup preflight "${label}" failed after ${entry.elapsedMs}ms (shared startup budget 30000ms): ${redact(error.message)}`);
      }
    };
    const unauthorized = await preflight("unauthenticated backend experience", `${backend.origin}/console/experience`);
    assert.ok([401, 403].includes(unauthorized.status), "Gateway must reject unauthenticated access");
    const ready = await preflight("authenticated proxy experience", `${url}/console/experience`);
    assert.equal(ready.status, 200, "Authenticated loopback proxy must reach the real gateway");
    const servedBundle = await preflight("embedded console bundle", `${url}/console/assets/console-app.js`);
    assert.equal(servedBundle.status, 200);
    const currentBundle = servedBundle.bytes.equals(
      fs.readFileSync(path.join(ROOT, "crates", "meerkat-meta", "console-dist", "console-app.js")));
    if (options.allowStaleBundle) log("embedded-console-bundle", { matchesCurrentBuild: currentBundle });
    else assert.ok(currentBundle, "Gateway embeds a stale console: prepare npm run build, then rebuild the openai-live binary");
    log("gateway-ready", { pid: gateway.pid, model: "gpt-5.5", voiceModel: "gpt-live-1" });
    return { url, stop };
  } catch (error) {
    await stop();
    throw error;
  }
}

// Runs in Chromium. All WebRTC, fetch, and native data-channel behavior remains
// real. AudioContext taps observe decoded samples without bypassing UI muting.
function instrumentBrowser() {
  const state = window.voiceAcceptance = { events: [], samples: [], peers: [], inputs: [], inputClocks: [],
    contexts: [], gains: [], sends: [], transportSamples: [], samplingErrors: [], timeline: [] };
  // Activation timeline: named marks on the page's performance clock.
  const mark = (name, extra = {}) => state.timeline.push({ at: performance.now(), name, ...extra });
  state.mark = mark;
  const enabledProperty = Object.getOwnPropertyDescriptor(MediaStreamTrack.prototype, "enabled");
  Object.defineProperty(MediaStreamTrack.prototype, "enabled", {
    configurable: true,
    get() { return enabledProperty.get.call(this); },
    set(value) {
      if (this.acceptanceMicrophone && value && !enabledProperty.get.call(this)) mark("mic-enabled");
      enabledProperty.set.call(this, value);
    },
  });
  const NativeContext = window.AudioContext;
  window.AudioContext = class extends NativeContext {
    constructor(...args) { super(...args); state.contexts.push(this); }
    createGain(...args) {
      const gain = super.createGain(...args);
      state.gains.push(gain);
      return gain;
    }
  };
  const capture = new NativeContext();
  state.capture = capture;
  state.meters = [];
  function meter(stream, kind, peer) {
    const source = capture.createMediaStreamSource(stream);
    const analyser = capture.createAnalyser();
    analyser.fftSize = 2048;
    const gain = capture.createGain();
    gain.gain.value = 0;
    source.connect(analyser).connect(gain).connect(capture.destination);
    state.meters.push({ source, analyser, gain, kind, peer, data: new Float32Array(2048) });
  }
  setInterval(() => {
    const at = performance.now();
    for (const clock of state.inputClocks) {
      if (!clock.stopped && clock.output.stream.getTracks().every(track => track.readyState === "ended")) {
        clock.source.stop();
        clock.source.disconnect();
        clock.stopped = true;
      }
    }
    for (const { analyser, data, kind, peer } of state.meters) {
      analyser.getFloatTimeDomainData(data);
      let sum = 0;
      for (const value of data) sum += value * value;
      state.samples.push({ at, peer, kind, rms: Math.sqrt(sum / data.length) });
    }
    state.samples = state.samples.filter(sample => sample.at > at - 240_000);
  }, 40);
  let statsPending = false;
  setInterval(async () => {
    if (statsPending) return;
    statsPending = true;
    try {
      for (const peer of state.peers) {
        if (peer.connectionState === "closed") continue;
        const rows = [...(await peer.getStats()).values()];
        state.transportSamples.push({ at: performance.now(), peer: peer.acceptanceId,
          captureState: capture.state, captureTime: capture.currentTime,
          outbound: rows.filter(row => row.type === "outbound-rtp" && row.kind === "audio")
            .map(row => ({ packetsSent: row.packetsSent, bytesSent: row.bytesSent })),
          inbound: rows.filter(row => row.type === "inbound-rtp" && row.kind === "audio")
            .map(row => ({ packetsReceived: row.packetsReceived, bytesReceived: row.bytesReceived,
              totalSamplesReceived: row.totalSamplesReceived, totalSamplesDuration: row.totalSamplesDuration,
              totalAudioEnergy: row.totalAudioEnergy })) });
      }
    } catch (error) {
      state.samplingErrors.push({ at: performance.now(), message: error.message });
    } finally {
      statsPending = false;
    }
  }, 500);
  const NativePeer = window.RTCPeerConnection;
  window.RTCPeerConnection = class extends NativePeer {
    constructor(...args) {
      super(...args);
      this.acceptanceId = state.peers.length;
      state.peers.push(this);
      mark("peer-created", { peer: this.acceptanceId });
      this.addEventListener("track", event => {
        if (event.track.kind === "audio") meter(new MediaStream([event.track]), "speaker", this.acceptanceId);
      });
      this.addEventListener("connectionstatechange", () => mark("connection-state", { peer: this.acceptanceId, state: this.connectionState }));
      this.addEventListener("iceconnectionstatechange", () => mark("ice-state", { peer: this.acceptanceId, state: this.iceConnectionState }));
      this.addEventListener("icegatheringstatechange", () => mark("ice-gathering", { peer: this.acceptanceId, state: this.iceGatheringState }));
    }
    async createOffer(...args) {
      mark("create-offer:start", { peer: this.acceptanceId });
      const offer = await super.createOffer(...args);
      mark("create-offer:end", { peer: this.acceptanceId });
      return offer;
    }
    async setLocalDescription(...args) {
      const result = await super.setLocalDescription(...args);
      mark("set-local:end", { peer: this.acceptanceId });
      return result;
    }
    async setRemoteDescription(...args) {
      mark("set-remote:start", { peer: this.acceptanceId });
      const result = await super.setRemoteDescription(...args);
      mark("set-remote:end", { peer: this.acceptanceId });
      return result;
    }
    createDataChannel(...args) {
      const channel = super.createDataChannel(...args);
      channel.addEventListener("open", () => mark("data-channel-open", { peer: this.acceptanceId }));
      channel.addEventListener("message", event => {
        const message = JSON.parse(event.data);
        state.events.push({ at: performance.now(), peer: this.acceptanceId, ...message });
      });
      const send = channel.send.bind(channel);
      channel.send = data => {
        state.sends.push({ peer: this.acceptanceId, at: performance.now(), type: JSON.parse(data).type });
        return send(data);
      };
      return channel;
    }
  };
  navigator.mediaDevices.getUserMedia = async constraints => {
    if (!constraints.audio || constraints.video) throw new Error("Only synthetic audio capture is allowed");
    mark("getUserMedia:start");
    await capture.resume();
    const output = capture.createMediaStreamDestination();
    // An unconnected destination stays live but stops generating RTP once a
    // finite WAV ends. Live context injection needs a continuous input clock,
    // including actual silence; keep zero PCM flowing without audible padding.
    const silence = capture.createConstantSource();
    silence.offset.value = 0;
    silence.connect(output);
    silence.start();
    state.inputClocks.push({ source: silence, output, stopped: false });
    state.inputs.push(output);
    meter(output.stream, "microphone", state.peers.length);
    state.output = output;
    for (const track of output.stream.getTracks()) track.acceptanceMicrophone = true;
    mark("getUserMedia:end");
    return output.stream;
  };
}
async function browserState(page) {
  return page.evaluate(() => {
    const state = window.voiceAcceptance;
    return { observedAt: performance.now(), events: state.events, samples: state.samples, sends: state.sends,
      timeline: state.timeline,
      capture: { state: state.capture.state, time: state.capture.currentTime },
      inputClocks: state.inputClocks.map(clock => ({ stopped: clock.stopped, value: clock.source.offset.value })),
      transportSamples: state.transportSamples, samplingErrors: state.samplingErrors,
      peers: state.peers.map(peer => ({ id: peer.acceptanceId, state: peer.connectionState,
        receivers: peer.getReceivers().map(receiver => receiver.track.readyState) })),
      inputs: state.inputs.map(input => input.stream.getTracks().map(track => ({ state: track.readyState, enabled: track.enabled }))),
      contexts: state.contexts.map(context => context.state), gains: state.gains.map(gain => gain.gain.value) };
  });
}
async function mark(page) {
  return page.evaluate(() => ({ at: performance.now(), peer: window.voiceAcceptance.peers.at(-1)?.acceptanceId }));
}
async function speak(page, names) {
  const encoded = names.map(name => fs.readFileSync(path.join(ASSETS, `${name}.wav`)).toString("base64"));
  const start = await mark(page);
  const duration = await page.evaluate(async files => {
    const state = window.voiceAcceptance;
    await state.capture.resume();
    let offset = state.capture.currentTime + 0.1;
    for (const file of files) {
      const bytes = Uint8Array.from(atob(file), char => char.charCodeAt(0));
      const buffer = await state.capture.decodeAudioData(bytes.buffer);
      const source = state.capture.createBufferSource();
      source.buffer = buffer;
      source.connect(state.output);
      source.start(offset);
      offset += buffer.duration + 0.08;
    }
    return (offset - state.capture.currentTime) * 1000;
  }, encoded);
  return { ...start, scheduledDurationMs: duration };
}

async function selfTestAudioClock() {
  const { chromium } = require("playwright");
  const server = http.createServer((_request, response) => response.end("<!doctype html><title>Local WebRTC clock test</title>"));
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  let browser;
  try {
    browser = await chromium.launch({ headless: true, env: childEnv() });
    const page = await browser.newPage();
    await page.addInitScript(instrumentBrowser);
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.evaluate(async () => {
      const microphone = await navigator.mediaDevices.getUserMedia({ audio: true });
      const sender = new RTCPeerConnection();
      const receiver = new RTCPeerConnection();
      const playback = document.createElement("audio");
      playback.autoplay = true;
      playback.muted = true;
      document.body.append(playback);
      const iceErrors = [];
      sender.onicecandidate = event => {
        if (event.candidate) receiver.addIceCandidate(event.candidate).catch(error => iceErrors.push(error.message));
      };
      receiver.onicecandidate = event => {
        if (event.candidate) sender.addIceCandidate(event.candidate).catch(error => iceErrors.push(error.message));
      };
      receiver.addEventListener("track", event => {
        playback.srcObject = new MediaStream([event.track]);
        void playback.play().catch(error => iceErrors.push(error.message));
      });
      for (const track of microphone.getTracks()) sender.addTrack(track, microphone);
      window.audioClockProbe = { sender, receiver, microphone, iceErrors };
      await sender.setLocalDescription(await sender.createOffer());
      await receiver.setRemoteDescription(sender.localDescription);
      await receiver.setLocalDescription(await receiver.createAnswer());
      await sender.setRemoteDescription(receiver.localDescription);
    });
    await page.waitForFunction(() => window.audioClockProbe.sender.connectionState === "connected",
      undefined, { timeout: 10_000 });
    const snapshot = label => page.evaluate(async label => {
      const { sender, receiver, microphone, iceErrors } = window.audioClockProbe;
      const outgoing = [...(await sender.getStats()).values()].filter(row => row.type === "outbound-rtp" && row.kind === "audio");
      const incoming = [...(await receiver.getStats()).values()].filter(row => row.type === "inbound-rtp" && row.kind === "audio");
      const state = window.voiceAcceptance;
      return { label, at: performance.now(), captureState: state.capture.state, captureTime: state.capture.currentTime,
        outgoing: outgoing.map(row => ({ packets: row.packetsSent, bytes: row.bytesSent })),
        incoming: incoming.map(row => ({ packets: row.packetsReceived, bytes: row.bytesReceived,
          samples: row.totalSamplesReceived, duration: row.totalSamplesDuration })),
        tracks: microphone.getTracks().map(track => ({ state: track.readyState, enabled: track.enabled })),
        microphonePeak: Math.max(0, ...state.samples.filter(sample => sample.kind === "microphone" &&
          sample.at > performance.now() - 1000).map(sample => sample.rms)), iceErrors };
    }, label);
    const advancing = (before, after) => {
      assert.equal(after.captureState, "running");
      const seconds = (after.at - before.at) / 1000;
      assert.ok(after.captureTime - before.captureTime >= seconds * 0.8, "Capture media clock must advance during silence");
      assert.equal(after.outgoing.length, 1);
      assert.equal(after.incoming.length, 1);
      for (const direction of ["outgoing", "incoming"]) {
        for (const sample of [before, after]) {
          assert.ok(Number.isFinite(sample[direction][0].packets), `${direction} packets must be observed, not defaulted`);
          assert.ok(Number.isFinite(sample[direction][0].bytes), `${direction} bytes must be observed, not defaulted`);
        }
        assert.ok(after[direction][0].packets - before[direction][0].packets >= seconds * 20,
          `${direction} real silence RTP must keep progressing: ${JSON.stringify({ before, after })}`);
        assert.ok(after[direction][0].bytes > before[direction][0].bytes);
      }
      assert.deepEqual(after.iceErrors, []);
      log("offline-audio-clock", { ...after, packetDelta: after.outgoing[0].packets - before.outgoing[0].packets,
        byteDelta: after.outgoing[0].bytes - before.outgoing[0].bytes });
    };
    await page.waitForTimeout(500);
    let before = await snapshot("initial");
    await page.waitForTimeout(2500);
    let after = await snapshot("pre-speech-silence");
    advancing(before, after);
    assert.equal(after.microphonePeak, 0, "Continuous source must produce zero PCM, not fake speech");
    const speech = await speak(page, ["reverse"]);
    await page.waitForTimeout(speech.scheduledDurationMs + 500);
    const speechState = await browserState(page);
    assert.ok(speechState.samples.some(sample => sample.kind === "microphone" && sample.at >= speech.at && sample.rms > 0.01),
      "Actual WAV must still enter the microphone stream");
    assert.ok(speechState.samples.some(sample => sample.kind === "speaker" && sample.at >= speech.at && sample.rms > 0.005),
      "Local native receiver must decode the actual speech");
    before = await snapshot("speech-ended");
    for (const index of [1, 2, 3]) {
      await page.waitForTimeout(2500);
      after = await snapshot(`post-speech-silence-${index}`);
      advancing(before, after);
      assert.equal(after.microphonePeak, 0, "WAV completion must leave only real silence");
      before = after;
    }
    await page.evaluate(() => {
      for (const track of window.audioClockProbe.microphone.getTracks()) track.enabled = false;
    });
    await page.waitForTimeout(2500);
    after = await snapshot("disabled-track-drain");
    advancing(before, after);
    assert.ok(after.tracks.every(track => !track.enabled), "Mute must stay honored while the media clock drains");
    await page.evaluate(() => {
      for (const track of window.audioClockProbe.microphone.getTracks()) track.stop();
      window.audioClockProbe.sender.close();
      window.audioClockProbe.receiver.close();
    });
    await page.waitForFunction(() => window.voiceAcceptance.inputClocks.every(clock => clock.stopped));
    const stopped = await browserState(page);
    assert.ok(stopped.inputs.flat().every(track => track.state === "ended"));
    assert.deepEqual(stopped.samplingErrors, []);
    log("offline-audio-pass", { providerCalls: 0, realWebRtc: true, continuousZeroPcm: true,
      microphoneSpeechPeak: Math.max(...speechState.samples.filter(sample => sample.kind === "microphone").map(sample => sample.rms)),
      decodedSpeechPeak: Math.max(...speechState.samples.filter(sample => sample.kind === "speaker").map(sample => sample.rms)),
      stoppedClockAfterTrackEnd: true });
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}

async function runBrowser(url, facts, directory, gate) {
  const { chromium } = require("playwright");
  const browser = await chromium.launch({ headless: true, env: childEnv() });
  const deadline = setTimeout(() => void browser.close(), 12 * 60_000);
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const requests = [];
  const pageErrors = [];
  const observations = { coverage: COVERAGE, directControls: [] };
  let keeperObserver;
  let rpcSequence = 0;
  const rpcRaw = async (method, params = {}) => {
    const response = await fetch(`${url}/console/rpc`, { method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: `acceptance-${++rpcSequence}`, method, params }),
      signal: AbortSignal.timeout(15_000) });
    assert.equal(response.status, 200, `${method} HTTP ${response.status}`);
    const data = await response.json();
    if (method.startsWith("meta/live/") || method.startsWith("meta/console/voice/")) {
      observations.directControls.push({ method, at: Date.now(), identity: params.identity,
        channelId: params.channel_id, requestId: params.request_id, phase: data.result?.phase,
        errorCode: data.error?.code, errorKind: data.error?.data?.kind,
        errorMessage: data.error && redact(data.error.message) });
    }
    return data;
  };
  const rpc = async (method, params = {}) => {
    const data = await rpcRaw(method, params);
    assert.ok(!data.error, `${method}: ${redact(JSON.stringify(data.error))}`);
    return data.result;
  };
  const controlParams = active => ({ identity: PRIMARY, channel_id: active.channelId,
    activation_receipt: active.activationReceipt });
  const status = active => rpc("meta/live/status", controlParams(active));
  const history = new Map();
  const sessionIds = new Map();
  const frames = async identity => {
    const result = await rpc("meta/console/query_timeline", { identity, mode: "recent", limit: 1000 });
    for (const frame of result.frames ?? []) history.set(frame.id, frame);
    return [...history.values()].filter(frame => frame.identity === identity);
  };
  const watermark = async identity => new Set((await frames(identity)).map(frame => frame.id));
  const since = async (identity, previous) => (await frames(identity)).filter(frame => !previous.has(frame.id));
  const final = (identity, previous, expected, scope = {}) => poll(`${identity} source final: ${expected}`, async () =>
    (await since(identity, previous)).find(frame => contains(finalText(frame,
      { identity, ...(sessionIds.has(identity) ? { session_id: sessionIds.get(identity) } : {}), ...scope }), expected)), 120_000, 500);
  const sourceSend = (identity, previous, expected) => poll(`${identity} successful native send: ${expected}`,
    async () => successfulSend(await since(identity, previous), expected), 120_000, 500);
  const send = async (identity, content) => {
    await page.getByTestId(`chat-composer:${identity}`).fill(content);
    const start = requests.length;
    await page.getByTestId(`chat-send:${identity}`).click();
    return poll("console accepted typed input", () => {
      const sent = requests.slice(start).find(row => row.method === "meta/console/send" && row.params.identity === identity);
      if (sent?.error) throw new Error(`Typed send failed: ${redact(JSON.stringify(sent.error))}`);
      return sent?.result && sent;
    });
  };
  const peerSend = content => rpc("meta/console/send", { identity: KEEPER, content,
    origin: "voice-live-acceptance", idempotency_key: crypto.randomUUID() });
  const hear = async (start, expected, label = expected) => {
    const evidence = await poll(`native spoken output: ${label}`, async () => {
      const state = await browserState(page);
      assert.ok(state.gains.some(gain => gain > 0), "UI speaker output must be unmuted while checking spoken audio");
      return spokenEvidence(state, start, expected);
    }, 90_000);
    log("heard", { label, ...evidence });
    return evidence;
  };
  const inputHeard = async (start, expected) => {
    await poll(`native input transcript: ${expected}`, async () => {
      const state = await browserState(page);
      return contains(state.events.filter(event => event.peer === start.peer && event.at >= start.at &&
        event.type === "session.input_transcript.delta").map(event => event.delta ?? "").join(""), expected) &&
        state.samples.filter(sample => sample.peer === start.peer && sample.at >= start.at &&
          sample.kind === "microphone" && sample.rms > 0.01).length >= 4;
    }, 45_000);
  };
  let original;
  const sameOriginal = async () => {
    const current = (await rpc("meta/console/inspect_identity", { identity: PRIMARY })).identity;
    assert.equal(current.runtime_member_id, original.runtime_member_id, "Delegation must keep ORIGINAL member");
    assert.equal(current.session_id, original.session_id, "Delegation must keep ORIGINAL background session");
  };
  const open = async () => {
    const begin = requests.length;
    await page.getByRole("button", { name: `Start voice with ${LABEL}`, exact: true }).click({ timeout: 30_000 });
    await page.getByTestId("voice-bar").and(page.locator('[data-phase="active"]')).waitFor({ timeout: 45_000 });
    const row = requests.slice(begin).find(request => request.method === "meta/console/voice/open" && request.result);
    assert.ok(row, "Real open RPC must succeed");
    assert.equal(row.params.identity, PRIMARY);
    assert.equal(row.result.execution_mode, "client_context");
    assert.equal(row.result.target_identity, PRIMARY);
    const activated = requests.slice(begin).find(request => request.method === "meta/live/status" &&
      request.result?.phase === "active");
    assert.ok(activated, "Server must report real active provider session");
    assert.equal(activated.result.handle.channel_id, row.result.channel_id);
    assert.equal(activated.result.handle.target_identity, PRIMARY);
    const activationReceipt = activated.result.handle.activation_receipt;
    assert.equal(typeof activationReceipt, "string");
    assert.ok(activationReceipt.length > 0);
    const issuedReceipts = requests.slice(begin).flatMap(request => receipts(request.result));
    assert.ok(issuedReceipts.every(receipt => receipt.length > 0));
    secrets.push(...issuedReceipts);
    return { requestId: row.params.request_id, channelId: row.result.channel_id, start: await mark(page),
      activationReceipt, receipts: issuedReceipts };
  };
  const close = async active => {
    const start = Date.now();
    await page.getByRole("button", { name: "End voice conversation", exact: true }).click();
    await poll("immediate local microphone and speaker mute", async () => {
      const state = await browserState(page);
      return state.inputs.flat().every(track => !track.enabled || track.state === "ended") &&
        state.gains.every(gain => gain === 0);
    }, 1000, 25);
    const closed = await poll("exact server cleanup under 5 seconds", () => requests.find(row =>
      row.method === "meta/console/voice/close" && row.params.request_id === active.requestId &&
      row.params.identity === PRIMARY && row.result?.phase === "closed"), Math.max(1, 4900 - (Date.now() - start)), 25);
    assert.ok(closed.ended - start < 5000, `Server close took ${closed.ended - start}ms`);
    await poll("all local media and peers stopped", async () => {
      const state = await browserState(page);
      return state.inputs.flat().every(track => track.state === "ended") &&
        state.peers.every(peer => peer.state === "closed") && state.contexts.every(context => context === "closed");
    }, Math.max(1, 5000 - (Date.now() - start)), 25);
    const localMs = Date.now() - start;
    assert.equal(await page.getByTestId("voice-bar").count(), 0);
    const receipt = { requestId: active.requestId, phase: closed.result.phase, serverMs: closed.ended - start, localMs };
    (observations.closes ??= []).push(receipt);
    log("closed", receipt);
    // Positive cleanup is proven above. The console releases channel custody
    // after close, so the retired channel must now fail its exact owner guard.
    assertRetiredChannelGuard(await rpcRaw("meta/live/status", controlParams(active)));
  };
  page.on("request", request => {
    if (!request.url().endsWith("/console/rpc")) return;
    const data = request.postDataJSON();
    requests.push({ request, method: data.method, params: data.params ?? {}, started: Date.now() });
  });
  page.on("response", async response => {
    const row = requests.find(candidate => candidate.request === response.request());
    if (!row) return;
    try {
      const data = await response.json();
      Object.assign(row, { result: data.result, error: data.error, ended: Date.now() });
    } catch { row.unreadable = true; }
  });
  page.on("pageerror", error => pageErrors.push(redact(error.message)));
  try {
    await page.addInitScript(instrumentBrowser);
    await page.goto(`${url}/console`);
    await page.getByTestId(`chat-composer:${PRIMARY}`).waitFor({ timeout: 30_000 });
    const [warm, keeperWarm] = await Promise.all([watermark(PRIMARY), watermark(KEEPER)]);
    const [primaryBarrier, keeperBarrier] = await Promise.all([
      send(PRIMARY, `Bootstrap barrier. Process this after prior queued startup work. Use no peer or shell tools for this operator message. Reply exactly ORIGINAL READY ${facts.nonce}.`),
      peerSend(`Bootstrap barrier. Process this after prior queued startup work. Use no peer or shell tools for this operator message. Reply exactly KEEPER READY ${facts.nonce}.`),
    ]);
    const barriers = await Promise.all([
      final(PRIMARY, warm, `ORIGINAL READY ${facts.nonce}`, { interaction_id: primaryBarrier.result.interaction_id }),
      final(KEEPER, keeperWarm, `KEEPER READY ${facts.nonce}`, { interaction_id: keeperBarrier.interaction_id }),
    ]);
    const records = [];
    for (const identity of [PRIMARY, KEEPER]) {
      const record = (await rpc("meta/console/inspect_identity", { identity })).identity;
      assert.equal(barriers.find(frame => frame.identity === identity)?.session_id, record.session_id,
        "Startup barrier must belong to the inspected canonical session");
      sessionIds.set(identity, record.session_id);
      records.push(record);
    }
    let lastWitness;
    let stableReads = 0;
    const bootstrapDeadline = Date.now() + 60_000;
    observations.bootstrapAdmissionRetries = 0;
    const bootstrap = await poll("both real agents idle after canonical startup barriers", async () => {
      const rows = [];
      observations.bootstrapLatest = rows;
      // These probes share one actor observation lane; parallel deep reads
      // can saturate that lane even when the two members themselves are idle.
      for (const record of records) {
        observations.bootstrapProbe = { identity: record.identity, method: "meta/status_identity" };
        const identityStatus = await rpc("meta/status_identity", { identity: record.identity });
        observations.bootstrapProbe.method = "meta/member_status";
        const memberResponse = await rpcRaw("meta/member_status", { member_id: record.runtime_member_id });
        if (memberObservationPending("meta/member_status", memberResponse)) {
          stableReads = 0;
          lastWitness = undefined;
          observations.bootstrapAdmissionRetries++;
          observations.bootstrapLastCause = memberResponse.error.message;
          log("bootstrap-observation-pending", { identity: record.identity,
            retry: observations.bootstrapAdmissionRetries, remainingMs: Math.max(0, bootstrapDeadline - Date.now()),
            cause: memberResponse.error.message });
          return null;
        }
        assert.ok(!memberResponse.error, `meta/member_status: ${redact(JSON.stringify(memberResponse.error))}`);
        const member = memberResponse.result;
        assert.ok(!["broken", "retiring", "retired"].includes(identityStatus.state),
          `Bootstrap member unhealthy: ${record.identity} ${identityStatus.state}`);
        assert.ok(!member.error && !member.kickoff?.error,
          `Bootstrap member failed: ${record.identity}: ${member.error ?? member.kickoff?.error}`);
        rows.push({ identity: record.identity, identityStatus, member,
          idle: bootstrapMemberIdle(record.identity, identityStatus, member) });
      }
      if (!rows.every(row => row.idle)) {
        stableReads = 0;
        lastWitness = undefined;
        return null;
      }
      const witness = JSON.stringify(rows.map(row => ({
        identity: row.identity, session: row.identityStatus.session_id,
        generation: row.identityStatus.generation, progress: row.member.progress,
        tokens: row.member.tokens_used, output: row.member.output_preview, kickoff: row.member.kickoff,
      })));
      stableReads = witness === lastWitness ? stableReads + 1 : 1;
      lastWitness = witness;
      return stableReads >= 3 && rows;
    }, Math.max(1, bootstrapDeadline - Date.now()), 750).catch(error => {
      throw new Error(`${redact(error.message)}; last bootstrap admission cause: ${observations.bootstrapLastCause ?? "none"}`);
    });
    observations.bootstrap = { barriers: barriers.map(frame => frame.id), stableReads, members: bootstrap };
    log("bootstrap-ready", { canonicalBarriers: barriers.length, stableReads,
      members: bootstrap.map(row => ({ identity: row.identity, progress: row.member.progress })) });
    original = (await rpc("meta/console/inspect_identity", { identity: PRIMARY })).identity;
    assert.ok(original.session_id && original.runtime_member_id, "Warmup must establish a real original session");
    const active = await open();

    log("phase-1", { case: "spoken request -> native delegation -> original agent/keeper exchange -> keeper-only verified spoken result" });
    const [beforeVoice, beforeWordKeeper] = await Promise.all([watermark(PRIMARY), watermark(KEEPER)]);
    assert.ok(!(await frames(PRIMARY)).some(frame => contains(finalText(frame), facts.wordVerification)),
      "The keeper-only verification code must not be available in the original agent's seed history");
    const spoken = await speak(page, ["reverse", ...facts.words, "finish"]);
    await inputHeard(spoken, facts.words.join(" "));
    await poll("native client delegation for real keeper lookup", async () =>
      (await browserState(page)).events.find(event => event.at >= spoken.at && event.peer === spoken.peer &&
        event.type === "session.delegation.created" && event.delegation?.target === "client"), 45_000);
    const reversed = [...facts.words].reverse().join(" ");
    const requestToKeeper = await sourceSend(PRIMARY, beforeVoice, facts.words.join(" "));
    assert.ok(contains(JSON.stringify(requestToKeeper.payload.args ?? requestToKeeper.payload.arguments), "VERIFY_WORDS"),
      "Original background agent must request actual verification, not just compute a reversal");
    const keeperReply = await sourceSend(KEEPER, beforeWordKeeper, facts.wordVerification);
    assert.ok(contains(JSON.stringify(keeperReply.payload.args ?? keeperReply.payload.arguments), reversed),
      "Keeper's successful native reply must carry its computed reversed words and keeper-only code");
    const keeperSource = await final(KEEPER, beforeWordKeeper, facts.wordVerification);
    assert.ok(verifiedWordReply(keeperSource, facts.words, facts.wordVerification),
      "Keeper's actual model final must verify both reversal and its code");
    const source = await final(PRIMARY, beforeVoice, facts.wordVerification);
    assert.ok(verifiedWordReply(source, facts.words, facts.wordVerification),
      "Original background final must report the complete verified peer reply");
    assert.equal(source.session_id, original.session_id, "Spoken result must come from original background session");
    assert.ok(keeperSource.session_id && keeperSource.session_id !== original.session_id,
      "Keeper verification must execute in its own real backend session");
    await hear(spoken, reversed, "keeper-verified reversed words");
    await hear(spoken, facts.wordVerification, "keeper-only verification code");
    observations.wordVerification = { primaryRequestFrameId: requestToKeeper.id, keeperReplyFrameId: keeperReply.id,
      keeperFinalFrameId: keeperSource.id, originalFinalFrameId: source.id };
    await sameOriginal();

    log("phase-2", { case: "typed exact value once -> original agent -> fresh spoken value and recall" });
    const beforeTyped = await watermark(PRIMARY);
    const typedMark = await mark(page);
    const content = `CURRENT_VALUE ${facts.typed}. Request nonce ${facts.nonce}-typed. Store this exact four-word value and confirm all four words once.`;
    const typed = await send(PRIMARY, content);
    await final(PRIMARY, beforeTyped, facts.typed, { interaction_id: typed.result.interaction_id });
    const typedSource = readCommittedTypedSource(directory, original.session_id);
    const canonicalTypedUser = assertCanonicalTypedUser(typedSource, original.session_id, typed.result.interaction_id, content);
    observations.typedCanonicalSource = { sessionId: typedSource.sessionId, storeRevision: typedSource.storeRevision,
      blobSha256: typedSource.blobSha256, interactionId: typed.result.interaction_id, message: canonicalTypedUser };
    await hear(typedMark, facts.typed, "typed exact value");
    const recall = await speak(page, ["recall"]);
    await inputHeard(recall, "current console value");
    await hear(recall, facts.typed, "subsequent spoken recall");
    assert.equal(requests.filter(row => row.method === "meta/console/send" && row.params.content === content).length, 1);
    assertTypedSendAdmission(await frames(PRIMARY), { runtime_key: original.runtime_key, identity: PRIMARY,
      session_id: original.session_id, interaction_id: typed.result.interaction_id }, content);
    assertCanonicalTypedUser(readCommittedTypedSource(directory, original.session_id),
      original.session_id, typed.result.interaction_id, content);
    await sameOriginal();

    log("phase-3", { case: "real peer holds reply until after initial background answer, then fresh Live update" });
    const beforePeer = await watermark(PRIMARY);
    const keeperBefore = await watermark(KEEPER);
    const peerMark = await speak(page, ["peer"]);
    await inputHeard(peerMark, "launch code");
    await sourceSend(PRIMARY, beforePeer, "launch");
    await final(PRIMARY, beforePeer, "WAITING FOR KEEPER");
    await final(KEEPER, keeperBefore, "HELD LAUNCH");
    assert.ok(!(await since(PRIMARY, beforePeer)).some(frame => contains(finalText(frame), facts.peer)),
      "Primary must not know the keeper-only fresh fact before release");
    const releaseMark = await mark(page);
    await peerSend(`RELEASE LAUNCH. Operator nonce ${facts.nonce}-release. Send the verified launch code to its waiting requester now.`);
    await sourceSend(KEEPER, keeperBefore, facts.peer);
    await final(PRIMARY, beforePeer, facts.peer);
    await hear(releaseMark, facts.peer, "late verified peer result (no extra voice prompt)");
    await sameOriginal();

    log("phase-4", { case: "close during background work; hold an old ordinary backend operation across reopen" });
    const overlapBefore = await watermark(PRIMARY);
    const overlapKeeper = await watermark(KEEPER);
    const overlap = await speak(page, ["overlap"]);
    await send(PRIMARY, `Start the overlap typed check with the keeper NOW. Request nonce ${facts.nonce}-overlap. Send the request and give an initial waiting answer, then announce its verified result when it arrives.`);
    await inputHeard(overlap, "overlap voice");
    await sourceSend(PRIMARY, overlapBefore, facts.voiceOperation);
    await sourceSend(PRIMARY, overlapBefore, facts.typedOperation);
    await final(KEEPER, overlapKeeper, "HELD OVERLAP VOICE");
    await final(KEEPER, overlapKeeper, "HELD OVERLAP TYPED");
    const oldDelegations = (await browserState(page)).events.filter(event => event.peer === overlap.peer &&
      event.at >= overlap.at && event.type === "session.delegation.created" && event.delegation?.target === "client")
      .map(event => event.delegation.id);
    assert.ok(oldDelegations.length, "The held voice operation must originate during a real native client delegation");
    assert.ok(!(await since(PRIMARY, overlapBefore)).some(frame =>
      contains(finalText(frame), facts.oldVoice) || contains(finalText(frame), facts.oldTyped)));
    await final(PRIMARY, overlapBefore, facts.voiceOperation);
    await peerSend(`RELEASE OPERATION ${facts.typedOperation}. Send ONLY that held operation's result now. Keep ${facts.voiceOperation} held; it is not released.`);
    await sourceSend(KEEPER, overlapKeeper, facts.oldTyped);
    await final(PRIMARY, overlapBefore, facts.oldTyped);
    const concurrentContent = `CURRENT_VALUE ${facts.overlapValue}. Request nonce ${facts.nonce}-concurrent-append. Replace the earlier console value and confirm these four words while the keeper checks finish.`;
    const concurrent = await send(PRIMARY, concurrentContent);
    await final(PRIMARY, overlapBefore, facts.overlapValue, { interaction_id: concurrent.result.interaction_id });
    const concurrentSource = readCommittedTypedSource(directory, original.session_id);
    const canonicalConcurrentUser = assertCanonicalTypedUser(concurrentSource,
      original.session_id, concurrent.result.interaction_id, concurrentContent);
    observations.concurrentTypedCanonicalSource = { sessionId: concurrentSource.sessionId, storeRevision: concurrentSource.storeRevision,
      blobSha256: concurrentSource.blobSha256, interactionId: concurrent.result.interaction_id, message: canonicalConcurrentUser };
    observations.nativeKeeperSubscription = {};
    keeperObserver = await subscribeNativeAgent(url, KEEPER, observations.nativeKeeperSubscription);
    observations.nativeKeeperEvents = keeperObserver.events;
    observations.nativeKeeperSubscribedAt = Date.now();
    await peerSend(`BEGIN EXTERNAL VERIFICATION ${facts.voiceOperation}. Execute this exact command once using shell, background=false, timeout_secs=250: ${gate.command}\nThe endpoint will return the verified result later. Wait for the real shell result, then send it once to the original requester. The launch and typed operations are unrelated.`);
    await poll("keeper's real external verification request", () => {
      keeperObserver.check();
      return gate.snapshot().phase === "waiting";
    }, 45_000);
    observations.externalGate = { requestStarted: gate.snapshot() };
    const gateCall = await poll("native foreground shell start for held verification", () => {
      keeperObserver.check();
      return pendingGateTool(keeperObserver.events, gate.command, gate.snapshot());
    }, 20_000);
    assert.ok(!successfulSend(await since(KEEPER, overlapKeeper), facts.oldVoice),
      "No verified result may be delivered while its real external request is still blocked");
    keeperObserver.check();
    assert.ok(pendingGateTool(keeperObserver.events, gate.command, gate.snapshot()),
      "Close requires an observed, live foreground tool request, not completed work or prompt intent");
    Object.assign(observations.externalGate, { beforeClose: gate.snapshot(), toolCallId: gateCall.payload.tool_call_id ?? gateCall.payload.id });
    log("background-inflight-at-close", { keeperForegroundTool: observations.externalGate.toolCallId,
      externalRequest: gate.snapshot(), canonicalMirrorPending: "not claimed" });
    await close(active);
    const reopenedRequestIndex = requests.length;
    const reopened = await open();
    assertReopenedScope(active, reopened, requests.slice(reopenedRequestIndex), [], oldDelegations);
    keeperObserver.check();
    assert.ok(pendingGateTool(keeperObserver.events, gate.command, gate.snapshot()),
      "Keeper's same real foreground operation must remain in flight after the new channel becomes active");
    observations.externalGate.afterReopen = gate.snapshot();
    assert.ok(!successfulSend(await since(KEEPER, overlapKeeper), facts.oldVoice),
      "Old voice operation must still be unreleased after the new provider channel becomes active");
    assert.ok(hasPendingResults(await since(PRIMARY, overlapBefore), [facts.oldVoice]),
      "The old voice result must not already be in canonical history at reopen");
    const newBefore = await watermark(PRIMARY);
    const newMark = await mark(page);
    const reopenedInput = await send(PRIMARY, `CURRENT_VALUE ${facts.reopened}. Request nonce ${facts.nonce}-reopened. Replace the earlier value. Confirm only this fresh four-word value; do not recap previous tasks.`);
    await final(PRIMARY, newBefore, facts.reopened, { interaction_id: reopenedInput.result.interaction_id });
    await hear(newMark, facts.reopened, "new-session current value");

    const lateRelease = await mark(page);
    gate.release();
    observations.externalGate.released = gate.snapshot();
    const shellResult = await poll("successful real foreground verification result", async () => {
      keeperObserver.check();
      const result = (await since(KEEPER, overlapKeeper)).find(frame => toolResultWithBody(frame) &&
        (frame.payload.tool_call_id ?? frame.payload.id) === observations.externalGate.toolCallId);
      if (!result) return null;
      assert.equal(result.payload.is_error, false, "External verification must not return a tool error");
      const output = JSON.parse(text(result.payload));
      assert.equal(output.exit_code, 0);
      assert.equal(output.timed_out, false);
      assert.deepEqual(JSON.parse(output.stdout), { operation_id: facts.voiceOperation, verified_code: facts.oldVoice });
      return result;
    }, 30_000);
    observations.externalGate.resultFrameId = shellResult.id;
    const nativeResult = await poll("native result of the same foreground shell execution", () => {
      keeperObserver.check();
      return keeperObserver.events.find(event => event.kind === "tool_result_received" &&
        (event.payload.tool_call_id ?? event.payload.id) === observations.externalGate.toolCallId);
    }, 5000);
    assert.ok(keeperObserver.events.some(event => event.kind === "tool_execution_completed" &&
      (event.payload.tool_call_id ?? event.payload.id) === observations.externalGate.toolCallId &&
      event.payload.is_error === false), "The native execution fact precedes its result");
    assert.equal(nativeResult.payload.is_error, false);
    assert.equal(JSON.parse(text(nativeResult.payload)).exit_code, 0);
    observations.externalGate.nativeResultEventId = nativeResult.id;
    const lateSend = await sourceSend(KEEPER, overlapKeeper, facts.oldVoice);
    assert.ok(contains(JSON.stringify(lateSend.payload.args ?? lateSend.payload.arguments), facts.voiceOperation),
      "Late delivery must identify the exact operation held in the old call");
    const lateFinal = await final(PRIMARY, overlapBefore, facts.oldVoice);
    assert.equal(lateFinal.session_id, original.session_id, "Late result must join the ORIGINAL canonical session");
    assert.ok(contains(finalText(lateFinal), facts.voiceOperation), "Canonical result must identify the old operation");
    const lateSpoken = await hear(lateRelease, facts.oldVoice, "late old operation through legitimate current canonical context");
    observations.lateOperation = { operationId: facts.voiceOperation, oldDelegations,
      currentChannelId: reopened.channelId, sourceFrameId: lateFinal.id, sourceSessionId: lateFinal.session_id,
      releasedOnPeer: lateRelease.peer, ...lateSpoken };
    await final(PRIMARY, overlapBefore, facts.oldTyped);
    await final(PRIMARY, overlapBefore, facts.overlapValue);
    assert.equal(requests.filter(row => row.method === "meta/console/send" &&
      row.params.content === concurrentContent).length, 1, "Concurrent typed update must be admitted only once");
    assertTypedSendAdmission(await frames(PRIMARY), { runtime_key: original.runtime_key, identity: PRIMARY,
      session_id: original.session_id, interaction_id: concurrent.result.interaction_id }, concurrentContent);
    assertCanonicalTypedUser(readCommittedTypedSource(directory, original.session_id),
      original.session_id, concurrent.result.interaction_id, concurrentContent);

    assert.equal((await status(reopened)).phase, "active");
    // This is an intentional adversarial control against this test's isolated
    // gateway, not UI traffic exempted from the browser receipt-isolation check.
    const mismatchedClose = await rpcRaw("meta/live/close", {
      ...controlParams(reopened), activation_receipt: active.activationReceipt,
    });
    assert.equal(mismatchedClose.error?.code, -32602,
      "The old real activation receipt must be rejected when targeting the new channel");
    assert.ok(!mismatchedClose.result, "Rejected stale-receipt close must not report success");
    assert.equal((await status(reopened)).phase, "active", "Stale receipt must not close the new provider channel");
    const oldClose = await rpc("meta/console/voice/close", { identity: PRIMARY, request_id: active.requestId });
    assert.equal(oldClose.phase, "closed", "The old request's idempotent close must stay scoped to that request");
    assert.equal((await status(reopened)).phase, "active", "Stale request close must leave the new channel active");
    observations.staleControls = { rejectedOldActivationReceipt: true, oldRequestRemainedClosed: true,
      currentChannelId: reopened.channelId };

    const newRecall = await speak(page, ["recall"]);
    await inputHeard(newRecall, "current console value");
    await hear(newRecall, facts.reopened, "new-session spoken recall");
    await sameOriginal();
    const state = await browserState(page);
    const currentEvents = state.events.filter(event => event.peer === reopened.start.peer);
    const lateText = currentEvents.filter(event => event.at >= lateRelease.at &&
      event.type === "session.output_transcript.delta").map(event => event.delta ?? "").join("");
    assert.equal(occurrences(lateText, facts.oldVoice), 1,
      "The late ordinary result must be spoken once in the observed current-channel window");
    const primaryAfterLate = await since(PRIMARY, overlapBefore);
    const keeperAfterLate = await since(KEEPER, overlapKeeper);
    assert.equal(successfulSends(primaryAfterLate, facts.voiceOperation).length, 1,
      "Reopen and stale controls must not re-execute the old backend operation");
    assert.equal(successfulSends(keeperAfterLate, facts.oldVoice).length, 1, "Keeper must deliver the old operation once");
    assert.equal(backgroundResponseIds(primaryAfterLate, facts.oldVoice).size, 1,
      "The late result must have one real canonical background answer, not multiple executions");
    observations.lateOperation.observedThrough = state.observedAt;
    observations.lateOperation.nativeSpokenOccurrences = 1;
    observations.lateOperation.canonicalResponseCount = 1;
    observations.lateOperation.providerAppendCount = "not publicly observable";
    const recallText = currentEvents.filter(event => event.at >= newRecall.at &&
      event.type === "session.output_transcript.delta").map(event => event.delta ?? "").join("");
    for (const old of [facts.typed, facts.overlapValue]) assert.ok(!contains(recallText, old),
      `Recall returned an obsolete CURRENT_VALUE after the late operation: ${old}`);
    assertReopenedScope(active, reopened, requests.slice(reopenedRequestIndex), currentEvents, oldDelegations);
    assert.equal((await status(reopened)).phase, "active",
      "Current provider must remain active after legitimate late canonical delivery");
    assert.equal(state.sends.length, 0, "Harness/UI must not invent native user text or playback ACKs");
    assert.deepEqual(state.samplingErrors, [], "Transport diagnostics must not silently fail");
    keeperObserver.check();
    await close(reopened);
    assert.deepEqual(pageErrors, []);
    log("paid-pass", { phases: 4, originalMember: original.runtime_member_id,
      originalSession: original.session_id, coverage: COVERAGE });
  } finally {
    clearTimeout(deadline);
    try {
      await keeperObserver?.stop();
      await page.screenshot({ path: path.join(directory, "console.png") })
        .catch(error => pageErrors.push(`diagnostic screenshot: ${redact(error.message)}`));
      const end = page.getByRole("button", { name: "End voice conversation", exact: true });
      if (await end.count().catch(() => 0)) {
        await end.click().catch(error => pageErrors.push(`failure cleanup click: ${redact(error.message)}`));
        await sleep(5200);
      }
      const state = await browserState(page).catch(error => {
        pageErrors.push(`diagnostic browser state: ${redact(error.message)}`);
        return null;
      });
      fs.writeFileSync(path.join(directory, "evidence.json"), redact(JSON.stringify({
        original, facts, pageErrors, observations, browser: state,
        frames: [...history.values()],
        rpc: requests.map(({ method, params, started, ended, result, error }) => ({
          method, started, ended, identity: params.identity, requestId: params.request_id, channelId: params.channel_id,
          phase: result?.phase, error,
        })),
      }, null, 2)), { mode: 0o600 });
    } finally {
      await browser.close();
    }
  }
}

function makeFacts(nonce) {
  const facts = { nonce, words: randomWords(3), voiceOperation: `old-voice-${nonce}`, typedOperation: `old-typed-${nonce}` };
  const chosen = new Set();
  for (const key of ["wordVerification", "typed", "peer", "oldVoice", "oldTyped", "overlapValue", "reopened"]) {
    let value;
    do { value = randomWords(4).join(" "); } while (chosen.has(value));
    chosen.add(value);
    facts[key] = value;
  }
  return facts;
}

const TTT_RUST_LOG = "warn,meerkat_meta=info,meerkat_meta::console_voice::timing=debug," +
  "meerkat::experimental_gpt_live=info,meerkat::session_runtime::live_orchestration=info,meerkat_openai::public_live=info";
const median = values => {
  const sorted = [...values].filter(value => Number.isFinite(value)).sort((a, b) => a - b);
  if (!sorted.length) return null;
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : Math.round((sorted[middle - 1] + sorted[middle]) / 2);
};
const maximum = values => {
  const finite = values.filter(value => Number.isFinite(value));
  return finite.length ? Math.max(...finite) : null;
};

// One activation, expressed as milliseconds after the click. Browser marks are
// on the page performance clock relative to the "click" mark; RPC rows are on
// the harness wall clock relative to clickWall (the two differ by one evaluate
// round trip, a few milliseconds).
function summarizeActivation({ clickWall, activeWall, rows, timeline }) {
  const clickAt = timeline.find(entry => entry.name === "click")?.at ?? 0;
  const markAt = (name, predicate = () => true) => {
    const entry = timeline.find(item => item.name === name && item.at >= clickAt && predicate(item));
    return entry ? Math.round(entry.at - clickAt) : null;
  };
  const rpcRow = method => rows.find(row => row.method === method && row.started >= clickWall - 50 && row.ended);
  const rpc = method => {
    const row = rpcRow(method);
    return row ? { at: row.started - clickWall, ms: row.ended - row.started, ok: !row.error } : null;
  };
  const statusRows = rows.filter(row => row.method === "meta/live/status" && row.params.pending_receipt &&
    row.started >= clickWall && row.ended);
  const activeRow = statusRows.find(row => row.result?.phase === "active");
  const status = statusRows.length ? {
    count: statusRows.indexOf(activeRow) + 1 || statusRows.length,
    firstAt: statusRows[0].started - clickWall,
    activeAt: activeRow ? activeRow.ended - clickWall : null,
    spanMs: activeRow ? activeRow.ended - statusRows[0].started : null,
    maxPollMs: Math.max(...statusRows.map(row => row.ended - row.started)),
    medianPollMs: median(statusRows.map(row => row.ended - row.started)),
  } : null;
  const answerReceived = rpc("meta/console/voice/answer_received");
  return {
    totalUiActiveMs: activeWall - clickWall,
    micEnabledAt: markAt("mic-enabled"),
    readiness: rpc("meta/console/voice/readiness"),
    getUserMedia: { at: markAt("getUserMedia:start"), ms: markAt("getUserMedia:end") - markAt("getUserMedia:start") },
    open: rpc("meta/console/voice/open"),
    offerLocal: { at: markAt("peer-created"), ms: markAt("set-local:end") - markAt("peer-created") },
    iceGatheringComplete: markAt("ice-gathering", entry => entry.state === "complete"),
    register: rpc("meta/live/playback_owner/register"),
    answer: rpc("live/webrtc/answer"),
    setRemoteMs: markAt("set-remote:end") - markAt("set-remote:start"),
    answerReceived,
    iceConnectedAt: markAt("ice-state", entry => entry.state === "connected" || entry.state === "completed"),
    peerConnectedAt: markAt("connection-state", entry => entry.state === "connected"),
    dataChannelOpenAt: markAt("data-channel-open"),
    status,
    answerReceivedToActiveMs: answerReceived && status?.activeAt !== null && status ?
      status.activeAt - (answerReceived.at + answerReceived.ms) : null,
  };
}

function sessionSize(directory, sessionId) {
  const database = path.join(directory, "state", "runtime.sqlite");
  const result = spawnSync("python3", ["-c", `
import json, sqlite3, sys
from pathlib import Path
database, session_id = sys.argv[1:]
connection = sqlite3.connect(Path(database).as_uri() + "?mode=ro", uri=True, timeout=5)
try:
    rows = connection.execute("""
        SELECT length(bodies.session_snapshot), bodies.session_snapshot
        FROM runtime_whole_blob_authority AS authority
        JOIN runtime_whole_blob_bodies AS bodies ON bodies.blob_sha256 = authority.blob_sha256
        WHERE authority.session_id = ?
    """, (session_id,)).fetchall()
    if len(rows) != 1:
        raise RuntimeError("expected one committed whole-blob authority")
    size, data = rows[0]
    print(json.dumps({"bytes": size, "messages": len(json.loads(data).get("messages", []))}))
finally:
    connection.close()
`, database, sessionId], { env: childEnv(), encoding: "utf8", timeout: 10_000, maxBuffer: 64 * 1024 * 1024 });
  if (result.status !== 0) return { error: redact(result.stderr || result.error?.message || "unknown") };
  return JSON.parse(result.stdout);
}

// PAID diagnostic: stage timings for click -> able to talk over several runs.
async function timeToTalk({ runs, seedTurns, seedWords, holdMs }) {
  const apiKey = process.env.OPENAI_API_KEY || process.env.OPENAI_API_KEY_OLD;
  assert.ok(apiKey, "PAID time-to-talk selected: set OPENAI_API_KEY (or OPENAI_API_KEY_OLD).");
  secrets = [apiKey];
  const binary = buildGateway();
  const nonce = crypto.randomUUID();
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "meta-voice-ttt-"));
  fs.chmodSync(directory, 0o700);
  const facts = makeFacts(nonce);
  const gateway = await launchGateway(binary, apiKey, facts, directory, {
    allowStaleBundle: process.env.META_VOICE_ALLOW_STALE_BUNDLE === "1",
    rustLog: process.env.META_VOICE_TTT_RUST_LOG || TTT_RUST_LOG,
  });
  const logPath = path.join(directory, "gateway.log");
  const serverLine = /console_voice::timing|context summary finished/;
  const { chromium } = require("playwright");
  const browser = await chromium.launch({ headless: true, env: childEnv() });
  const results = [];
  const pageErrors = [];
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const requests = [];
    let rpcSequence = 0;
    const rpc = async (method, params = {}) => {
      const response = await fetch(`${gateway.url}/console/rpc`, { method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: `ttt-${++rpcSequence}`, method, params }), signal: AbortSignal.timeout(15_000) });
      assert.equal(response.status, 200, `${method} HTTP ${response.status}`);
      const data = await response.json();
      assert.ok(!data.error, `${method}: ${redact(JSON.stringify(data.error))}`);
      return data.result;
    };
    page.on("request", request => {
      if (!request.url().endsWith("/console/rpc")) return;
      const data = request.postDataJSON();
      requests.push({ request, method: data.method, params: data.params ?? {}, started: Date.now() });
    });
    page.on("response", async response => {
      const row = requests.find(candidate => candidate.request === response.request());
      if (!row) return;
      try {
        const data = await response.json();
        Object.assign(row, { result: data.result, error: data.error, ended: Date.now() });
      } catch { row.unreadable = true; }
    });
    page.on("pageerror", error => pageErrors.push(redact(error.message)));
    await page.addInitScript(instrumentBrowser);
    await page.goto(`${gateway.url}/console`);
    await page.getByTestId(`chat-composer:${PRIMARY}`).waitFor({ timeout: 30_000 });
    const finals = async (scope = {}) => new Set(((await rpc("meta/console/query_timeline", { identity: PRIMARY, mode: "recent", limit: 1000 }))
      .frames ?? []).filter(frame => finalText(frame, { identity: PRIMARY, ...scope })).map(frame => frame.id));
    const seedStarted = Date.now();
    const turns = Math.max(1, seedTurns);
    for (let turn = 1; turn <= turns; turn++) {
      const before = await finals();
      const content = seedTurns < 1
        ? `Startup barrier ${nonce}. Use no tools. Reply exactly READY.`
        : `Seed turn ${turn} of ${turns}. Use no tools. Reply in about ${seedWords} words of plain prose that mention the number ${turn} and give facts about the word ${WORDS[turn % WORDS.length]}.`;
      const accepted = await rpc("meta/console/send", { identity: PRIMARY, content, origin: "voice-time-to-talk", idempotency_key: crypto.randomUUID() });
      await poll(`seed turn ${turn} final`, async () => [...await finals({ interaction_id: accepted.interaction_id })]
        .some(id => !before.has(id)), 120_000, 500);
    }
    const original = (await rpc("meta/console/inspect_identity", { identity: PRIMARY })).identity;
    // Let the last turn's checkpoint commit before reading the store.
    await sleep(1500);
    const size = sessionSize(directory, original.session_id);
    log("seeded", { turns: seedTurns, seedWords, elapsedMs: Date.now() - seedStarted, session: size });
    for (let run = 1; run <= runs; run++) {
      const logOffset = fs.statSync(logPath).size;
      const begin = requests.length;
      await page.evaluate(() => { window.voiceAcceptance.timeline.length = 0; window.voiceAcceptance.mark("click"); });
      const clickWall = Date.now();
      await page.getByRole("button", { name: `Start voice with ${LABEL}`, exact: true }).click({ timeout: 30_000 });
      await page.getByTestId("voice-bar").and(page.locator('[data-phase="active"]')).waitFor({ timeout: 60_000 });
      const activeWall = Date.now();
      // The primary status must tell the user they can talk as soon as the
      // microphone is open, independent of context preparation.
      await page.locator('[data-testid="voice-status"][data-talk-ready="true"]', { hasText: /you can talk/i }).waitFor({ timeout: 10_000 });
      const talkReadyWall = Date.now();
      await sleep(400);
      // The console polls context_status once a second while the concurrent
      // summary is prepared; hold the call so that phase can settle.
      const contextRows = () => requests.slice(begin).filter(row => row.method === "meta/console/voice/context_status" && row.ended);
      const preparation = row => row.result?.context_preparation ?? row.result?.preparation;
      const settled = row => ["provider_acknowledged", "failed"].includes(preparation(row)?.phase);
      if (holdMs > 0) await poll("context preparation settled", () => contextRows().some(settled), holdMs, 250).catch(() => {});
      const state = await browserState(page);
      const stages = summarizeActivation({ clickWall, activeWall, rows: requests.slice(begin), timeline: state.timeline });
      stages.talkReadyVisibleMs = talkReadyWall - clickWall;
      const transitions = [];
      for (const row of contextRows()) {
        const key = `${preparation(row)?.phase ?? "?"}${preparation(row)?.stage ? `:${preparation(row).stage}` : ""}`;
        if (transitions.at(-1)?.key !== key) transitions.push({ key, at: row.ended - clickWall });
      }
      const settledRow = contextRows().find(settled);
      stages.contextPreparation = { settledAt: settledRow ? settledRow.ended - clickWall : null,
        settledPhase: settledRow ? preparation(settledRow).phase : null, polls: contextRows().length,
        transitions: transitions.map(entry => `${entry.key}@${entry.at}`) };
      const closeStart = Date.now();
      await page.getByRole("button", { name: "End voice conversation", exact: true }).click();
      await poll("voice closed", () => requests.find(row => row.method === "meta/console/voice/close" &&
        row.started >= closeStart && row.result?.phase === "closed"), 30_000, 50);
      await poll("voice bar gone", async () => (await page.getByTestId("voice-bar").count()) === 0, 10_000, 50);
      stages.closeMs = Date.now() - closeStart;
      await sleep(1500);
      stages.server = fs.readFileSync(logPath, "utf8").slice(logOffset).split("\n")
        .filter(line => serverLine.test(line)).map(line => line.replace(/^\S+\s+/, ""));
      // The gateway's summary line: generation time, output size, cache reuse.
      // The public GPT Live transport appends the summary in UTF-8 fragments of
      // at most 500 bytes, one provider receipt each.
      const summaryLine = stages.server.find(line => line.includes("context summary finished"));
      const field = name => summaryLine?.match(new RegExp(`${name}=(?:Some\\()?([^\\s)]+)`))?.[1];
      const summaryBytes = Number(field("output_bytes"));
      stages.summary = summaryLine ? {
        ms: Number(field("elapsed_ms")), bytes: Number.isFinite(summaryBytes) ? summaryBytes : null,
        fragments: Number.isFinite(summaryBytes) ? Math.ceil(summaryBytes / 500) : null,
        cache: field("cache") ?? null, windowMessages: Number(field("window_messages")) || null,
        totalMessages: Number(field("total_messages")) || null, model: field("model") ?? null,
      } : null;
      results.push(stages);
      log("time-to-talk-run", { run, ...stages });
    }
    const pick = (label, select) => ({ stage: label, medianMs: median(results.map(select)), maxMs: maximum(results.map(select)) });
    const table = [
      pick("readiness rpc", row => row.readiness?.ms),
      pick("getUserMedia", row => row.getUserMedia.ms),
      pick("open rpc", row => row.open?.ms),
      pick("offer + setLocalDescription", row => row.offerLocal.ms),
      pick("playback_owner/register rpc", row => row.register?.ms),
      pick("live/webrtc/answer rpc", row => row.answer?.ms),
      pick("setRemoteDescription", row => row.setRemoteMs),
      pick("answer_received rpc", row => row.answerReceived?.ms),
      pick("answer_received end -> status active", row => row.answerReceivedToActiveMs),
      pick("status polls until active (count)", row => row.status?.count),
      pick("status poll rpc (median per run)", row => row.status?.medianPollMs),
      pick("peer connected (from click)", row => row.peerConnectedAt),
      pick("data channel open (from click)", row => row.dataChannelOpenAt),
      pick("status active (from click)", row => row.status?.activeAt),
      pick("mic enabled (from click)", row => row.micEnabledAt),
      pick("UI active (from click)", row => row.totalUiActiveMs),
      pick("'you can talk' visible (from click)", row => row.talkReadyVisibleMs),
      pick("context preparation settled (from click)", row => row.contextPreparation.settledAt),
      pick("summary generation (gateway)", row => row.summary?.ms),
      pick("summary output bytes", row => row.summary?.bytes),
      pick("summary fragments (500 B each)", row => row.summary?.fragments),
      pick("close (click -> closed)", row => row.closeMs),
    ];
    log("time-to-talk-summary", { runs, seedTurns, seedWords, holdMs, session: size, pageErrors, table });
    console.table(table);
  } finally {
    await browser.close();
    await gateway.stop();
    log("artifacts", { directory, contains: "redacted gateway log; no credentials" });
  }
}

async function main() {
  const args = process.argv.slice(2);
  if (args[0] === "--help") {
    assert.deepEqual(args, ["--help"]);
    process.stdout.write(HELP);
    return;
  }
  if (args[0] === "--self-test") {
    assert.deepEqual(args, ["--self-test"]);
    selfTest();
    await selfTestPeerGate();
    return;
  }
  if (args[0] === "--self-test-audio") {
    assert.deepEqual(args, ["--self-test-audio"]);
    await selfTestAudioClock();
    return;
  }
  if (args[0] === "--time-to-talk") {
    const options = { runs: 3, seedTurns: 40, seedWords: 40, holdMs: 0 };
    const names = { runs: "runs", "seed-turns": "seedTurns", "seed-words": "seedWords", "hold-ms": "holdMs" };
    for (const arg of args.slice(1)) {
      const match = /^--(runs|seed-turns|seed-words|hold-ms)=(\d+)$/.exec(arg);
      assert.ok(match, `Unsupported --time-to-talk option ${arg}; use --runs=N --seed-turns=N --seed-words=N --hold-ms=N`);
      options[names[match[1]]] = Number(match[2]);
    }
    await timeToTalk(options);
    return;
  }
  assert.deepEqual(args, [], "Supported options: --self-test, --self-test-audio, --time-to-talk, --help");
  const apiKey = process.env.OPENAI_API_KEY || process.env.OPENAI_API_KEY_OLD;
  assert.ok(apiKey, "PAID voice E2E selected: set OPENAI_API_KEY (or OPENAI_API_KEY_OLD). Missing credentials are a FAILURE, never a skip.");
  secrets = [apiKey];
  selfTest();
  const binary = buildGateway();
  const nonce = crypto.randomUUID();
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "meta-voice-live-"));
  fs.chmodSync(directory, 0o700);
  const facts = makeFacts(nonce);
  let gateway;
  let gate;
  let passed = false;
  try {
    gate = await startPeerGate(facts);
    gateway = await launchGateway(binary, apiKey, facts, directory);
    await runBrowser(gateway.url, facts, directory, gate);
    passed = true;
  } finally {
    try {
      await gate?.stop();
    } finally {
      await gateway?.stop();
    }
    // Own mkdtemp directory only; never touch the user's demo or another run.
    if (passed && process.env.META_VOICE_KEEP_ARTIFACTS !== "1") fs.rmSync(directory, { recursive: true });
    else log("artifacts", { directory, contains: "redacted gateway log, evidence and screenshot; no credentials" });
  }
}
if (require.main === module) main().catch(error => { console.error(redact(error.stack)); process.exitCode = 1; });
