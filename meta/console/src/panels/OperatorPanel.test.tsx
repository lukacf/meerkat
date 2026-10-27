import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OperatorPanel } from "./OperatorPanel";
import { Sidebar } from "./Sidebar";
import { buildOperatorPanelTarget, operatorPanelPath } from "../lib/adapters";

afterEach(() => { cleanup(); localStorage.clear(); });
const sidebar = { agents: [], selectedMemberId: "", recentActivity: [], collapsed: false, visibleControls: [],
  onSelect: vi.fn(), onOpenControl: vi.fn() };

describe("Operator panels", () => {
  it("accepts only same-origin panel paths", () => {
    expect(operatorPanelPath("/host/reviews?tab=open")).toBe("/host/reviews?tab=open");
    for (const value of ["https://host.example/x", "//host.example/x", "host/x", "/a b", " /a", "/a ", "/a\\b", "/a\u0007b",
      "/a\u0085b", `/${"a".repeat(2048)}`, "/.//evil.example/x", "/..//x", "/%2e//x", "/%2E%2e//evil.example/x", "/a/../b",
      "/a/./b", "\ufeff/a", "/a\ufeffb", 7, undefined]) {
      expect(operatorPanelPath(value)).toBeUndefined();
    }
    // The limit counts UTF-8 bytes, like the server.
    const multibyte = `/${"\u00e9".repeat(1023)}`;
    expect(operatorPanelPath(multibyte)).toBe(multibyte);
    expect(operatorPanelPath(`${multibyte}\u00e9`)).toBeUndefined();
    expect(operatorPanelPath("/host/a.b/..c?next=/../")).toBe("/host/a.b/..c?next=/../");
  });

  it("treats an empty panel as no panel", () => {
    const view = render(<Sidebar {...sidebar} onOpenOperatorPanel={vi.fn()} customButtons={[
      { id: "docs", label: "Docs", href: "https://docs.example/", panel: "" },
    ]} />);
    expect(view.queryByTestId("nav-panel:docs")).toBeNull();
    expect(view.getByTestId("nav-custom:docs").getAttribute("href")).toBe("https://docs.example/");
  });

  it("opens a configured panel button in the dock and drops invalid ones", () => {
    const onOpenOperatorPanel = vi.fn();
    const view = render(<Sidebar {...sidebar} onOpenOperatorPanel={onOpenOperatorPanel} customButtons={[
      { id: "reviews", label: "Reviews", panel: "/host/reviews" },
      { id: "bad", label: "Bad", panel: "https://evil.example/" },
    ]} />);
    fireEvent.click(view.getByTestId("nav-panel:reviews"));
    expect(onOpenOperatorPanel).toHaveBeenCalledWith("reviews");
    expect(view.queryByTestId("nav-panel:bad")).toBeNull();
    expect(buildOperatorPanelTarget({ id: "reviews", label: "Reviews" })).toMatchObject({ kind: "operator-panel", buttonId: "reviews", title: "Reviews" });
  });

  it("renders the path from current configuration, not from the saved target", () => {
    const view = render(<OperatorPanel buttonId="reviews" buttons={[{ id: "reviews", label: "Reviews", panel: "/host/reviews" }]} />);
    const frame = view.container.querySelector("iframe");
    expect(frame?.getAttribute("src")).toBe("/host/reviews");
    expect(frame?.getAttribute("title")).toBe("Reviews");
    cleanup();
    expect(render(<OperatorPanel buttonId="reviews" buttons={[]} />).getByTestId("operator-panel-missing:reviews")).toBeTruthy();
    cleanup();
    expect(render(<OperatorPanel buttonId="reviews" buttons={[{ id: "reviews", label: "Reviews", panel: "https://evil.example/" }]} />)
      .container.querySelector("iframe")).toBeNull();
  });

  it("restores a saved panel through current configuration only", () => {
    const saved = JSON.parse(JSON.stringify({ ...buildOperatorPanelTarget({ id: "reviews", label: "Reviews" }), panel: "/tampered" }));
    const view = render(<OperatorPanel buttonId={saved.buttonId} buttons={[{ id: "reviews", label: "Reviews", panel: "/host/reviews" }]} />);
    expect(view.container.querySelector("iframe")?.getAttribute("src")).toBe("/host/reviews");
  });

  it("follows a changed path under the same button id", () => {
    const view = render(<OperatorPanel buttonId="reviews" buttons={[{ id: "reviews", label: "Reviews", panel: "/host/reviews" }]} />);
    view.rerender(<OperatorPanel buttonId="reviews" buttons={[{ id: "reviews", label: "Reviews", panel: "/host/reviews-v2" }]} />);
    expect(view.container.querySelector("iframe")?.getAttribute("src")).toBe("/host/reviews-v2");
    view.rerender(<OperatorPanel buttonId="reviews" buttons={[{ id: "reviews", label: "Reviews", panel: "/./host" }]} />);
    expect(view.container.querySelector("iframe")).toBeNull();
  });
});
