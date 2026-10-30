import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkGraphWireEdge, WorkGraphWireItem } from "../types";
import { WorkGraphGraphView } from "./WorkGraphGraphView";

// DOM assertions compare booleans and strings, never elements.

function item(id: string, createdAt: string, overrides: Partial<WorkGraphWireItem> = {}): WorkGraphWireItem {
  return { id, title: id, status: "open", created_at: createdAt, updated_at: createdAt, ...overrides } as WorkGraphWireItem;
}

// A root and three children in the next column.
const items = [
  item("root", "2026-07-08T08:00:00Z", { title: "Ship it" }),
  item("child-run", "2026-07-08T08:10:00Z", { status: "in_progress" }),
  item("child-done", "2026-07-08T08:20:00Z", { status: "completed" }),
  item("child-stuck", "2026-07-08T08:30:00Z", { status: "blocked" }),
];
const edges: WorkGraphWireEdge[] = [
  { kind: "parent", from_id: "child-run", to_id: "root" },
  { kind: "parent", from_id: "child-done", to_id: "root" },
  { kind: "parent", from_id: "child-stuck", to_id: "root" },
];

describe("WorkGraphGraphView keyboard", () => {
  it("is one tab stop: arrow keys, Home and End move focus between nodes, Enter and Space select", () => {
    // jsdom has no canvas; the labels fall back to their character caps.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    const onSelect = vi.fn();
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} onSelect={onSelect} />);
    const nodes = view.getAllByTestId("workgraph-graph-node");
    expect(nodes.length).toBe(4);
    const node = (id: string) => nodes.find((element) => element.getAttribute("data-item-id") === id) as Element;
    const tabStops = () => nodes.filter((element) => element.getAttribute("tabindex") === "0").map((element) => element.getAttribute("data-item-id"));
    const focusedId = () => document.activeElement?.getAttribute("data-item-id") ?? null;
    const press = (key: string) => fireEvent.keyDown(document.activeElement as Element, { key });

    expect(tabStops()).toEqual(["root"]);
    expect(nodes.filter((element) => element.getAttribute("tabindex") === "-1").length).toBe(3);

    act(() => (node("root") as SVGGElement).focus());
    press("ArrowRight");
    expect(focusedId()).toBe("child-run");
    expect(tabStops()).toEqual(["child-run"]);
    press("ArrowDown");
    expect(focusedId()).toBe("child-done");
    press("End");
    expect(focusedId()).toBe("child-stuck");
    // Nothing lies below the last child: focus stays.
    press("ArrowDown");
    expect(focusedId()).toBe("child-stuck");
    press("ArrowLeft");
    expect(focusedId()).toBe("root");
    press("End");
    press("Home");
    expect(focusedId()).toBe("root");
    expect(tabStops()).toEqual(["root"]);
    // Moving focus never selects; Enter and Space do.
    expect(onSelect.mock.calls.length).toBe(0);
    press("ArrowRight");
    press("Enter");
    press("ArrowDown");
    press(" ");
    expect(onSelect.mock.calls.map((call) => call[0])).toEqual(["child-run", "child-done"]);
  });

  it("makes the selected node the tab stop", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} selectedId="child-done" />);
    const stops = view.getAllByTestId("workgraph-graph-node")
      .filter((element) => element.getAttribute("tabindex") === "0")
      .map((element) => element.getAttribute("data-item-id"));
    expect(stops).toEqual(["child-done"]);
  });
});
