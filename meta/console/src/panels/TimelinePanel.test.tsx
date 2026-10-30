import { describe, expect, it } from "vitest";

import { describeTodayFrame } from "./TimelinePanel";
import { rememberOwnSend } from "../lib/turn-origin";
import type { ConsoleFrame } from "../types";

// The sentence with each agent segment shown as its key.
function sentence(frame: ConsoleFrame): string {
  return describeTodayFrame(frame)
    .segments.map((segment) => (typeof segment === "string" ? segment : segment.agent ?? "System"))
    .join("");
}

function userInput(id: string, data: Record<string, unknown>): ConsoleFrame {
  return { id, event: "user_input", identity: "identity:nightly-monitor", interactionId: `turn-${id}`, timestampMs: Date.now(), data };
}

describe("Today names who sent a turn", () => {
  it("says a scheduled run, a connector, another operator or you, from the frame's origin", () => {
    rememberOwnSend("today-key-mine");
    expect(sentence(userInput("a", { content: "Nightly check", origin: "console:schedule", origin_kind: "scheduler" })))
      .toBe("Scheduled run for identity:nightly-monitor");
    expect(sentence(userInput("b", { content: "Customer email", origin: "mail-bridge", origin_kind: "connector" })))
      .toBe("A connector sent identity:nightly-monitor a message");
    expect(sentence(userInput("c", { content: "Backlog", origin: "console:pane-1", origin_kind: "operator", idempotency_key: "someone-else" })))
      .toBe("An operator asked identity:nightly-monitor");
    expect(sentence(userInput("d", { content: "Summary", origin: "console:pane-2", origin_kind: "operator", idempotency_key: "today-key-mine" })))
      .toBe("You asked identity:nightly-monitor");
    expect(sentence(userInput("e", { content: "Probe", origin: "release-gate", origin_kind: "operator_probe" })))
      .toBe("An operator check asked identity:nightly-monitor");
  });
});
