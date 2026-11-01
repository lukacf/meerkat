import { act, fireEvent, render, screen, within } from "@testing-library/react";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { GraphView } from "./graph-view";
import { GraphList, connectionCount, connectionsSummary, graphListRows } from "./list-twin";
import { StatusSquare } from "./status-square";
import { estimateLabelWidth, markGeometry } from "../layout/geometry";
import { graphSummary, nodeAccessibleName, vizRelationWord } from "./words";
import type { GraphViewHandle, VizEdge, VizNode } from "./types";

// jsdom has no PointerEvent: a MouseEvent with a pointer id carries the modifier keys.
if (typeof (globalThis as { PointerEvent?: unknown }).PointerEvent === "undefined") {
  class TestPointerEvent extends MouseEvent {
    readonly pointerId: number;
    constructor(type: string, init: MouseEventInit & { pointerId?: number } = {}) {
      super(type, init);
      this.pointerId = init.pointerId ?? 1;
    }
  }
  (globalThis as { PointerEvent?: unknown }).PointerEvent = TestPointerEvent;
}

function team(): { nodes: VizNode[]; edges: VizEdge[] } {
  const nodes: VizNode[] = [
    { id: "router", kind: "coordinator", label: "Router", lane: "coord", sublabel: "Coordinator", status: "idle" },
    { id: "triage", kind: "coordinator", label: "Triage", lane: "coord", live: true, status: "working", statusLabel: "Working" },
    { id: "billing", kind: "agent", label: "Billing", lane: "domains", status: "idle", memory: "idle" },
    { id: "support", kind: "agent", label: "Support", lane: "domains", status: "degraded", tone: "warn" },
    { id: "old", kind: "agent", label: "Old agent", lane: "domains", retired: true },
  ];
  const edges: VizEdge[] = [
    { id: "w1", from: "router", to: "triage", relation: "wired" },
    { id: "w2", from: "router", to: "billing", relation: "wired" },
    { id: "t1", from: "triage", to: "support", relation: "traffic", weight: 12, directed: true, label: "12/h" },
    { id: "d1", from: "billing", to: "support", relation: "wired", state: "declared" },
    { id: "d2", from: "router", to: "support", relation: "wired", state: "declared", mark: null },
  ];
  return { nodes, edges };
}

const lanes = [
  { id: "coord", label: "Coordinators" },
  { id: "domains", label: "Domains" },
];

describe("GraphView", () => {
  it("is one labelled graph group with derived test ids and data attributes", () => {
    const { nodes, edges } = team();
    render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="4 agents, 5 connections" testIdPrefix="topology" testIds={{ root: "topology-dense-map" }} rootAttributes={{ "data-topology-editable": "false", "data-drag-kind": "none" }} />);
    const root = screen.getByTestId("topology-dense-map");
    expect(root).toHaveAttribute("role", "group");
    expect(root).toHaveAttribute("aria-roledescription", "graph");
    expect(root).toHaveAttribute("aria-label", "4 agents, 5 connections");
    expect(root).toHaveAttribute("data-topology-editable", "false");
    expect(root).toHaveAttribute("data-drag-kind", "none");
    const triage = screen.getByTestId("topology-node:triage");
    expect(triage).toHaveAttribute("data-kind", "coordinator");
    expect(triage).toHaveAttribute("data-live", "true");
    const traffic = screen.getByTestId("topology-edge:triage:support:traffic");
    expect(traffic).toHaveAttribute("data-relation", "traffic");
    expect(traffic).toHaveAttribute("data-state", "normal");
    expect(screen.getByTestId("topology-edge:billing:support:wired")).toHaveAttribute("data-state", "declared");
    expect(screen.getByTestId("topology-legend")).toBeInTheDocument();
    expect(screen.getByTestId("topology-legend:wired")).toHaveAttribute("aria-pressed", "true");
    for (const action of ["in", "out", "reset", "fit"]) expect(screen.getByTestId(`topology-zoom:${action}`)).toBeInTheDocument();
  });

  it("hides retired nodes unless asked, and draws the declared mark unless the edge says none", () => {
    const { nodes, edges } = team();
    const { container, rerender } = render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" />);
    expect(screen.queryByTestId("t-node:old")).toBeNull();
    const declared = screen.getByTestId("t-edge:billing:support:wired");
    expect(declared.querySelector(".cc-viz-edge__mark.is-warn")).not.toBeNull();
    expect(screen.getByTestId("t-edge:router:support:wired").querySelector(".cc-viz-edge__mark")).toBeNull();
    rerender(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" showRetired />);
    expect(screen.getByTestId("t-node:old")).toHaveClass("is-retired");
    expect(container.querySelectorAll(".cc-viz-lane__label")).toHaveLength(2);
  });

  it("names each node in words and keeps exactly one tab stop", () => {
    const { nodes, edges } = team();
    render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" selection={["billing"]} />);
    const buttons = screen.getAllByRole("button", { pressed: false }).filter((element) => element.hasAttribute("data-node-id"));
    const billing = screen.getByTestId("t-node:billing");
    expect(billing).toHaveAttribute("aria-pressed", "true");
    expect(billing).toHaveAttribute("aria-label", "Billing, Idle, 2 connections");
    const stops = [billing, ...buttons].filter((element) => element.getAttribute("tabindex") === "0");
    expect(stops).toEqual([billing]);
  });

  it("moves focus with the arrow keys, opens on Enter, toggles on Space and clears on Escape", () => {
    const { nodes, edges } = team();
    const onOpen = vi.fn();
    const onSelect = vi.fn();
    render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" onOpen={onOpen} onSelect={onSelect} />);
    const router = screen.getByTestId("t-node:router");
    act(() => router.focus());
    fireEvent.keyDown(router, { key: "ArrowRight" });
    const triage = screen.getByTestId("t-node:triage");
    expect(document.activeElement).toBe(triage);
    expect(triage).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("status")).toHaveTextContent("Triage, Working, 2 connections");
    fireEvent.keyDown(triage, { key: "ArrowDown" });
    expect(document.activeElement?.getAttribute("data-node-id")).toMatch(/billing|support/);
    fireEvent.keyDown(document.activeElement!, { key: "Enter" });
    expect(onOpen).toHaveBeenCalledWith(document.activeElement!.getAttribute("data-node-id"));
    fireEvent.keyDown(document.activeElement!, { key: " " });
    expect(onSelect).toHaveBeenLastCalledWith([document.activeElement!.getAttribute("data-node-id")]);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(onSelect).toHaveBeenLastCalledWith([]);
  });

  it("selects on a press, adds with Shift, and clears on a press on empty space", () => {
    const { nodes, edges } = team();
    const onSelect = vi.fn();
    const { container } = render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" onSelect={onSelect} />);
    const press = (element: Element, init: Record<string, unknown> = {}) => {
      fireEvent.pointerDown(element, { button: 0, pointerId: 1, clientX: 10, clientY: 10, ...init });
      fireEvent.pointerUp(element, { button: 0, pointerId: 1, clientX: 10, clientY: 10, ...init });
    };
    press(screen.getByTestId("t-node:billing"));
    expect(onSelect).toHaveBeenLastCalledWith(["billing"]);
    press(screen.getByTestId("t-node:router"), { shiftKey: true });
    expect(onSelect).toHaveBeenLastCalledWith(["billing", "router"]);
    press(container.querySelector(".cc-viz-graph__svg")!);
    expect(onSelect).toHaveBeenLastCalledWith([]);
  });

  it("toggles a relation from the legend", () => {
    const { nodes, edges } = team();
    const onHidden = vi.fn();
    render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" onHiddenRelationsChange={onHidden} />);
    fireEvent.click(screen.getByTestId("t-legend:traffic"));
    expect(onHidden).toHaveBeenCalledWith(["traffic"]);
    expect(screen.queryByTestId("t-edge:triage:support:traffic")).toBeNull();
    expect(screen.getByTestId("t-legend:traffic")).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(screen.getByTestId("t-legend:traffic"));
    expect(screen.getByTestId("t-edge:triage:support:traffic")).toBeInTheDocument();
  });

  it("shows the legend note even where auto would hide the legend", () => {
    render(
      <GraphView
        nodes={[{ id: "a", kind: "agent", label: "A" }]}
        edges={[]}
        layout="lanes"
        ariaLabel="g"
        testIdPrefix="t"
        legendNote="Routes need routing access"
      />,
    );
    expect(within(screen.getByTestId("t-legend")).getByText("Routes need routing access")).toBeInTheDocument();
  });

  it("keeps WorkGraph's pins: fixed test ids, data attributes, title classes and goal frames", () => {
    const nodes: VizNode[] = [
      { id: "goal", kind: "goal", label: "Ship the 2.4 release", status: "in_progress", progress: { done: 1, inProgress: 1, blocked: 1, total: 5 }, trailing: { text: "Coordinator · Leading the work", avatar: "C" }, attributes: { "data-item-id": "goal", "data-status": "in_progress" }, className: "is-in_progress" },
      { id: "a", kind: "item", label: "Freeze the branch", frame: "goal", status: "completed", sublabel: "Release Manager", attributes: { "data-item-id": "a", "data-status": "completed", "data-goal-id": "goal" }, className: "is-completed" },
      { id: "b", kind: "item", label: "Run the suite", frame: "goal", status: "in_progress", live: true, attributes: { "data-item-id": "b", "data-status": "in_progress" }, className: "is-in_progress" },
    ];
    const edges: VizEdge[] = [{ id: "ab", from: "a", to: "b", relation: "blocks", label: "blocks", attributes: { "data-kind": "blocks" } }];
    render(
      <GraphView
        nodes={nodes}
        edges={edges}
        layout="layered"
        ariaLabel="Work item dependency graph"
        testIdPrefix="workgraph-graph"
        testIds={{ root: "workgraph-graph-frame", svg: "workgraph-graph", viewport: "workgraph-graph-viewport", node: "workgraph-graph-node", edge: "workgraph-graph-edge", edgeLabel: "workgraph-graph-edge-label" }}
        classNames={{ node: "workgraph-graph__node", itemTitle: "workgraph-graph__node-title" }}
        edgeLabels="always"
        accentPath={{ nodes: ["b"], edges: ["ab"] }}
      />,
    );
    const items = screen.getAllByTestId("workgraph-graph-node");
    expect(items).toHaveLength(3);
    const done = items.find((element) => element.getAttribute("data-item-id") === "a")!;
    expect(done).toHaveAttribute("data-status", "completed");
    expect(done).toHaveAttribute("data-goal-id", "goal");
    expect(done).toHaveClass("workgraph-graph__node", "is-completed");
    expect(done.querySelector(".workgraph-graph__node-title")).toHaveTextContent("Freeze the branch");
    expect(screen.getByTestId("workgraph-graph-frame:goal")).toBeInTheDocument();
    expect(screen.getAllByTestId("workgraph-graph-edge")).toHaveLength(1);
    expect(screen.getByTestId("workgraph-graph-edge")).toHaveAttribute("data-kind", "blocks");
    expect(screen.getByTestId("workgraph-graph-edge")).toHaveClass("is-accent");
    expect(screen.getAllByTestId("workgraph-graph-edge-label")).toHaveLength(1);
    expect(screen.getByTestId("workgraph-graph-viewport").getAttribute("transform")).toMatch(/^translate\([-\d.e]+ [-\d.e]+\) scale\([-\d.e]+\)$/);
    const goal = items.find((element) => element.getAttribute("data-item-id") === "goal")!;
    expect(goal.querySelector(".workgraph-graph__node-title")).toHaveTextContent("Ship the 2.4 release");
    expect(goal.querySelectorAll(".cc-viz-ring-progress__arc")).toHaveLength(3);
    expect(goal.querySelector(".cc-viz-goal__trailing")).toHaveTextContent("Coordinator · Leading the work");
    expect(items.find((element) => element.getAttribute("data-item-id") === "b")).toHaveClass("is-accent");
  });

  it("draws in a server render, where no layout effect measures the frame", () => {
    const nodes: VizNode[] = [
      { id: "goal", kind: "goal", label: "Ship the 2.4 release", status: "in_progress" },
      { id: "a", kind: "item", label: "Freeze the branch", frame: "goal", status: "completed", attributes: { "data-goal-id": "goal" }, className: "is-completed" },
      { id: "b", kind: "item", label: "Run the suite", frame: "goal", status: "blocked", className: "is-blocked" },
    ];
    const edges: VizEdge[] = [{ id: "ab", from: "a", to: "b", relation: "blocks", attributes: { "data-kind": "blocks" } }];
    const html = renderToStaticMarkup(
      <GraphView
        nodes={nodes}
        edges={edges}
        layout="layered"
        ariaLabel="Work item dependency graph"
        testIdPrefix="workgraph-graph"
        testIds={{ root: "workgraph-graph-frame", node: "workgraph-graph-node", edge: "workgraph-graph-edge" }}
        classNames={{ itemTitle: "workgraph-graph__node-title" }}
      />,
    );
    expect(html.match(/data-testid="workgraph-graph-node"/g)).toHaveLength(3);
    expect(html).toContain('data-testid="workgraph-graph-frame:goal"');
    expect(html).toContain('data-goal-id="goal"');
    expect(html).toMatch(/class="[^"]*\bis-blocked\b/);
    expect(html).toMatch(/data-testid="workgraph-graph-edge"[^>]*data-kind="blocks"|data-kind="blocks"[^>]*data-testid="workgraph-graph-edge"/);
    expect(html).toContain("workgraph-graph__node-title");
  });

  it("brings focusId into the tab order and returns focus through the handle", () => {
    const { nodes, edges } = team();
    const ref = React.createRef<GraphViewHandle>();
    const { rerender } = render(<GraphView ref={ref} nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" />);
    rerender(<GraphView ref={ref} nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" focusId="support" />);
    expect(screen.getByTestId("t-node:support")).toHaveAttribute("tabindex", "0");
    act(() => ref.current!.focusNode("billing"));
    expect(document.activeElement).toBe(screen.getByTestId("t-node:billing"));
    expect(ref.current!.viewport()).toEqual(expect.objectContaining({ scale: expect.any(Number) }));
  });

  it("zooms to a node or to a goal's whole frame through the handle", () => {
    // The graph's frame is 800 x 600; its chrome takes no room.
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const [width, height] = this.classList.contains("cc-viz-graph") ? [800, 600] : [0, 0];
      return { x: 0, y: 0, left: 0, top: 0, right: width, bottom: height, width, height, toJSON: () => ({}) } as DOMRect;
    });
    try {
      const nodes: VizNode[] = [
        { id: "goal", kind: "goal", label: "Ship the 2.4 release" },
        ...Array.from({ length: 6 }, (_, index): VizNode => ({ id: `i${index}`, kind: "item", label: `Item ${index}`, frame: "goal", status: "open" })),
      ];
      const edges: VizEdge[] = Array.from({ length: 5 }, (_, index) => ({ id: `e${index}`, from: `i${index}`, to: `i${index + 1}`, relation: "blocks" }));
      const ref = React.createRef<GraphViewHandle>();
      render(<GraphView ref={ref} nodes={nodes} edges={edges} layout="layered" ariaLabel="g" testIdPrefix="t" legend="off" />);
      act(() => ref.current!.fitTo(["i0"]));
      const one = ref.current!.viewport();
      expect(one.scale).toBe(1.6);
      act(() => ref.current!.fitTo(["goal"]));
      const frame = ref.current!.viewport();
      expect(frame.scale).toBeLessThan(one.scale);
      act(() => ref.current!.fitTo(["nobody"]));
      expect(ref.current!.viewport()).toEqual(frame);
    } finally {
      rect.mockRestore();
    }
  });

  it("fits a small layered graph by its right-hand labels, inside the given padding", () => {
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const [width, height] = this.classList.contains("cc-viz-graph") ? [420, 64] : [0, 0];
      return { x: 0, y: 0, left: 0, top: 0, right: width, bottom: height, width, height, toJSON: () => ({}) } as DOMRect;
    });
    try {
      const first = "Version one, the first of them";
      const nodes: VizNode[] = [
        { id: "v1", kind: "agent", label: first },
        { id: "v2", kind: "agent", label: "v2" },
        { id: "v3", kind: "agent", label: "v3" },
      ];
      const edges: VizEdge[] = [
        { id: "a", from: "v1", to: "v2", relation: "blocks" },
        { id: "b", from: "v2", to: "v3", relation: "blocks" },
      ];
      const ref = React.createRef<GraphViewHandle>();
      render(<GraphView ref={ref} nodes={nodes} edges={edges} layout="layered" ariaLabel="g" testIdPrefix="t" legend="off" controls={false} minimap="off" fitPadding={8} />);
      const view = ref.current!.viewport();
      const at = (id: string) => screen.getByTestId(`t-node:${id}`).getAttribute("transform")!.match(/translate\(([-\d.]+) ([-\d.]+)\)/)!.slice(1).map(Number);
      const r = markGeometry("agent").outer;
      const [x1, y1] = at("v1");
      const [x3] = at("v3");
      expect(view.scale).toBe(1);
      // The labels sit on the right: nothing is reserved left of the first mark
      // (the picture is centred across any slack the width leaves).
      const left = (x1 - r) * view.scale + view.tx;
      expect(left).toBeGreaterThanOrEqual(8);
      expect(left).toBeLessThanOrEqual(8 + 8);
      expect((x3 + r + 5 + estimateLabelWidth("v3")) * view.scale + view.tx).toBeLessThanOrEqual(420 - 8);
      // Pinned to the top: the first row (24 px) starts 8 px in.
      expect(y1 * view.scale + view.ty).toBeLessThanOrEqual(8 + 12 + 1);
    } finally {
      rect.mockRestore();
    }
  });

  it("folds the legend in a narrow frame until it is unfolded, then stacks its rows", () => {
    let frameWidth = 500;
    const rect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const [width, height] = this.classList.contains("cc-viz-graph") ? [frameWidth, 600] : [0, 0];
      return { x: 0, y: 0, left: 0, top: 0, right: width, bottom: height, width, height, toJSON: () => ({}) } as DOMRect;
    });
    try {
      const { nodes, edges } = team();
      const { unmount } = render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" />);
      const legend = () => screen.getByTestId("t-legend");
      expect(legend()).toHaveClass("is-collapsed");
      fireEvent.click(within(legend()).getByRole("button", { name: "Legend" }));
      expect(legend()).not.toHaveClass("is-collapsed");
      expect(legend()).toHaveClass("is-stacked");
      expect(screen.getByTestId("t-legend:wired")).toBeInTheDocument();
      unmount();
      frameWidth = 900;
      render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" />);
      expect(legend()).not.toHaveClass("is-collapsed");
      expect(legend()).not.toHaveClass("is-stacked");
    } finally {
      rect.mockRestore();
    }
  });

  it("highlights a hovered node's neighbours and lets the highlight linger a moment over empty space", async () => {
    const { nodes, edges } = team();
    const onHover = vi.fn();
    render(<GraphView nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" onHover={onHover} />);
    const root = screen.getByTestId("t-graph");
    fireEvent.pointerOver(screen.getByTestId("t-node:billing"));
    expect(root).toHaveAttribute("data-hovering", "true");
    expect(screen.getByTestId("t-node:router")).toHaveClass("is-lit");
    expect(screen.getByTestId("t-node:support")).toHaveClass("is-lit");
    expect(screen.getByTestId("t-node:triage")).not.toHaveClass("is-lit");
    expect(onHover).toHaveBeenLastCalledWith({ node: "billing" });
    // Empty space: still lit at once, cleared shortly after.
    fireEvent.pointerOver(root.querySelector("svg")!);
    expect(root).toHaveAttribute("data-hovering", "true");
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 150));
    });
    expect(root).not.toHaveAttribute("data-hovering");
    expect(onHover).toHaveBeenLastCalledWith(null);
    // Leaving the graph clears at once.
    fireEvent.pointerOver(screen.getByTestId("t-node:router"));
    expect(root).toHaveAttribute("data-hovering", "true");
    fireEvent.pointerLeave(root);
    expect(root).not.toHaveAttribute("data-hovering");
  });

  it("leaves plain wheel scrolling to the page unless asked to zoom with it", () => {
    const { nodes, edges } = team();
    const wheel = (target: Element, init: WheelEventInit) => {
      const event = new WheelEvent("wheel", { deltaY: -100, bubbles: true, cancelable: true, ...init });
      act(() => {
        target.dispatchEvent(event);
      });
      return event.defaultPrevented;
    };
    const ref = React.createRef<GraphViewHandle>();
    const { rerender } = render(<GraphView ref={ref} nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" />);
    const root = screen.getByTestId("t-graph");
    const start = ref.current!.viewport().scale;
    expect(wheel(root, {})).toBe(true);
    const zoomed = ref.current!.viewport().scale;
    expect(zoomed).toBeGreaterThan(start);
    rerender(<GraphView ref={ref} nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" wheelZoom="pinch" />);
    expect(wheel(root, {})).toBe(false);
    expect(ref.current!.viewport().scale).toBe(zoomed);
    expect(wheel(root, { ctrlKey: true })).toBe(true);
    expect(ref.current!.viewport().scale).toBeGreaterThan(zoomed);
    rerender(<GraphView ref={ref} nodes={nodes} edges={edges} lanes={lanes} layout="lanes" ariaLabel="g" testIdPrefix="t" wheelZoom="off" />);
    expect(wheel(root, { ctrlKey: true })).toBe(false);
  });

  it("draws above 300 nodes on a canvas with one focus proxy, and switches force to clusters above 150", () => {
    const nodes: VizNode[] = Array.from({ length: 320 }, (_, index) => ({ id: `n${index}`, kind: index < 4 ? "coordinator" : "agent", label: `Agent ${index}`, lane: `g${index % 4}` }));
    const edges: VizEdge[] = nodes.slice(4).map((node, index) => ({ id: `e${index}`, from: `n${index % 4}`, to: node.id, relation: "wired" }));
    const { container } = render(<GraphView nodes={nodes} edges={edges} layout="force" ariaLabel="g" testIdPrefix="t" />);
    const root = screen.getByTestId("t-graph");
    expect(root).toHaveAttribute("data-renderer", "canvas");
    expect(root).toHaveAttribute("data-layout", "clusters");
    expect(container.querySelector("canvas.cc-viz-graph__canvas")).not.toBeNull();
    expect(container.querySelectorAll('[data-node-id][role="button"]')).toHaveLength(0);
    const proxy = container.querySelector(".cc-viz-proxy")!;
    expect(proxy).toHaveAttribute("tabindex", "0");
    expect(proxy.getAttribute("aria-label")).toMatch(/^Agent /);
    expect(container.querySelectorAll(".cc-viz-cluster")).toHaveLength(4);
  });
});

describe("the list twin", () => {
  it("lists every node with its relations in words, sorted by group, connections or messages", () => {
    const { nodes, edges } = team();
    const rows = graphListRows(nodes, edges, { lanes });
    expect(rows.map((row) => row.node.id)).toEqual(["router", "triage", "billing", "support"]);
    const triage = rows.find((row) => row.node.id === "triage")!;
    expect(triage.messages).toBe(12);
    expect(connectionsSummary(triage)).toBe("Router · wired, Support · 12 messages");
    const byDegree = graphListRows(nodes, edges, { sortBy: "degree" });
    expect(byDegree.map((row) => row.degree)).toEqual([...byDegree.map((row) => row.degree)].sort((a, b) => b - a));
    expect(byDegree.slice(0, 2).map((row) => row.node.id)).toEqual(["router", "support"]);
    expect(graphListRows(nodes, edges, { sortBy: "messages" })[0].node.id).toMatch(/triage|support/);
    expect(graphListRows(nodes, edges, { showRetired: true })).toHaveLength(5);
  });

  it("renders rows that select and open, and windows above 200 rows", () => {
    const { nodes, edges } = team();
    const onSelect = vi.fn();
    const onOpen = vi.fn();
    render(<GraphList nodes={nodes} edges={edges} lanes={lanes} ariaLabel="Agents" testIdPrefix="topology" onSelect={onSelect} onOpen={onOpen} />);
    const row = screen.getByTestId("topology-list-row:billing");
    fireEvent.click(within(row).getByRole("button", { name: "Billing" }));
    expect(onSelect).toHaveBeenCalledWith(["billing"]);
    fireEvent.click(within(row).getByRole("button", { name: "Open Billing" }));
    expect(onOpen).toHaveBeenCalledWith("billing");
    const many: VizNode[] = Array.from({ length: 500 }, (_, index) => ({ id: `n${index}`, kind: "agent", label: `Agent ${String(index).padStart(3, "0")}` }));
    const { container } = render(<GraphList nodes={many} edges={[]} ariaLabel="All" testIdPrefix="big" />);
    const rendered = container.querySelectorAll('[data-testid^="big-list-row:"]');
    expect(rendered.length).toBeGreaterThan(0);
    expect(rendered.length).toBeLessThan(100);
    expect(container.querySelector("table")).toHaveAttribute("aria-rowcount", "501");
  });

  it("carries the status and the connection count on the meta line a narrow list shows", () => {
    const { nodes, edges } = team();
    render(<GraphList nodes={nodes} edges={edges} lanes={lanes} ariaLabel="Agents" testIdPrefix="topology" />);
    const fold = (id: string) => screen.getByTestId(`topology-list-row:${id}`).querySelector(".cc-viz-list__fold")!.textContent;
    expect(fold("triage")).toBe("Working · 2 connections");
    expect(fold("router")).toBe(" · Idle · 3 connections");
    expect(connectionCount(1)).toBe("1 connection");
    expect(connectionCount(0)).toBe("No connections");
  });
});

describe("words and marks", () => {
  it("summarises the graph and names relations in words", () => {
    const { nodes, edges } = team();
    expect(graphSummary(nodes, edges)).toBe("5 agents, 5 connections");
    expect(graphSummary([{ id: "i", kind: "item", label: "I" }, { id: "g", kind: "goal", label: "G" }], [{ id: "e", from: "i", to: "g", relation: "parent" }])).toBe("2 items, 1 link");
    expect(graphSummary([{ id: "s", kind: "source", label: "S" }, { id: "a", kind: "agent", label: "A" }], [])).toBe("1 agent, 1 channel, 0 connections");
    expect(vizRelationWord("lineage", 2)).toBe("helpers");
    expect(nodeAccessibleName(nodes[1], 2)).toBe("Triage, Working, 2 connections");
  });

  it("draws the status square without a data-status (the WorkGraph drawer pins exactly one)", () => {
    const { container } = render(<StatusSquare status="blocked" />);
    const svg = container.querySelector("svg")!;
    expect(svg).not.toHaveAttribute("data-status");
    expect(svg).toHaveAttribute("data-square", "blocked");
    expect(svg).toHaveAttribute("aria-hidden", "true");
    const named = render(<StatusSquare status="weird" label="Open" />).container.querySelector("svg")!;
    expect(named).toHaveAttribute("data-square", "open");
    expect(named).toHaveAttribute("role", "img");
  });
});
