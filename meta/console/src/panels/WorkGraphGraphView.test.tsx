import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { WorkGraphWireEdge, WorkGraphWireItem } from "../types";
import { WorkGraphGraphView } from "./WorkGraphGraphView";
import { WorkGraphPanel } from "./WorkGraphPanel";
import type { WorkGraphPanelData } from "./WorkGraphPanel";

// DOM assertions compare booleans and strings, never elements.

function item(id: string, createdAt: string, overrides: Partial<WorkGraphWireItem> = {}): WorkGraphWireItem {
  return { id, title: id, status: "open", created_at: createdAt, updated_at: createdAt, revision: 1, ...overrides } as WorkGraphWireItem;
}

// A goal and three children in its frame; the running child blocks the stuck one.
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
  { kind: "blocks", from_id: "child-run", to_id: "child-stuck" },
];

/// jsdom lays nothing out: give every element a canvas-sized box so the
/// view measures a frame and fits into it.
function sizeTheCanvas(width = 900, height = 520) {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0, y: 0, top: 0, left: 0, right: width, bottom: height, width, height, toJSON: () => ({}),
  } as DOMRect);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(width);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(height);
}

beforeEach(() => {
  // jsdom has no canvas; text measures fall back to their estimates.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
});
afterEach(() => vi.restoreAllMocks());

const nodeIds = (root: HTMLElement) =>
  Array.from(root.querySelectorAll('[data-testid="workgraph-graph-node"]')).map((node) => node.getAttribute("data-item-id"));

describe("WorkGraphGraphView keyboard", () => {
  it("is one tab stop: arrow keys move focus between nodes, Enter and Space select", () => {
    const onSelect = vi.fn();
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} onSelect={onSelect} />);
    const nodes = view.getAllByTestId("workgraph-graph-node");
    expect(nodes.length).toBe(4);
    const tabStops = () => view.getAllByTestId("workgraph-graph-node")
      .filter((element) => element.getAttribute("tabindex") === "0")
      .map((element) => element.getAttribute("data-item-id"));
    expect(tabStops().length).toBe(1);
    const focusedId = () => document.activeElement?.getAttribute("data-item-id") ?? null;
    const press = (key: string) => fireEvent.keyDown(document.activeElement as Element, { key });

    const first = nodes.find((element) => element.getAttribute("tabindex") === "0") as unknown as SVGGElement;
    act(() => first.focus());
    const visited = new Set([focusedId()]);
    for (const key of ["ArrowRight", "ArrowDown", "ArrowDown", "ArrowLeft", "End", "Home"]) {
      press(key);
      visited.add(focusedId());
    }
    expect(visited.size > 1).toBe(true);
    // Moving focus never selects; Enter and Space do.
    expect(onSelect.mock.calls.length).toBe(0);
    press("Enter");
    expect(onSelect.mock.calls.length > 0).toBe(true);
    expect(onSelect.mock.calls[onSelect.mock.calls.length - 1][0]).toBe(focusedId());
  });

  it("makes the selected node the tab stop and marks it", () => {
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} selectedId="child-done" />);
    const stops = view.getAllByTestId("workgraph-graph-node")
      .filter((element) => element.getAttribute("tabindex") === "0")
      .map((element) => element.getAttribute("data-item-id"));
    expect(stops).toEqual(["child-done"]);
    const selected = view.getAllByTestId("workgraph-graph-node").find((element) => element.getAttribute("data-item-id") === "child-done");
    expect(selected?.getAttribute("aria-pressed")).toBe("true");
  });
});

describe("WorkGraphGraphView refresh", () => {
  it("never refits after the person moved the view, even when the data changes", () => {
    sizeTheCanvas();
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} />);
    const transform = () => view.getByTestId("workgraph-graph-viewport").getAttribute("transform");
    const fitted = transform();
    fireEvent.click(view.getByTestId("workgraph-graph-zoom:in"));
    const moved = transform();
    expect(moved === fitted).toBe(false);
    // A refresh: fresh objects, one item finished, one item added.
    const refreshed = [
      ...items.map((entry) => ({ ...entry, status: entry.id === "child-run" ? "completed" : entry.status })),
      item("child-new", "2026-07-08T08:40:00Z"),
    ];
    view.rerender(
      <WorkGraphGraphView items={refreshed} edges={[...edges, { kind: "parent", from_id: "child-new", to_id: "root" }]} attention={[]} />,
    );
    expect(nodeIds(view.container).includes("child-new")).toBe(true);
    expect(transform()).toBe(moved);
    // Fit hands the view back to the data.
    fireEvent.click(view.getByTestId("workgraph-graph-fit"));
    expect(transform() === moved).toBe(false);
  });
});

describe("WorkGraphGraphView goal zoom", () => {
  it("zooms to a goal when its header is double-clicked, and opens it", () => {
    sizeTheCanvas();
    const onSelect = vi.fn();
    const twoGoals = [...items, item("other", "2026-07-08T07:00:00Z", { title: "Other goal" }), item("other-a", "2026-07-08T07:10:00Z")];
    const view = render(
      <WorkGraphGraphView
        items={twoGoals}
        edges={[...edges, { kind: "parent", from_id: "other-a", to_id: "other" }]}
        attention={[]}
        onSelect={onSelect}
      />,
    );
    const transform = () => view.getByTestId("workgraph-graph-viewport").getAttribute("transform");
    const fitted = transform();
    const header = view.getAllByTestId("workgraph-graph-node").find((element) => element.getAttribute("data-item-id") === "root")!;
    fireEvent.doubleClick(header);
    expect(onSelect.mock.calls.map((call) => call[0]).includes("root")).toBe(true);
    expect(transform() === fitted).toBe(false);
  });
});

describe("WorkGraphGraphView drawer", () => {
  it("opens on a node and says what blocks the item and what it unblocks", () => {
    const onSelect = vi.fn();
    const view = render(<WorkGraphGraphView items={items} edges={edges} attention={[]} onSelect={onSelect} />);
    const stuck = view.getAllByTestId("workgraph-graph-node").find((element) => element.getAttribute("data-item-id") === "child-stuck")!;
    // The graph selects on a pointer press and release, as a mouse click does.
    fireEvent.pointerDown(stuck, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    fireEvent.pointerUp(stuck, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    expect(onSelect.mock.calls.map((call) => call[0])).toEqual(["child-stuck"]);

    view.rerender(<WorkGraphGraphView items={items} edges={edges} attention={[]} onSelect={onSelect} selectedId="child-stuck" />);
    const drawer = view.getByTestId("workgraph-graph-detail");
    expect(drawer.querySelectorAll("h1, h2, h3, h4, h5, h6").length).toBe(1);
    expect(drawer.querySelectorAll("[data-status]").length).toBe(1);
    expect(drawer.querySelector("[data-status]")?.getAttribute("data-status")).toBe("blocked");
    expect(view.getByTestId("workgraph-drawer:blocked-by").textContent?.includes("child-run")).toBe(true);
    // The blocker is a link: choosing it selects it.
    const link = Array.from(view.getByTestId("workgraph-drawer:blocked-by").querySelectorAll("button"))
      .find((button) => button.textContent === "child-run")!;
    fireEvent.click(link);
    expect(onSelect.mock.calls[onSelect.mock.calls.length - 1][0]).toBe("child-run");
  });
});

function panelData(overrides: Partial<WorkGraphPanelData> = {}): WorkGraphPanelData {
  return {
    items,
    edges: [...edges, { kind: "blocks", from_id: "child-stuck", to_id: "root" }],
    attention: [],
    events: [],
    capturedAt: "2026-07-08T09:00:00Z",
    readyItemIds: [],
    unavailable: false,
    denied: false,
    error: null,
    ...overrides,
  };
}

describe("WorkGraphPanel", () => {
  it("keeps the outline in jsdom (no width) and offers the graph", () => {
    const view = render(<WorkGraphPanel data={panelData()} canManage={false} onRefresh={() => {}} />);
    expect(view.queryByTestId("workgraph-graph")).toBeNull();
    expect(view.getByTestId("workgraph-view-toggle:tree").getAttribute("aria-pressed")).toBe("true");
    expect(view.getByTestId("workgraph-panel-item:root").tagName).toBe("ARTICLE");
  });

  it("toggles the critical path from TO FINISH and draws it in the accent", () => {
    const view = render(<WorkGraphPanel data={panelData()} canManage={false} onRefresh={() => {}} />);
    const toggle = view.getByTestId("workgraph-critical-path");
    // child-run blocks child-stuck, which blocks root: three steps, on.
    expect(view.getByTestId("workgraph-glance:to_finish").textContent?.includes("3")).toBe(true);
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(view.getByTestId("workgraph-view-toggle:graph"));
    const accented = () => view.getAllByTestId("workgraph-graph-node")
      .filter((node) => node.classList.contains("is-accent"))
      .map((node) => node.getAttribute("data-item-id"))
      .sort();
    expect(accented()).toEqual(["child-run", "child-stuck", "root"]);
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    expect(accented()).toEqual([]);
  });
});
