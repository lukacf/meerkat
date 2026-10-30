import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkGraphWireEdge, WorkGraphWireItem } from "../types";
import { WorkGraphGraphView } from "./WorkGraphGraphView";
import { revealViewport } from "./topology/zoom-pan";

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

describe("WorkGraphGraphView keyboard focus and the view", () => {
  it("revealViewport pans the least needed, and not at all for a box in view", () => {
    const view = { tx: 0, ty: 0, scale: 1 };
    expect(revealViewport(view, { x: 20, y: 20, w: 100, h: 40 }, 600, 400) === view).toBe(true);
    expect(revealViewport(view, { x: 900, y: 10, w: 100, h: 40 }, 600, 400)).toEqual({ tx: -416, ty: 6, scale: 1 });
    expect(revealViewport({ tx: -500, ty: 0, scale: 2 }, { x: 10, y: 50, w: 50, h: 20 }, 600, 400)).toEqual({ tx: -4, ty: 0, scale: 2 });
  });

  it("pans a node that takes keyboard focus off screen into the frame", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    // A frame smaller than the graph at its smallest fit.
    vi.spyOn(Element.prototype, "clientWidth", "get").mockReturnValue(160);
    vi.spyOn(Element.prototype, "clientHeight", "get").mockReturnValue(90);
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} onSelect={vi.fn()} />);
    const transform = () => {
      const match = /translate\(([-\d.e]+) ([-\d.e]+)\) scale\(([-\d.e]+)\)/.exec(view.getByTestId("workgraph-graph-viewport").getAttribute("transform") || "");
      return match ? match.slice(1).map(Number) : [0, 0, 1];
    };
    const nodes = view.getAllByTestId("workgraph-graph-node");
    const inFrame = (id: string) => {
      const node = nodes.find((element) => element.getAttribute("data-item-id") === id) as Element;
      const [x, y] = (/translate\(([-\d.e]+) ([-\d.e]+)\)/.exec(node.getAttribute("transform") || "") || []).slice(1).map(Number);
      const box = node.querySelector(".workgraph-graph__node-box");
      const w = Number(box?.getAttribute("width")), h = Number(box?.getAttribute("height"));
      const [tx, ty, scale] = transform();
      const left = tx + x * scale, top = ty + y * scale;
      return left >= 0 && top >= 0 && left + w * scale <= 160 && top + h * scale <= 90;
    };
    const offScreen = nodes.map((element) => element.getAttribute("data-item-id") || "").filter((id) => !inFrame(id));
    expect(offScreen.length > 0).toBe(true);
    const target = offScreen[offScreen.length - 1]!;
    act(() => {
      (nodes.find((element) => element.getAttribute("data-item-id") === target) as unknown as HTMLElement).focus();
    });
    expect(inFrame(target)).toBe(true);
  });
});
