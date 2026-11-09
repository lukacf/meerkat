// NOTE (2026-07-14 audit): this file is written against vitest globals and
// @testing-library/react, NEITHER of which is a dependency of this repo — it
// has never executed in any CI lane or local runner. Runnable coverage for
// these behaviors lives in console/src/lib/component-interaction.test.tsx
// (repo-standard esbuild + node --test) and, for the pure grouping/parsing
// logic, in packages/console-core/src/*.test.ts via the node:test-backed
// shim (test-support/vitest-shim.ts). If you add vitest + RTL as real
// dependencies, wire this file into CI before trusting it.
import { fireEvent, render, screen } from "@testing-library/react";

import { ConsoleSidebar } from "./console-sidebar";

function Icon({ name }: { name: string; className?: string }) {
  return (
    <svg>
      <use href={`#${name}`} />
    </svg>
  );
}

describe("ConsoleSidebar", () => {
  test("does not emit React key warnings for multi-action strips", () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    render(
      <ConsoleSidebar
        Icon={Icon}
        viewState={{
          blocks: [{
            id: "primary",
            kind: "action_strip",
            actions: [
              { id: "new_thread", label: "New thread", iconName: "i-new-thread" },
              { id: "automations", label: "Automations", iconName: "i-clock" },
              { id: "skills", label: "Skills", iconName: "i-cube" },
            ],
          }],
        }}
      />,
    );

    expect(errorSpy).not.toHaveBeenCalled();
    errorSpy.mockRestore();
  });

  test("renders action strips, section headers, row actions, and trailing content", () => {
    const onBlockAction = vi.fn();
    const onSelectSection = vi.fn();
    const onSectionAction = vi.fn();
    const onSelectItem = vi.fn();
    const onItemAction = vi.fn();

    render(
      <ConsoleSidebar
        Icon={Icon}
        onBlockAction={onBlockAction}
        onItemAction={onItemAction}
        onSelectItem={onSelectItem}
        onSectionAction={onSectionAction}
        onSelectSection={onSelectSection}
        renderItemTrailing={({ item }) => <span>{`archive:${item.id}`}</span>}
        viewState={{
          blocks: [
            {
              id: "primary",
              kind: "action_strip",
              actions: [{ id: "new_thread", label: "New thread", iconName: "i-new-thread" }],
            },
            {
              id: "threads",
              kind: "list",
              title: "Threads",
              actions: [{ id: "filter_sort", label: "Filter and sort", iconName: "i-sliders" }],
              sections: [{
                id: "workspace",
                title: "workspace",
                iconName: "i-folder",
                actions: [{ id: "create_thread", label: "New thread", iconName: "i-plus" }],
                items: [{
                  id: "thread-1",
                  title: "Extract the console sidebar",
                  unread: true,
                  selected: true,
                  badgeIconName: "i-open",
                  actions: [{ id: "pin", label: "Pin thread", iconName: "i-pin" }],
                }],
              }],
            },
          ],
        }}
      />,
    );

    fireEvent.click(screen.getAllByRole("button", { name: "New thread" })[0] as HTMLElement);
    expect(onBlockAction).toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "workspace" }));
    expect(onSelectSection).toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Pin thread" }));
    expect(onItemAction).toHaveBeenCalled();

    fireEvent.click(screen.getByText("Extract the console sidebar"));
    expect(onSelectItem).toHaveBeenCalled();

    expect(screen.getByText("archive:thread-1")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Extract the console sidebar/ }))
      .toBeInTheDocument();
  });

  test("emits optional item drag and drop callbacks", () => {
    const onItemDragStart = vi.fn();
    const onItemDrop = vi.fn();
    const data = new Map<string, string>();
    const dataTransfer = {
      effectAllowed: "",
      dropEffect: "",
      setData: vi.fn((key: string, value: string) => data.set(key, value)),
      getData: vi.fn((key: string) => data.get(key) || ""),
    };

    render(
      <ConsoleSidebar
        isItemDraggable={(_block, _section, item) => item.id === "thread-a"}
        isItemDropTarget={(_block, _section, item) => item.id === "thread-b"}
        onItemDragStart={onItemDragStart}
        onItemDrop={onItemDrop}
        viewState={{
          blocks: [{
            id: "threads",
            kind: "list",
            sections: [{
              id: "workspace",
              title: "workspace",
              items: [
                { id: "thread-a", title: "Thread A" },
                { id: "thread-b", title: "Thread B" },
              ],
            }],
          }],
        }}
      />,
    );

    const source = screen.getByRole("button", { name: "Thread A" });
    const target = screen.getByRole("button", { name: "Thread B" });

    expect(source).toHaveAttribute("draggable", "true");
    fireEvent.dragStart(source, { dataTransfer });
    expect(data.get("application/x-console-sidebar-item-id")).toBe("thread-a");
    fireEvent.dragOver(target, { dataTransfer });
    fireEvent.drop(target, { dataTransfer });

    expect(onItemDragStart).toHaveBeenCalled();
    expect(onItemDrop).toHaveBeenCalled();
  });

  test("lets consumers add semantic item groups without rebuilding interactive rows", () => {
    const onSelectItem = vi.fn();

    render(
      <ConsoleSidebar
        onSelectItem={onSelectItem}
        renderSectionItems={({ section, defaultItems }) => {
          const agents = defaultItems.filter((_row, index) => section.items[index]?.id.startsWith("agent:"));
          const threads = defaultItems.filter((_row, index) => !section.items[index]?.id.startsWith("agent:"));
          return (
            <>
              <div aria-labelledby="agents-heading" role="group">
                <h3 id="agents-heading">Agents</h3>
                {agents}
              </div>
              <div aria-labelledby="threads-heading" role="group">
                <h3 id="threads-heading">Threads</h3>
                {threads}
              </div>
            </>
          );
        }}
        viewState={{
          blocks: [{
            id: "projects",
            kind: "list",
            sections: [{
              id: "workspace",
              title: "workspace",
              items: [
                { id: "agent:desktop", title: "Desktop UI" },
                { id: "thread-1", title: "Polish the sidebar" },
              ],
            }],
          }],
        }}
      />,
    );

    expect(screen.getByRole("group", { name: "Agents" })).toContainElement(
      screen.getByRole("button", { name: "Desktop UI" }),
    );
    expect(screen.getByRole("group", { name: "Threads" })).toContainElement(
      screen.getByRole("button", { name: "Polish the sidebar" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Desktop UI" }));
    expect(onSelectItem).toHaveBeenCalledWith(
      expect.objectContaining({ id: "projects" }),
      expect.objectContaining({ id: "workspace" }),
      expect.objectContaining({ id: "agent:desktop" }),
    );
  });
});
