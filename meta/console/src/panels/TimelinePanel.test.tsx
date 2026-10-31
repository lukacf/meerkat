import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TimelinePanel, describeTodayFrame } from "./TimelinePanel";
import { __chatPaneTest } from "./ChatPane";
import { formatClockTime } from "../../../packages/console-components/src/panel-kit/format";
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

describe("Today names memory events in plain words", () => {
  const memory = (event: string, data: Record<string, unknown>) => describeTodayFrame({
    id: event, event, identity: "identity:steward", timestampMs: Date.now(), data,
  });
  it("says a record moved, a verdict in words and a session that became untrusted", () => {
    const moved = memory("memory.record.promoted", { scope_kind: "mob", scope_key: "main" });
    expect(sentence({ id: "m", event: "memory.record.promoted", identity: "identity:steward", timestampMs: Date.now(), data: {} }))
      .toBe("identity:steward moved a memory record");
    expect(moved.tone).toBe("ok");
    expect(memory("memory.quarantine.verdict", { verdict: "tombstone" }).snippet).toBe("discarded");
    expect(memory("memory.quarantine.verdict", { verdict: "promote" }).snippet).toBe("moved to team memory");
    // The steward's open-loop nudge rides the verdict event, but nobody reviewed anything.
    expect(sentence({ id: "n", event: "memory.quarantine.verdict", identity: "identity:steward", timestampMs: Date.now(), data: { verdict: "open_loop_escalated", record_id: "rec-1" } }))
      .toBe("An open loop needs follow-up");
    expect(memory("memory.quarantine.verdict", { verdict: "open_loop_escalated" }).snippet === undefined).toBe(true);
    expect(sentence({ id: "t", event: "memory.taint.transition", identity: "identity:steward", timestampMs: Date.now(), data: { kind: "tainted" } }))
      .toBe("identity:steward's session became untrusted");
  });
});

describe("Today rows", () => {
  it("renders a frame that arrives twice without colliding React keys", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const frame: ConsoleFrame = { id: "twice", event: "interaction_complete", identity: "identity:coordinator", interactionId: "turn-1", timestampMs: Date.now(), data: { result: "Done." } };
    const view = render(<TimelinePanel frames={[frame, { ...frame }]} />);
    const collisions = errors.mock.calls.filter((call) => /same key/.test(String(call[0])));
    errors.mockRestore();
    expect(collisions.length).toBe(0);
    expect(view.container.querySelectorAll(".today__row").length).toBe(2);
  });

  it("shows each event's time on the chat's 24-hour clock, in a 12-hour locale too", () => {
    // A browser whose locale reads clock times in 12 hours (en-US).
    const localeTime = Date.prototype.toLocaleTimeString;
    vi.spyOn(Date.prototype, "toLocaleTimeString").mockImplementation(function (
      this: Date,
      locales?: Intl.LocalesArgument,
      options?: Intl.DateTimeFormatOptions,
    ) {
      return localeTime.call(this, locales ?? "en-US", options);
    });
    const todayAt = (hours: number, minutes: number) => new Date().setHours(hours, minutes, 0, 0);
    const instants = [todayAt(14, 52), todayAt(9, 5)];
    const frames: ConsoleFrame[] = instants.map((timestampMs, index) => ({
      id: `clock-${index}`, event: "member_ready", identity: "identity:coordinator", timestampMs, data: {},
    }));
    // The time the chat shows on a message sent at the same instant.
    const chatTime = (timestampMs: number) => __chatPaneTest.buildChatMessages([{
      id: `chat-${timestampMs}`,
      kind: "message",
      variant: "plain",
      identity: { id: "agent", label: "Agent", role: "assistant" },
      createdAt: new Date(timestampMs).toISOString(),
      text: "Ready.",
    }])[0]?.time;

    const view = render(<TimelinePanel frames={frames} />);
    const shown = Array.from(view.container.querySelectorAll(".today__time"), (time) => time.textContent);
    expect(shown).toEqual(["14:52", "09:05"]);
    expect(shown).toEqual(instants.map(chatTime));
    expect(shown).toEqual(instants.map((timestampMs) => formatClockTime(timestampMs)));
  });
});
