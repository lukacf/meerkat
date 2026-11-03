import { describe, expect, it } from "vitest";
import { beginConsoleSendAttempt, createConsoleSendAttempt, finishConsoleSendAttempt, recoverConsoleSendAttempt, consoleSendFailureState } from "./send-attempt";

const draft = () => createConsoleSendAttempt({ id: "a", scope: "authority/realm/principal", destination: "agent", origin: "console:pane-a", idempotencyKey: "key", text: "  hi\r\n🌳  ", now: 1 });
describe("durable send attempts", () => {
  it("freezes exact content, original pane, scope and key before dispatch", () => {
    const attempted = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 2, handlingMode: "queue" });
    expect(JSON.parse(attempted.envelopeJson!)).toEqual({ identity: "agent", content: "  hi\r\n🌳  ", origin: "console:pane-a", origin_kind: "operator", idempotency_key: "key", handling_mode: "queue" });
    const rejected = finishConsoleSendAttempt(attempted, { state: "definitely-rejected", error: "invalid" });
    expect(beginConsoleSendAttempt(rejected, { owner: "tab2", now: 3, handlingMode: "queue", retryRejected: true }).envelopeJson).toBe(attempted.envelopeJson);
  });
  it("does not replay a possible accepted write after lease expiry or reload", () => {
    const attempted = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 2, handlingMode: "queue" });
    const reloaded = recoverConsoleSendAttempt(JSON.parse(JSON.stringify(attempted)), 20_000);
    expect(reloaded.state).toBe("outcome-unknown");
    expect(reloaded.envelopeJson).toBe(attempted.envelopeJson);
    expect(() => beginConsoleSendAttempt(reloaded, { owner: "tab2", now: 21_000, handlingMode: "queue" })).toThrow(/Reconcile/);
    expect(() => beginConsoleSendAttempt(reloaded, { owner: "tab2", now: 21_000, handlingMode: "steer" })).toThrow(/Reconcile/);
  });
  it("sends an uncertain attempt again only on an explicit resend, with the same frozen envelope and key", () => {
    const attempted = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 2, handlingMode: "queue" });
    const unknown = finishConsoleSendAttempt(attempted, { state: "outcome-unknown", error: "lost" });
    expect(() => beginConsoleSendAttempt(unknown, { owner: "tab2", now: 3, handlingMode: "queue", retryRejected: true })).toThrow(/Reconcile/);
    const resent = beginConsoleSendAttempt(unknown, { owner: "tab2", now: 3, handlingMode: "queue", resend: true });
    expect(resent.state).toBe("attempting");
    expect(resent.envelopeJson).toBe(attempted.envelopeJson);
    expect(JSON.parse(resent.envelopeJson!).idempotency_key).toBe("key");
  });
  it("prevents promotion of an already attempted queue item", () => {
    const attempt = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 2, handlingMode: "queue" });
    const rejected = finishConsoleSendAttempt(attempt, { state: "definitely-rejected", error: "denied" });
    expect(() => beginConsoleSendAttempt(rejected, { owner: "tab2", now: 3, handlingMode: "steer", retryRejected: true })).toThrow(/handling mode/);
  });
  it("retains canonical acceptance and rejects empty acceptance", () => {
    const attempt = beginConsoleSendAttempt(draft(), { owner: "t", now: 2, handlingMode: "steer" });
    expect(finishConsoleSendAttempt(attempt, { state: "accepted", interactionId: "i", inputFrameId: "f" }).accepted).toEqual({ interactionId: "i", inputFrameId: "f" });
    expect(() => finishConsoleSendAttempt(attempt, { state: "accepted", interactionId: "" })).toThrow(/prove acceptance/);
  });
  it("treats network loss, in-flight and idempotency conflicts conservatively", () => {
    for (const error of [new Error("lost response"), { rpcError: { code: -32009, data: { kind: "idempotency_conflict" } } }, { rpcError: { code: -32001 } }]) {
      expect(consoleSendFailureState(error)).toBe("outcome-unknown");
    }
    expect(consoleSendFailureState({ rpcError: { code: -32602 } })).toBe("definitely-rejected");
  });
});

import { reconcileConsoleSendReceipt } from "./send-attempt";
it("reconciles only owner receipts matching exact envelope and destination", () => {
  const attempted = beginConsoleSendAttempt(draft(), { owner: "tab1", now: 2, handlingMode: "queue" });
  const unknown = finishConsoleSendAttempt(attempted, { state: "outcome-unknown", error: "timeout" });
  const frame = { id: "frame", event: "user_input", identity: "agent", interactionId: "interaction", data: JSON.parse(attempted.envelopeJson!) };
  expect(reconcileConsoleSendReceipt(unknown, frame)?.accepted?.inputFrameId).toBe("frame");
  expect(reconcileConsoleSendReceipt(unknown, { ...frame, identity: "other" })).toBeNull();
  expect(reconcileConsoleSendReceipt(unknown, { ...frame, data: { ...frame.data, origin: "console:different-pane" } })).toBeNull();
  expect(reconcileConsoleSendReceipt(unknown, { ...frame, data: { ...frame.data, content: "edited" } })).toBeNull();
});

it("reconciles text-block receipts across JSON key ordering without changing any text bytes", () => {
  const context = { version: 1 as const, id: "q", sourceScope: "source", sourceIdentity: "reader", messageId: "m", quote: "  A\u030a\r\n🚀  ", label: "Source" };
  const attempted = beginConsoleSendAttempt({ ...draft(), contexts: [context] }, { owner: "tab", now: 2, handlingMode: "queue" });
  const envelope = JSON.parse(attempted.envelopeJson!);
  const content = envelope.content.map((block: { type: string; text: string }) => ({ text: block.text, type: block.type }));
  const frame = { id: "frame", event: "user_input", identity: "agent", interactionId: "interaction", data: { ...envelope, content } };
  expect(reconcileConsoleSendReceipt(attempted, frame)?.accepted?.inputFrameId).toBe("frame");
  expect(reconcileConsoleSendReceipt(attempted, { ...frame, data: { ...frame.data, content: [...content].reverse() } })).toBeNull();
  expect(reconcileConsoleSendReceipt(attempted, { ...frame, data: { ...frame.data, content: content.map((block: { type: string; text: string }) => ({ ...block, text: block.text.trim() })) } })).toBeNull();
});

import { validateConsoleSendAttempt } from "./send-attempt";
it("rejects malformed identifiers, leases and acceptance receipts", () => {
  const attempted = beginConsoleSendAttempt(draft(), { owner: "tab", now: 2, handlingMode: "queue" });
  const accepted = finishConsoleSendAttempt(attempted, { state: "accepted", interactionId: "owner-receipt" });
  for (const malformed of [
    { ...draft(), id: 123 }, { ...draft(), origin: " " }, { ...draft(), idempotencyKey: {} },
    { ...attempted, lease: { owner: "tab", expiresAt: "never" } },
    { ...attempted, lease: { owner: "", expiresAt: 100 } }, { ...attempted, lease: undefined },
    { ...accepted, accepted: undefined }, { ...accepted, accepted: { interactionId: " " } },
    { ...accepted, accepted: { interactionId: "i", inputFrameId: 4 } },
    { ...attempted, accepted: accepted.accepted },
  ]) expect(() => validateConsoleSendAttempt(malformed as never)).toThrow();
  for (const interactionId of [123, " ", null]) {
    expect(() => finishConsoleSendAttempt(attempted, { state: "accepted", interactionId } as never)).toThrow(/prove acceptance/);
  }
});
it("keeps authoritative acceptance terminal across late failures", () => {
  const attempted = beginConsoleSendAttempt(draft(), { owner: "tab", now: 2, handlingMode: "queue" });
  const accepted = finishConsoleSendAttempt(attempted, { state: "accepted", interactionId: "owner-receipt", inputFrameId: "frame" });
  expect(finishConsoleSendAttempt(accepted, { state: "outcome-unknown", error: "late timeout" })).toBe(accepted);
  expect(finishConsoleSendAttempt(accepted, { state: "definitely-rejected", error: "late refusal" })).toBe(accepted);
});

it("reconciles a canonical alias receipt only with exact owner-proven correspondence", () => {
  const attempt = beginConsoleSendAttempt(draft(), { owner: "tab", now: 2, handlingMode: "queue" });
  const frame = { id: "canonical-input", event: "user_input", identity: "mob/canonical-agent", interactionId: "alias-turn", data: JSON.parse(attempt.envelopeJson!) };
  const resolution = { requestedIdentity: attempt.destination, canonicalIdentity: frame.identity };
  expect(reconcileConsoleSendReceipt(attempt, frame)).toBeNull();
  expect(reconcileConsoleSendReceipt(attempt, frame, { ...resolution, requestedIdentity: "wrong-alias" })).toBeNull();
  expect(reconcileConsoleSendReceipt(attempt, frame, { ...resolution, canonicalIdentity: "different-canonical" })).toBeNull();
  expect(reconcileConsoleSendReceipt(attempt, frame, resolution)?.accepted?.inputFrameId).toBe("canonical-input");
  expect(reconcileConsoleSendReceipt(attempt, { ...frame, data: { ...frame.data, idempotency_key: "other" } }, resolution)).toBeNull();
  expect(reconcileConsoleSendReceipt(attempt, { ...frame, identity: "other" }, resolution)).toBeNull();
});

import { classifyConsoleSendFailure, consoleSendFailureLabel, describeConsoleAcceptanceCheckFailure, CONSOLE_ACCEPTANCE_NO_RECEIPT } from "./send-attempt";
describe("typed send failures", () => {
  it.each([
    [{ httpStatus: 401 }, "definitely-rejected", "unauthenticated", "Not authorized"],
    [{ httpStatus: 401, responseRpcError: { code: -32600, data: { kind: "unauthenticated" } } }, "definitely-rejected", "unauthenticated", "Not authorized"],
    [{ httpStatus: 403 }, "definitely-rejected", "access_denied", "Not allowed"],
    [{ rpcError: { code: -32030, message: "access denied: agent.send", data: { kind: "access_denied" } } }, "definitely-rejected", "access_denied", "Not allowed"],
    [{ rpcError: { code: -32000, data: { kind: "read_only" } } }, "definitely-rejected", "read_only", "Console read-only"],
    [{ rpcError: { code: -32602, message: "content must be non-empty" } }, "definitely-rejected", "rejected", "Rejected"],
    [{ rpcError: { code: -32001, message: "unknown identity domain:nope" } }, "outcome-unknown", "refused", "Send failed"],
    [{ httpStatus: 429 }, "outcome-unknown", "rate_limited", "Gateway busy"],
    [{ httpStatus: 413 }, "definitely-rejected", "rejected", "Rejected"],
    [Object.assign(new Error("console rpc timeout after 60 s"), { transportFailure: "timeout", timeoutMs: 60_000 }), "outcome-unknown", "timeout", "No response"],
    [Object.assign(new TypeError("Failed to fetch"), { transportFailure: "connection_failed" }), "outcome-unknown", "connection_failed", "Acceptance unknown"],
    [Object.assign(new Error("non-JSON"), { transportFailure: "invalid_response", httpStatus: 200 }), "outcome-unknown", "invalid_response", "Unreadable response"],
    [{ httpStatus: 502 }, "outcome-unknown", "gateway_error", "Gateway error"],
    [Object.assign(new Error("host human input refused"), { rpcError: { code: -32000, message: "host human input refused; inspect the typed reason before retrying" } }), "outcome-unknown", "refused", "Send failed"],
    [{ rpcError: { code: -32009, message: "idempotency key conflict: k", data: { kind: "idempotency_conflict" } } }, "outcome-unknown", "refused", "Send failed"],
    [new Error("lost response"), "outcome-unknown", "unknown", "Acceptance unknown"],
    [Object.assign(new Error("Console authority lifetime ended"), { name: "AbortError" }), "outcome-unknown", "interrupted", "Interrupted"],
  ] as const)("classifies %o", (error, state, kind, label) => {
    const failure = classifyConsoleSendFailure(error);
    expect(failure.state).toBe(state);
    expect(failure.kind).toBe(kind);
    expect(failure.message.trim().length).toBeGreaterThan(0);
    expect(consoleSendFailureState(error)).toBe(state);
    expect(consoleSendFailureLabel({ state: failure.state, failureKind: failure.kind })).toBe(label);
  });
  it("never classifies from message text", () => {
    expect(classifyConsoleSendFailure(new Error("request failed 401 unauthorized access_denied")).kind).toBe("unknown");
  });
  it("records the typed kind on the settled attempt and clears it on retry", () => {
    const attempted = beginConsoleSendAttempt(draft(), { owner: "tab", now: 2, handlingMode: "queue" });
    const failure = classifyConsoleSendFailure({ httpStatus: 401 });
    const rejected = finishConsoleSendAttempt(attempted, { state: failure.state, error: failure.message, kind: failure.kind });
    expect(rejected).toMatchObject({ state: "definitely-rejected", failureKind: "unauthenticated", error: failure.message });
    expect(() => validateConsoleSendAttempt(JSON.parse(JSON.stringify(rejected)))).not.toThrow();
    const retried = beginConsoleSendAttempt(rejected, { owner: "tab", now: 3, handlingMode: "queue", retryRejected: true });
    expect(retried.failureKind).toBeUndefined();
    expect(retried.error).toBeUndefined();
    expect(() => validateConsoleSendAttempt({ ...attempted, failureKind: "unauthenticated" })).toThrow();
    // A newer tab's kind does not block the saved queue.
    expect(() => validateConsoleSendAttempt({ ...rejected, failureKind: "future_kind" as never })).not.toThrow();
    expect(consoleSendFailureLabel({ state: "definitely-rejected", failureKind: "future_kind" as never })).toBe("Not accepted");
  });
  it("names a failed acceptance check without claiming anything about the send", () => {
    expect(describeConsoleAcceptanceCheckFailure({ httpStatus: 401 })).toEqual({
      kind: "unauthenticated",
      message: "Could not check acceptance: authentication required (401). The saved message is unchanged and was not resent.",
    });
    expect(describeConsoleAcceptanceCheckFailure(Object.assign(new TypeError("Failed to fetch"), { transportFailure: "connection_failed" })).message)
      .toBe("Could not check acceptance: the connection failed before the gateway answered. The saved message is unchanged and was not resent.");
    expect(describeConsoleAcceptanceCheckFailure(Object.assign(new Error("boom"), { httpStatus: 500 })).message)
      .toBe("Could not check acceptance (HTTP 500): boom. The saved message is unchanged and was not resent.");
    expect(CONSOLE_ACCEPTANCE_NO_RECEIPT.kind).toBe("no_receipt");
  });
});
