import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createAgentDirectory } from "../../../packages/console-components/src/panel-kit/agents";
import type { ConsoleFrame } from "../types";
import { LogsPanel } from "./LogsPanel";

afterEach(() => { cleanup(); });

const NOW = Date.now();
const at = (secondsAgo: number) => NOW - secondsAgo * 1000;
const directory = createAgentDirectory([
  { id: "billing", label: "Billing" },
  { id: "shipping", label: "Shipping" },
]);
const frames: ConsoleFrame[] = [
  { id: "ask", event: "user_input", identity: "billing", interactionId: "turn-a", timestampMs: at(300), data: { content: "Refund order 77310." }, sourceKind: "send" },
  // The saved copy of the same input: one row, not two.
  { id: "ask-saved", event: "user_input", identity: "billing", interactionId: "turn-a", timestampMs: at(299), data: { content: "Refund order 77310." }, sourceKind: "session_history" },
  { id: "done", event: "interaction_complete", identity: "billing", interactionId: "turn-a", timestampMs: at(290), data: { result: "Visible answer." } },
  { id: "timeout", event: "tool_execution_timed_out", identity: "shipping", interactionId: "turn-b", timestampMs: at(120), data: { name: "query_carrier", id: "call-1", timeout_ms: 4000 } },
  { id: "failed", event: "interaction_failed", identity: "shipping", interactionId: "turn-b", timestampMs: at(110), data: { error_report: { class: "llm", message: "The provider timed out." } } },
].reverse();

function rows(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>('[data-testid^="log-line:"]'))
    .filter((row) => !/:(toggle|detail|chat)$/.test(row.getAttribute("data-testid") ?? ""));
}

describe("Logs v2", () => {
  it("lists one row per event, with its level, agent, words and a severity strip", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const view = render(<LogsPanel frames={frames} agentDirectory={directory} />);
    expect(errors.mock.calls.filter((call) => /same key|key/.test(String(call[0]))).length).toBe(0);
    errors.mockRestore();
    const listed = rows(view.container);
    expect(listed.length).toBe(4);
    expect(listed.map((row) => row.className.match(/logline--(\w+)/)?.[1])).toEqual(["error", "warn", "info", "info"]);
    expect(listed[0].textContent).toContain("Shipping");
    expect(listed[0].textContent).toContain("Error");
    expect(view.getByTestId("logs-severity-strip")).toBeTruthy();
    expect(view.getByTestId("logs-strip-bin:0")).toBeTruthy();
    expect(view.getByTestId("logs-strip-bin:59")).toBeTruthy();
    expect(view.getByTestId("logs-summary").textContent).toMatch(/4 events.*1 warning.*1 error.*Live/);
  });

  it("keeps the key fields and the data in the expanded detail", () => {
    const view = render(<LogsPanel frames={frames} agentDirectory={directory} />);
    fireEvent.click(view.getByTestId("log-line:done:toggle"));
    const detail = view.getByTestId("log-line:done:detail");
    expect(detail.textContent).toContain("result=Visible answer.");
    expect(detail.textContent).toContain("interaction_complete");
  });

  it("filters by level, agent and a range picked from the strip", () => {
    const view = render(<LogsPanel frames={frames} agentDirectory={directory} />);
    fireEvent.click(view.getByTestId("logs-level:error"));
    expect(rows(view.container).map((row) => row.getAttribute("data-testid"))).toEqual(["log-line:failed"]);
    fireEvent.click(view.getByTestId("logs-level:all"));
    const agent = view.getByTestId("logs-agent-filter");
    expect(agent.tagName).toBe("SELECT");
    fireEvent.change(agent, { target: { value: "billing" } });
    expect(rows(view.container).length).toBe(2);
    fireEvent.change(agent, { target: { value: "" } });
    // Keyboard on the strip: Space picks the focused bin as the range.
    const strip = view.getByTestId("logs-severity-strip").querySelector("svg")!;
    fireEvent.focus(strip);
    fireEvent.keyDown(strip, { key: "Home" });
    fireEvent.keyDown(strip, { key: " " });
    expect(view.getByTestId("logs-range")).toBeTruthy();
    expect(rows(view.container).length).toBe(1);
    fireEvent.click(within(view.getByTestId("logs-range")).getByRole("button", { name: "Clear the time range" }));
    expect(view.queryByTestId("logs-range")).toBeNull();
    expect(rows(view.container).length).toBe(4);
  });

  it("groups a turn's events by interaction", () => {
    window.localStorage.setItem("meta-console-logs-group:default", "on");
    const view = render(<LogsPanel frames={frames} agentDirectory={directory} />);
    const group = view.getByTestId("logs-group:turn-b");
    expect(group.textContent).toContain("2 events");
    expect(rows(view.container).length).toBe(0);
    fireEvent.click(within(group).getByRole("button"));
    expect(rows(view.container).length).toBe(2);
    window.localStorage.removeItem("meta-console-logs-group:default");
  });
});
