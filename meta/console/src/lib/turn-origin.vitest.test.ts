import { afterEach, describe, expect, it } from "vitest";

import {
  forgetOwnSends,
  rememberOwnCallerScope,
  rememberOwnSend,
  turnSender,
  turnSenderLine,
} from "./turn-origin";

// The `user_input` frame a console send records: the gateway names its
// sender from the caller it authenticated (`payload.sender`), never from
// the request.
function sentBy(callerScope: string, displayName: string, key: string): Record<string, unknown> {
  return {
    origin: "console",
    origin_kind: "operator",
    idempotency_key: key,
    sender: {
      kind: "person",
      assurance: "person_authenticated",
      display_name: displayName,
      caller_scope: callerScope,
    },
  };
}

describe("own_message_is_you_in_second_tab", () => {
  afterEach(() => forgetOwnSends());

  it("names the caller's own message You in a tab that did not send it", () => {
    // A second tab: it sent nothing itself, and whoami names the caller
    // whose send the frame records.
    rememberOwnCallerScope("console-caller:v1:alice");
    const mine = sentBy("console-caller:v1:alice", "alice@example.test", "sent-in-another-tab");
    expect(turnSender(mine)).toBe("you");
    expect(turnSenderLine(mine, "Router")).toBe("You asked Router");
  });

  it("names another caller's message by its sender's display name", () => {
    rememberOwnCallerScope("console-caller:v1:alice");
    const theirs = sentBy("console-caller:v1:bob", "bob@example.test", "bobs-send");
    expect(turnSender(theirs)).toBe("named");
    expect(turnSenderLine(theirs, "Router")).toBe("bob@example.test asked Router");
  });

  it("keeps the earlier words for a frame that names no sender", () => {
    rememberOwnCallerScope("console-caller:v1:alice");
    const older = { origin: "console", origin_kind: "operator", idempotency_key: "older" };
    expect(turnSender(older)).toBe("operator");
  });
});

// An anonymous caller's send on an Open console. Every anonymous caller of
// an Open console shares one caller scope, which `meta/console/whoami` also
// reports to each of them.
const SHARED_ANONYMOUS_SCOPE = "console-caller:v1:anonymous";

function sentAnonymously(key: string): Record<string, unknown> {
  return {
    origin: "console",
    origin_kind: "operator",
    idempotency_key: key,
    sender: { kind: "anonymous", caller_scope: SHARED_ANONYMOUS_SCOPE },
  };
}

describe("anonymous_tabs_name_only_their_own_messages_you", () => {
  afterEach(() => forgetOwnSends());

  it("names this tab's own message You and the other anonymous tab's not", () => {
    rememberOwnCallerScope(SHARED_ANONYMOUS_SCOPE);
    rememberOwnSend("sent-in-tab-one");
    expect(turnSender(sentAnonymously("sent-in-tab-one"))).toBe("you");
    const fromTabTwo = sentAnonymously("sent-in-tab-two");
    expect(turnSender(fromTabTwo)).toBe("operator");
    expect(turnSenderLine(fromTabTwo, "Router")).toBe("An operator asked Router");
  });

  it("does the same from the other tab", () => {
    rememberOwnCallerScope(SHARED_ANONYMOUS_SCOPE);
    rememberOwnSend("sent-in-tab-two");
    expect(turnSender(sentAnonymously("sent-in-tab-two"))).toBe("you");
    expect(turnSender(sentAnonymously("sent-in-tab-one"))).toBe("operator");
  });

  it("keeps the caller scope rule for a signed-in caller", () => {
    rememberOwnCallerScope("console-caller:v1:alice");
    const mine = sentBy("console-caller:v1:alice", "alice@example.test", "sent-in-another-tab");
    expect(turnSender(mine)).toBe("you");
  });
});
