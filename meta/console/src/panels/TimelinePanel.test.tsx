import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { TimelinePanel, describeTodayFrame } from "./TimelinePanel";
import { __chatPaneTest } from "./ChatPane";
import { formatClockTime } from "../../../packages/console-components/src/panel-kit/format";
import { rememberOwnSend } from "../lib/turn-origin";
import { insertConsoleFrame } from "../../../packages/console-core/src/timeline-recovery";
import { ActivityLedgerProvider, createActivityLedger } from "../lib/activity-ledger";
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
  it("renders a frame that arrives twice once, without colliding React keys", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const frame: ConsoleFrame = { id: "twice", event: "interaction_complete", identity: "identity:coordinator", interactionId: "turn-1", timestampMs: Date.now(), data: { result: "Done." } };
    // The console's activity buffer keeps one copy of a frame however often it arrives.
    const frames = [frame, { ...frame }].reduce<ConsoleFrame[]>((held, copy) => insertConsoleFrame(held, copy, 200) ?? held, []);
    const view = render(<TimelinePanel frames={frames} />);
    const collisions = errors.mock.calls.filter((call) => /same key/.test(String(call[0])));
    errors.mockRestore();
    expect(collisions.length).toBe(0);
    expect(view.container.querySelectorAll(".today__row").length).toBe(1);
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

describe("Today v2", () => {
  const NOW = Date.now();
  const at = (minutesAgo: number) => NOW - minutesAgo * 60_000;
  const frames: ConsoleFrame[] = [
    { id: "ask", event: "user_input", identity: "identity:billing", interactionId: "turn-a", timestampMs: at(20), data: { content: "Refund order 77310.", origin: "console:pane-1", origin_kind: "operator" } },
    { id: "start", event: "run_started", identity: "identity:billing", interactionId: "turn-a", runId: "run-a", timestampMs: at(20), data: { run_id: "run-a" } },
    { id: "done", event: "interaction_complete", identity: "identity:billing", interactionId: "turn-a", runId: "run-a", timestampMs: at(18), data: { result: "Refunded." } },
    { id: "start-b", event: "run_started", identity: "identity:shipping", interactionId: "turn-b", runId: "run-b", timestampMs: at(12), data: { run_id: "run-b" } },
    { id: "failed", event: "interaction_failed", identity: "identity:shipping", interactionId: "turn-b", runId: "run-b", timestampMs: at(11), data: { error_report: { class: "llm", message: "The provider timed out." } } },
    { id: "ready", event: "member_ready", identity: "identity:returns", timestampMs: at(5), data: {} },
  ];
  function renderToday() {
    const ledger = createActivityLedger({ notifyMs: 0 });
    ledger.ingestMany(frames);
    return render(
      <ActivityLedgerProvider ledger={ledger}>
        <TimelinePanel frames={[...frames].reverse()} />
      </ActivityLedgerProvider>,
    );
  }

  it("draws the day's lanes: an agent per lane, its turns as spans the ledger paired", () => {
    const view = renderToday();
    expect(view.getByTestId("timeline-swimlanes")).toBeTruthy();
    expect(view.getByTestId("timeline-lane:identity:billing")).toBeTruthy();
    expect(view.getByTestId("timeline-span:run-a").getAttribute("data-state")).toBe("done");
    expect(view.getByTestId("timeline-span:run-b").getAttribute("data-state")).toBe("failed");
    // The failure lines up on the problems lane.
    expect(view.getByTestId("timeline-lane:problems")).toBeTruthy();
  });

  it("tells the day as rows, a turn as one, and filters to problems", () => {
    const view = renderToday();
    const rows = () => Array.from(view.container.querySelectorAll<HTMLElement>('[data-testid^="timeline-story-row:"]'));
    expect(rows().length).toBe(3);
    const turn = view.getByTestId("timeline-story-row:turn:identity:billing:turn-a");
    expect(turn.textContent).toContain("An operator asked Billing");
    expect(turn.textContent).toContain("replied");
    fireEvent.click(view.getByTestId("timeline-filter:problems"));
    expect(rows().length).toBe(1);
    expect(rows()[0].getAttribute("data-severity")).toBe("crit");
    expect(rows()[0].textContent).toContain("turn failed");
    // A row opens its detail in the drawer.
    fireEvent.click(view.getByTestId(`timeline-story-open:${rows()[0].getAttribute("data-testid")!.slice("timeline-story-row:".length)}`));
    expect(view.getByTestId("timeline-drawer").textContent).toContain("Failed");
  });

  it("names the figures of the day in the glance strip", () => {
    const view = renderToday();
    expect(view.getByTestId("timeline-glance:turns").textContent).toContain("2");
    const problems = view.getByTestId("timeline-glance:problems");
    expect(problems.textContent).toContain("1");
    fireEvent.click(problems);
    expect(view.getByTestId("timeline-filter:problems").getAttribute("aria-pressed")).toBe("true");
  });
});
