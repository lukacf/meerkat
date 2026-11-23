import React from "react";
import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { WorkGraphCard } from "../../../packages/console-components/src/conversation/work-graph-card";
import { WorkGraphGraphView } from "../panels/WorkGraphGraphView";
import type { ConsoleFrame, WorkGraphWireItem } from "../types";
import { buildWorkGraphOperatorResultFrame, mapFramesToTimelineEntries } from "./adapters";

const AGENT = { agent_id: "planner", label: "Planner" };
const AVAILABLE = "2026-07-08T09:00:00Z";

function item(finishBy?: string): WorkGraphWireItem {
  return {
    id: "finish-target",
    title: "A target with independent availability",
    status: "open",
    revision: 1,
    created_at: AVAILABLE,
    updated_at: AVAILABLE,
    due_at: AVAILABLE,
    ...(finishBy ? { finish_by: finishBy } : {}),
  };
}

function responseFrames(value: WorkGraphWireItem): ConsoleFrame[] {
  // Frame IDs identify immutable bytes in the result cache, including across
  // independent renders. The past/future examples must have distinct IDs.
  const prefix = `finish-${value.finish_by ?? "unset"}-${value.revision}`;
  return [
    {
      id: `${prefix}-call`,
      event: "tool_call_requested",
      interactionId: "finish-turn",
      timestampMs: 1_779_405_464_000,
      data: { id: "finish-tool", name: "workgraph_create", args: { title: value.title } },
    },
    {
      id: `${prefix}-result`,
      event: "tool_execution_completed",
      interactionId: "finish-turn",
      timestampMs: 1_779_405_464_200,
      data: { id: "finish-tool", name: "workgraph_create", result: JSON.stringify({ item: value }) },
    },
  ];
}

function cardFrom(frames: ConsoleFrame[]) {
  const card = mapFramesToTimelineEntries(AGENT, frames).find((entry) => entry.kind === "workgraph");
  if (!card || card.kind !== "workgraph") throw new Error("Expected the response's WorkGraph card");
  return card;
}

describe("finish-by response display", () => {
  beforeEach(() => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  });

  it.each(["2026-07-01T12:00:00Z", "2030-01-02T12:00:00Z"])(
    "carries target %s from a response through the adapter and card, then removes a cleared target",
    (target) => {
      const frames = responseFrames(item(target));
      const card = cardFrom(frames);
      expect(card.items[0].finishBy).toBe(target);
      expect(card.items[0].dueAt).toBe(AVAILABLE);
      expect(card.items[0].status).toBe("open");
      const view = render(<WorkGraphCard entry={card} />);
      expect(view.getByText(`Finish by ${target.slice(0, 10)}`).getAttribute("title"))
        .toBe("Target day to finish. Nothing is held back when it passes.");
      expect(view.getByText("From 2026-07-08").getAttribute("title"))
        .toBe("Available from this day");

      // Current readers omit an unset target. A later successful clear must
      // remove the chip, including when a stale response subsequently arrives.
      const clear = buildWorkGraphOperatorResultFrame({
        method: "meta/workgraph/update",
        params: { id: "finish-target", expected_revision: 1, finish_by: { op: "clear" } },
        result: { item: { ...item(), revision: 2 } },
        identity: "planner",
        timestampMs: 1_779_405_465_000,
        frameId: "finish-clear",
      });
      const cleared = cardFrom([...frames, clear]);
      expect(cleared.items[0].finishBy).toBeNull();
      view.rerender(<WorkGraphCard entry={cleared} />);
      expect(view.queryByText(/^Finish by /)).toBeNull();
      expect(view.getByText("From 2026-07-08")).toBeTruthy();
      expect(cardFrom([...frames, clear, ...responseFrames(item(target))]).items[0].finishBy).toBeNull();
    },
  );

  it("labels both dates in the graph drawer and omits the target for an older item", () => {
    const target = "2030-01-02T12:00:00Z";
    const view = render(<WorkGraphGraphView items={[item(target)]} edges={[]} attention={[]} selectedId="finish-target" />);
    const drawer = view.getByTestId("workgraph-graph-detail");
    expect(drawer.textContent).toContain("Available from");
    expect(drawer.textContent).toContain("Finish by");
    view.rerender(<WorkGraphGraphView items={[item()]} edges={[]} attention={[]} selectedId="finish-target" />);
    expect(view.getByTestId("workgraph-graph-detail").textContent).toContain("Available from");
    expect(view.getByTestId("workgraph-graph-detail").textContent).not.toContain("Finish by");
  });
});
