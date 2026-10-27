import { fireEvent, render, screen } from "@testing-library/react";

import { ConsoleDock } from "./console-dock";

function Icon({ name }: { name: string; className?: string }) {
  return <span>{name}</span>;
}

afterEach(() => {
  delete document.documentElement.dataset.ccResizing;
  delete document.documentElement.dataset.ccResizeLockCount;
});

describe("ConsoleDock", () => {
  test("renders tabs and panel actions and forwards callbacks", () => {
    const onSelectTab = vi.fn();
    const onCreateTab = vi.fn();
    const onFocusPanel = vi.fn();
    const onSplitPanel = vi.fn();
    const onResizeSplit = vi.fn();
    const onClosePanel = vi.fn();
    const onCloseTab = vi.fn();
    const getBoundingClientRect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({
      width: 400,
      height: 280,
      top: 20,
      left: 40,
      right: 440,
      bottom: 300,
      x: 40,
      y: 20,
      toJSON: () => ({}),
    }));

    render(
      <ConsoleDock
        Icon={Icon}
        onClosePanel={onClosePanel}
        onCloseTab={onCloseTab}
        onCreateTab={onCreateTab}
        onFocusPanel={onFocusPanel}
        onResizeSplit={onResizeSplit}
        onSelectTab={onSelectTab}
        onSplitPanel={onSplitPanel}
        renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
        renderPanelFooter={(panel) => <div>{`footer:${panel.id}`}</div>}
        viewState={{
          activeTabId: "compare",
          focusedPanelId: "panel-a",
          panels: [
            { id: "panel-a", title: "Panel A", subtitle: "workspace" },
            { id: "panel-b", title: "Panel B", subtitle: "workspace", closable: true },
          ],
          tabs: [
            {
              id: "focus",
              title: "Focus",
              layout: { kind: "panel", panelId: "panel-a" },
            },
            {
              id: "compare",
              title: "Compare",
              layout: {
                kind: "split",
                id: "compare-layout",
                direction: "horizontal",
                first: { kind: "panel", panelId: "panel-a" },
                second: { kind: "panel", panelId: "panel-b" },
              },
            },
          ],
        }}
      />,
    );

    fireEvent.click(screen.getByRole("tab", { name: /Compare/i }));
    expect(onSelectTab).toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "New tab" }));
    expect(onCreateTab).toHaveBeenCalled();

    fireEvent.mouseDown(screen.getByText("body:panel-a"));
    expect(onFocusPanel).toHaveBeenCalled();

    fireEvent.click(screen.getAllByRole("button", { name: "Split right" })[0]!);
    expect(onSplitPanel).toHaveBeenCalled();

    fireEvent.click(screen.getAllByRole("button", { name: "Close panel" })[0]!);
    expect(onClosePanel).toHaveBeenCalled();

    fireEvent.pointerDown(screen.getByRole("button", { name: "Resize horizontal split" }), { clientX: 240, clientY: 40 });
    fireEvent.pointerMove(window, { clientX: 320, clientY: 40 });
    fireEvent.pointerUp(window);
    expect(onResizeSplit).toHaveBeenCalledWith("compare-layout", 0.7);

    fireEvent.click(screen.getByRole("button", { name: "Close Compare" }));
    expect(onCloseTab).toHaveBeenCalled();

    expect(screen.getByText("footer:panel-a")).toBeInTheDocument();
    getBoundingClientRect.mockRestore();
  });

  test("hides the tab list for a single tab and flattens a solitary panel", () => {
    const { container } = render(
      <ConsoleDock
        Icon={Icon}
        onCreateTab={() => undefined}
        renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
        renderPanelFooter={(panel) => <div>{`footer:${panel.id}`}</div>}
        tabActions={<span>Layouts</span>}
        viewState={{
          activeTabId: "focus",
          focusedPanelId: "panel-a",
          panels: [
            { id: "panel-a", title: "Panel A", subtitle: "workspace" },
          ],
          tabs: [
            {
              id: "focus",
              title: "Focus",
              subtitle: "workspace",
              layout: { kind: "panel", panelId: "panel-a" },
            },
          ],
        }}
      />,
    );

    expect(screen.getByRole("tablist", { name: "Dock tabs" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /Focus/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New tab" })).toBeInTheDocument();
    expect(container.querySelector(".cc-dock-panel.is-solitary")).toBeTruthy();
    expect(container.querySelector(".cc-dock-panel__header")).toBeNull();
    // A solitary panel renders no floating split arrows — they used to hover
    // over the first right-aligned user message. Layout changes go through the
    // tab-bar layout control instead.
    expect(container.querySelector(".cc-dock-panel__floating-actions")).toBeNull();
  });

  test("cleans global resize state when a drag is interrupted by unmount", () => {
    const getBoundingClientRect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({
      width: 400,
      height: 280,
      top: 20,
      left: 40,
      right: 440,
      bottom: 300,
      x: 40,
      y: 20,
      toJSON: () => ({}),
    }));

    const { unmount } = render(
      <ConsoleDock
        Icon={Icon}
        onResizeSplit={() => undefined}
        renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
        viewState={{
          activeTabId: "compare",
          focusedPanelId: "panel-a",
          panels: [
            { id: "panel-a", title: "Panel A", subtitle: "workspace" },
            { id: "panel-b", title: "Panel B", subtitle: "workspace", closable: true },
          ],
          tabs: [
            {
              id: "compare",
              title: "Compare",
              layout: {
                kind: "split",
                id: "compare-layout",
                direction: "horizontal",
                first: { kind: "panel", panelId: "panel-a" },
                second: { kind: "panel", panelId: "panel-b" },
              },
            },
          ],
        }}
      />,
    );

    fireEvent.pointerDown(screen.getByRole("button", { name: "Resize horizontal split" }), { clientX: 240, clientY: 40 });
    expect(document.documentElement.dataset.ccResizing).toBe("true");

    unmount();
    expect(document.documentElement.dataset.ccResizing).toBeUndefined();
    getBoundingClientRect.mockRestore();
  });

  test("keeps resize mode active until every active split drag has cleaned up", () => {
    const getBoundingClientRect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({
      width: 400,
      height: 280,
      top: 20,
      left: 40,
      right: 440,
      bottom: 300,
      x: 40,
      y: 20,
      toJSON: () => ({}),
    }));

    render(
      <>
        <ConsoleDock
          Icon={Icon}
          onResizeSplit={() => undefined}
          renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
          viewState={{
            activeTabId: "compare-a",
            focusedPanelId: "panel-a",
            panels: [
              { id: "panel-a", title: "Panel A", subtitle: "workspace" },
              { id: "panel-b", title: "Panel B", subtitle: "workspace", closable: true },
            ],
            tabs: [
              {
                id: "compare-a",
                title: "Compare A",
                layout: {
                  kind: "split",
                  id: "compare-layout-a",
                  direction: "horizontal",
                  first: { kind: "panel", panelId: "panel-a" },
                  second: { kind: "panel", panelId: "panel-b" },
                },
              },
            ],
          }}
        />
        <ConsoleDock
          Icon={Icon}
          onResizeSplit={() => undefined}
          renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
          viewState={{
            activeTabId: "compare-b",
            focusedPanelId: "panel-c",
            panels: [
              { id: "panel-c", title: "Panel C", subtitle: "workspace" },
              { id: "panel-d", title: "Panel D", subtitle: "workspace", closable: true },
            ],
            tabs: [
              {
                id: "compare-b",
                title: "Compare B",
                layout: {
                  kind: "split",
                  id: "compare-layout-b",
                  direction: "horizontal",
                  first: { kind: "panel", panelId: "panel-c" },
                  second: { kind: "panel", panelId: "panel-d" },
                },
              },
            ],
          }}
        />
      </>,
    );

    const [firstDivider, secondDivider] = screen.getAllByRole("button", { name: "Resize horizontal split" });
    fireEvent.pointerDown(firstDivider!, { clientX: 200, clientY: 40, pointerId: 1 });
    fireEvent.pointerDown(secondDivider!, { clientX: 220, clientY: 40, pointerId: 2 });

    expect(document.documentElement.dataset.ccResizeLockCount).toBe("2");
    expect(document.documentElement.dataset.ccResizing).toBe("true");

    fireEvent(firstDivider!, new Event("lostpointercapture"));
    expect(document.documentElement.dataset.ccResizeLockCount).toBe("1");
    expect(document.documentElement.dataset.ccResizing).toBe("true");

    fireEvent(secondDivider!, new Event("lostpointercapture"));
    expect(document.documentElement.dataset.ccResizeLockCount).toBeUndefined();
    expect(document.documentElement.dataset.ccResizing).toBeUndefined();
    getBoundingClientRect.mockRestore();
  });

  test("ignores pointerup from a different active drag", () => {
    const getBoundingClientRect = vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({
      width: 400,
      height: 280,
      top: 20,
      left: 40,
      right: 440,
      bottom: 300,
      x: 40,
      y: 20,
      toJSON: () => ({}),
    }));

    render(
      <>
        <ConsoleDock
          Icon={Icon}
          onResizeSplit={() => undefined}
          renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
          viewState={{
            activeTabId: "compare-a",
            focusedPanelId: "panel-a",
            panels: [
              { id: "panel-a", title: "Panel A", subtitle: "workspace" },
              { id: "panel-b", title: "Panel B", subtitle: "workspace", closable: true },
            ],
            tabs: [
              {
                id: "compare-a",
                title: "Compare A",
                layout: {
                  kind: "split",
                  id: "compare-layout-a",
                  direction: "horizontal",
                  first: { kind: "panel", panelId: "panel-a" },
                  second: { kind: "panel", panelId: "panel-b" },
                },
              },
            ],
          }}
        />
        <ConsoleDock
          Icon={Icon}
          onResizeSplit={() => undefined}
          renderPanelBody={(panel) => <div>{`body:${panel.id}`}</div>}
          viewState={{
            activeTabId: "compare-b",
            focusedPanelId: "panel-c",
            panels: [
              { id: "panel-c", title: "Panel C", subtitle: "workspace" },
              { id: "panel-d", title: "Panel D", subtitle: "workspace", closable: true },
            ],
            tabs: [
              {
                id: "compare-b",
                title: "Compare B",
                layout: {
                  kind: "split",
                  id: "compare-layout-b",
                  direction: "horizontal",
                  first: { kind: "panel", panelId: "panel-c" },
                  second: { kind: "panel", panelId: "panel-d" },
                },
              },
            ],
          }}
        />
      </>,
    );

    const [firstDivider, secondDivider] = screen.getAllByRole("button", { name: "Resize horizontal split" });
    fireEvent.pointerDown(firstDivider!, { clientX: 200, clientY: 40, pointerId: 1 });
    fireEvent.pointerDown(secondDivider!, { clientX: 220, clientY: 40, pointerId: 2 });

    fireEvent.pointerUp(window, { pointerId: 1 });

    expect(document.documentElement.dataset.ccResizeLockCount).toBe("1");
    expect(document.documentElement.dataset.ccResizing).toBe("true");

    fireEvent.pointerUp(window, { pointerId: 2 });

    expect(document.documentElement.dataset.ccResizeLockCount).toBeUndefined();
    expect(document.documentElement.dataset.ccResizing).toBeUndefined();
    getBoundingClientRect.mockRestore();
  });
});
