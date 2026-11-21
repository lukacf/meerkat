import React from "react";
import { fireEvent, render } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import type { WorkGraphWireItem } from "../types";
import { WorkGraphPanel } from "./WorkGraphPanel";
import type { WorkGraphPanelData } from "./WorkGraphPanel";

const AT = "2026-11-18T09:00:00Z";
const READY_TITLE = "Counts loaded items that the runtime reports as ready and for which the snapshot records no blockers. Panel filters do not change this count.";

function item(id: string, overrides: Partial<WorkGraphWireItem> = {}): WorkGraphWireItem {
  return { id, title: id, status: "open", revision: 1, created_at: AT, updated_at: AT, ...overrides };
}

function data(overrides: Partial<WorkGraphPanelData> = {}): WorkGraphPanelData {
  return {
    items: [item("free")],
    edges: [],
    attention: [],
    events: [],
    capturedAt: AT,
    readyItemIds: ["free"],
    unavailable: false,
    denied: false,
    error: null,
    ...overrides,
  };
}

function panel(snapshot: WorkGraphPanelData) {
  return render(<WorkGraphPanel data={snapshot} canManage={false} onRefresh={() => {}} />);
}

function readyValue(view: ReturnType<typeof panel>): string | null | undefined {
  return view.getByTestId("workgraph-glance:ready").querySelector(".cc-figure__value")?.textContent;
}

describe("WorkGraph Ready figure in the rendered panel", () => {
  beforeEach(() => window.localStorage.clear());

  it("keeps the loaded Ready count when the person changes the graph filter", () => {
    const view = panel(data({
      items: [item("free"), item("cancelled", { status: "cancelled" })],
      // An id outside the loaded snapshot must not inflate the figure.
      readyItemIds: ["free", "not-loaded"],
    }));
    fireEvent.click(view.getByTestId("workgraph-view-toggle:graph"));
    const visibleIds = () => Array.from(view.container.querySelectorAll('[data-testid="workgraph-graph-node"]'))
      .map((element) => element.getAttribute("data-item-id"));
    expect(readyValue(view)).toBe("1");
    expect(view.getByTestId("workgraph-glance:ready").getAttribute("title")).toBe(READY_TITLE);
    fireEvent.click(view.getByTestId("workgraph-menu"));
    const toggle = view.getByTestId("workgraph-menu:show-cancelled");
    const wasShown = toggle.getAttribute("aria-checked") === "true";
    expect(visibleIds().includes("cancelled")).toBe(wasShown);
    fireEvent.click(toggle);
    expect(visibleIds().includes("cancelled")).toBe(!wasShown);
    expect(readyValue(view)).toBe("1");
    expect(view.getByTestId("workgraph-glance:ready").getAttribute("title")).toBe(READY_TITLE);
  });

  it("excludes a runtime-ready item with an unresolved blocker link in the snapshot", () => {
    const view = panel(data({
      items: [item("free"), item("waiting"), item("blocker", { status: "in_progress" })],
      edges: [{ kind: "blocks", from_id: "blocker", to_id: "waiting" }],
      readyItemIds: ["free", "waiting"],
    }));
    expect(readyValue(view)).toBe("1");
    expect(view.getByTestId("workgraph-glance:ready").getAttribute("title")).toBe(READY_TITLE);
  });

  it("excludes a runtime-ready item with a positive stored blocker count", () => {
    const view = panel(data({
      items: [item("free"), item("waiting", { machine_state: { unresolved_blocker_count: 2 } })],
      readyItemIds: ["free", "waiting"],
    }));
    expect(readyValue(view)).toBe("1");
    expect(view.getByTestId("workgraph-glance:ready").getAttribute("title")).toBe(READY_TITLE);
  });

  it("omits the runtime hover when the ready set is absent, while an empty set remains an answer", () => {
    const view = panel(data({ readyItemIds: undefined }));
    expect(readyValue(view)).toBe("1");
    expect(view.getByTestId("workgraph-glance:ready").hasAttribute("title")).toBe(false);
    view.rerender(<WorkGraphPanel data={data({ readyItemIds: null })} canManage={false} onRefresh={() => {}} />);
    expect(view.getByTestId("workgraph-glance:ready").hasAttribute("title")).toBe(false);
    view.rerender(<WorkGraphPanel data={data({ readyItemIds: [] })} canManage={false} onRefresh={() => {}} />);
    expect(readyValue(view)).toBe("0");
    expect(view.getByTestId("workgraph-glance:ready").getAttribute("title")).toBe(READY_TITLE);
  });
});
