import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, test } from "vitest";

import { WorkGraphCard, __workGraphCardUiState } from "@console-components";
import type { ConversationWorkGraphEntry } from "@console-core";

import { mapFramesToTimelineEntries, workGraphFailureNote } from "./adapters";

// The refusal the tool gate answers a disallowed WorkGraph call with.
const ACCESS_DENIED = "{\"error\":\"access_denied\",\"message\":\"Tool 'workgraph_ready' is not allowed by policy\"}";
const UNKNOWN_KIND = "{\"error\":\"lease_exhausted\",\"message\":\"no lease left for this owner\"}";

const AGENT = { agent_id: "planner", member_id: "planner", label: "Planner", kind: "identity" };

function failedCallFrames(name: string, result: string) {
  return [
    {
      id: "wg-refused-call",
      event: "tool_call_requested",
      interactionId: "turn-refused",
      timestampMs: 1_779_405_464_000,
      data: { id: "wg-refused-tc", name, args: {} },
    },
    {
      id: "wg-refused-done",
      event: "tool_execution_completed",
      interactionId: "turn-refused",
      timestampMs: 1_779_405_464_200,
      data: { id: "wg-refused-tc", name, is_error: true, result },
    },
  ];
}

function cardFor(name: string, result: string): ConversationWorkGraphEntry {
  const cards = mapFramesToTimelineEntries(AGENT, failedCallFrames(name, result))
    .filter((entry): entry is ConversationWorkGraphEntry => entry.kind === "workgraph");
  expect(cards).toHaveLength(1);
  return cards[0];
}

describe("WorkGraph failure notes", () => {
  beforeEach(() => {
    __workGraphCardUiState.reset();
  });

  test("an access_denied refusal reads as the action and the kind in words", () => {
    expect(workGraphFailureNote("workgraph_ready", ACCESS_DENIED)).toEqual({
      text: "Couldn't list ready work items: access denied",
      technical: ACCESS_DENIED,
    });
    // An operator action names its method in the same vocabulary.
    expect(workGraphFailureNote("workgraph_claim", { code: "conflict", message: "stale revision 9" }).text)
      .toBe("Couldn't claim a work item: conflict with another change");
  });

  test("an unknown kind falls back to the action alone, never to nothing", () => {
    expect(workGraphFailureNote("workgraph_claim", UNKNOWN_KIND)).toEqual({
      text: "Couldn't claim a work item",
      technical: UNKNOWN_KIND,
    });
    expect(workGraphFailureNote("workgraph_claim", "{\"detail\":\"no kind here\"}").text).toBe("Couldn't claim a work item");
    expect(workGraphFailureNote("workgraph_close", undefined)).toEqual({ text: "Couldn't close a work item" });
    expect(workGraphFailureNote("workgraph_ensure_items", ACCESS_DENIED).text)
      .toBe("Couldn't run WorkGraph ensure items: access denied");
    expect(workGraphFailureNote("workgraph", undefined).text).toBe("Couldn't complete the WorkGraph action");
  });

  test("a plain text message follows the action, cut to a line with the full text kept", () => {
    expect(workGraphFailureNote("workgraph_claim", "revision conflict: expected 9, found 1")).toEqual({
      text: "Couldn't claim a work item: revision conflict: expected 9, found 1",
    });
    const long = `revision conflict: ${"x".repeat(120)}`;
    const note = workGraphFailureNote("workgraph_claim", long);
    expect(note.text.length).toBeLessThanOrEqual("Couldn't claim a work item: ".length + 80);
    expect(note.technical).toBe(long);
  });

  test("the card's line is the sentence and the raw JSON sits behind the technical details", () => {
    const entry = cardFor("workgraph_ready", ACCESS_DENIED);
    expect(entry.lastActionFailed).toBe(true);
    expect(entry.recentEvents).toEqual(["Couldn't list ready work items: access denied"]);
    expect(entry.recentEventDetails).toEqual([
      { failed: true, tool: "workgraph_ready", technical: ACCESS_DENIED },
    ]);

    render(<WorkGraphCard entry={entry} />);
    const lineId = `workgraph-card:${entry.rootId}:event:0`;
    const line = screen.getByTestId(lineId);
    expect(line).toHaveTextContent(/^Couldn't list ready work items: access denied$/);
    expect(line.textContent).not.toContain("{");
    expect(line.querySelector(".cc-mark[data-tone='crit']")).not.toBeNull();

    const details = screen.getByTestId(`${lineId}:details`);
    expect(details.tagName).toBe("DETAILS");
    expect(details).not.toHaveAttribute("open");
    expect(within(details).getByText("Technical details")).toBeInTheDocument();
    expect(screen.getByTestId(`${lineId}:response`)).toHaveTextContent(ACCESS_DENIED);

    const chip = screen.getByTestId(`workgraph-card:${entry.rootId}:last-action-failed`);
    expect(chip).toHaveTextContent("Failed");
  });

  test("an unknown kind still shows a sentence with the raw JSON behind the details", () => {
    const entry = cardFor("workgraph_claim", UNKNOWN_KIND);
    expect(entry.recentEvents).toEqual(["Couldn't claim a work item"]);

    render(<WorkGraphCard entry={entry} />);
    const lineId = `workgraph-card:${entry.rootId}:event:0`;
    expect(screen.getByTestId(lineId)).toHaveTextContent(/^Couldn't claim a work item$/);
    expect(screen.getByTestId(`${lineId}:response`)).toHaveTextContent(UNKNOWN_KIND);
  });
});
