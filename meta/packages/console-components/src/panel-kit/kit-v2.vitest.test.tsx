import { act, fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";

import {
  Chip,
  IconButton,
  OverflowMenu,
  SearchField,
  SelectField,
  StatusDot,
  StatusMark,
  StatusPill,
  ViewOptionsMenu,
  type MenuEntry,
} from "./index";

// Console v2 kit parts (spec A3). DOM assertions compare booleans and
// strings, never elements: a failing element comparison walks React
// internals and can exhaust memory.

describe("status marks", () => {
  it("draws the tone's shape, decorative beside a word", () => {
    const view = render(<StatusMark tone="warn" />);
    const mark = view.container.querySelector(".cc-mark");
    expect(mark?.getAttribute("data-tone")).toBe("warn");
    expect(mark?.getAttribute("aria-hidden")).toBe("true");
    expect(mark?.hasAttribute("role")).toBe(false);
  });

  it("stands alone as an image named by its label", () => {
    const view = render(<StatusMark tone="live" label="Working now" size="md" />);
    const mark = view.getByRole("img", { name: "Working now" });
    expect(mark.getAttribute("data-size")).toBe("md");
    expect(mark.getAttribute("title")).toBe("Working now");
  });

  it("puts the mark before the word in a status pill", () => {
    const view = render(<StatusPill tone="crit" testId="pill">Stalled</StatusPill>);
    const pill = view.getByTestId("pill");
    expect(pill.className).toBe("cc-chip cc-chip--mark");
    expect(pill.getAttribute("data-tone")).toBe("crit");
    expect(pill.firstElementChild?.className).toBe("cc-mark cc-chip__mark");
    expect(pill.textContent).toBe("Stalled");
  });

  it("keeps the older chip and dot markup", () => {
    const chip = render(<Chip tone="ok" dot outline>Allowed</Chip>).container.innerHTML;
    expect(chip).toBe('<span class="cc-chip cc-chip--outline" data-tone="ok"><span class="cc-chip__dot" aria-hidden="true"></span>Allowed</span>');
    const dot = render(<StatusDot tone="info" />).container.innerHTML;
    expect(dot).toBe('<span class="cc-dot" data-tone="info" aria-hidden="true"></span>');
  });

  it("offers a quiet chip for dense rows", () => {
    const view = render(<Chip tone="warn" mark variant="quiet">Paused</Chip>);
    expect(view.container.firstElementChild?.className).toBe("cc-chip cc-chip--quiet cc-chip--mark");
  });
});

describe("icon buttons", () => {
  it("names itself by its label and shows it as the tooltip", () => {
    const onClick = vi.fn();
    const view = render(<IconButton label="Refresh approvals" glyph="refresh" onClick={onClick} data-testid="refresh" />);
    const button = view.getByRole("button", { name: "Refresh approvals" });
    expect(button.getAttribute("title")).toBe("Refresh approvals");
    expect(button.className).toBe("cc-btn cc-btn--icon cc-btn--ghost");
    expect(button.querySelector('svg[data-glyph="refresh"]') !== null).toBe(true);
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("carries the reveal, quiet and badge states as classes", () => {
    const view = render(<IconButton label="Roster view options" glyph="ellipsis" size="sm" quiet reveal badge />);
    const button = view.getByRole("button", { name: "Roster view options" });
    expect(button.className).toBe("cc-btn cc-btn--icon cc-btn--ghost cc-btn--sm cc-btn--quiet cc-reveal has-badge");
    expect(button.querySelector(".cc-btn__badge") !== null).toBe(true);
  });
});

function MenuFixture({ onRefresh, onDensity }: { onRefresh: () => void; onDensity: (id: string) => void }) {
  const [density, setDensity] = useState("comfortable");
  const [ids, setIds] = useState(false);
  const items: MenuEntry[] = [
    {
      kind: "radio",
      id: "density",
      label: "Density",
      value: density,
      options: [
        { id: "comfortable", label: "Comfortable", testId: "menu-density:comfortable" },
        { id: "compact", label: "Compact", testId: "menu-density:compact" },
      ],
      onChange: (id) => {
        setDensity(id);
        onDensity(id);
      },
    },
    { kind: "checkbox", id: "ids", label: "Show technical IDs", checked: ids, onChange: setIds, testId: "menu-ids" },
    { kind: "separator" },
    { kind: "item", id: "refresh", label: "Refresh now", onSelect: onRefresh, testId: "menu-refresh" },
  ];
  return (
    <div>
      <OverflowMenu label="More actions for Topology" items={items} triggerTestId="menu-trigger" testId="menu" />
      <button type="button">After</button>
    </div>
  );
}

describe("overflow menu", () => {
  it("opens from its trigger on the first item and names itself by the trigger", () => {
    const view = render(<MenuFixture onRefresh={() => {}} onDensity={() => {}} />);
    const trigger = view.getByTestId("menu-trigger");
    expect(trigger.getAttribute("aria-haspopup")).toBe("menu");
    expect(trigger.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(trigger);
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    const menu = view.getByRole("menu");
    expect(menu.getAttribute("aria-labelledby")).toBe(trigger.id);
    expect(trigger.getAttribute("aria-controls")).toBe(menu.id);
    expect(document.activeElement === view.getByTestId("menu-density:comfortable")).toBe(true);
    expect(view.getByRole("group", { name: "Density" }).textContent?.startsWith("Density")).toBe(true);
  });

  it("moves with the arrow keys, wraps, and finds items by their first letter", () => {
    const view = render(<MenuFixture onRefresh={() => {}} onDensity={() => {}} />);
    fireEvent.keyDown(view.getByTestId("menu-trigger"), { key: "ArrowUp" });
    expect(document.activeElement === view.getByTestId("menu-refresh")).toBe(true);
    fireEvent.keyDown(document.activeElement as Element, { key: "ArrowDown" });
    expect(document.activeElement === view.getByTestId("menu-density:comfortable")).toBe(true);
    fireEvent.keyDown(document.activeElement as Element, { key: "End" });
    expect(document.activeElement === view.getByTestId("menu-refresh")).toBe(true);
    fireEvent.keyDown(document.activeElement as Element, { key: "s" });
    expect(document.activeElement === view.getByTestId("menu-ids")).toBe(true);
  });

  it("keeps the menu open for options and closes it for commands", () => {
    const onRefresh = vi.fn();
    const onDensity = vi.fn();
    const view = render(<MenuFixture onRefresh={onRefresh} onDensity={onDensity} />);
    fireEvent.click(view.getByTestId("menu-trigger"));
    fireEvent.click(view.getByTestId("menu-density:compact"));
    expect(onDensity).toHaveBeenCalledWith("compact");
    expect(view.getByTestId("menu-density:compact").getAttribute("aria-checked")).toBe("true");
    fireEvent.click(view.getByTestId("menu-ids"));
    expect(view.getByTestId("menu-ids").getAttribute("aria-checked")).toBe("true");
    fireEvent.click(view.getByTestId("menu-refresh"));
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(view.queryByRole("menu") === null).toBe(true);
    expect(document.activeElement === view.getByTestId("menu-trigger")).toBe(true);
  });

  it("closes on Escape with focus back on the trigger, and on a press outside", () => {
    const view = render(<MenuFixture onRefresh={() => {}} onDensity={() => {}} />);
    fireEvent.click(view.getByTestId("menu-trigger"));
    fireEvent.keyDown(document.activeElement as Element, { key: "Escape" });
    expect(view.queryByRole("menu") === null).toBe(true);
    expect(document.activeElement === view.getByTestId("menu-trigger")).toBe(true);
    fireEvent.click(view.getByTestId("menu-trigger"));
    fireEvent.pointerDown(view.getByRole("button", { name: "After" }));
    expect(view.queryByRole("menu") === null).toBe(true);
  });
});

function ViewFixture() {
  const [sort, setSort] = useState("manual");
  return (
    <ViewOptionsMenu
      label="Roster view options"
      ariaLabel="Roster view"
      triggerTestId="view-trigger"
      resetTestId="view-reset"
      isDefault={sort === "manual"}
      onReset={() => setSort("manual")}
      controls={[
        {
          label: "Sort by",
          options: [
            { id: "manual", label: "Manual", testId: "view-sort:manual" },
            { id: "name", label: "Name", testId: "view-sort:name" },
          ],
          value: sort,
          onChange: setSort,
          testId: "view-sort",
        },
      ]}
    />
  );
}

describe("view options", () => {
  it("opens a non-modal dialog on the pressed option and applies changes at once", () => {
    const view = render(<ViewFixture />);
    const trigger = view.getByTestId("view-trigger");
    expect(trigger.getAttribute("aria-haspopup")).toBe("dialog");
    expect(trigger.querySelector(".cc-btn__badge") === null).toBe(true);
    fireEvent.click(trigger);
    const dialog = view.getByRole("dialog", { name: "Roster view" });
    expect(dialog.getAttribute("aria-modal")).toBe("false");
    expect(document.activeElement === view.getByTestId("view-sort:manual")).toBe(true);
    expect((view.getByTestId("view-reset") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(view.getByTestId("view-sort:name"));
    expect(view.getByTestId("view-sort:name").getAttribute("aria-pressed")).toBe("true");
    expect(view.queryByRole("dialog") !== null).toBe(true);
    expect((view.getByTestId("view-reset") as HTMLButtonElement).disabled).toBe(false);
    expect(trigger.querySelector(".cc-btn__badge") !== null).toBe(true);
    fireEvent.keyDown(document.activeElement as Element, { key: "Escape" });
    expect(view.queryByRole("dialog") === null).toBe(true);
    expect(document.activeElement === trigger).toBe(true);
  });

  it("resets the view from its footer", () => {
    const view = render(<ViewFixture />);
    fireEvent.click(view.getByTestId("view-trigger"));
    fireEvent.click(view.getByTestId("view-sort:name"));
    act(() => {
      fireEvent.click(view.getByTestId("view-reset"));
    });
    expect(view.getByTestId("view-sort:manual").getAttribute("aria-pressed")).toBe("true");
  });
});

describe("fields", () => {
  it("renders the scope chip as the native select with its accessible name", () => {
    const view = render(
      <SelectField
        label="Memory team"
        showLabel={false}
        variant="chip"
        value="ops"
        options={[{ value: "ops", label: "Operations team" }]}
        onChange={() => {}}
      />,
    );
    const select = view.getByRole("combobox", { name: "Memory team" });
    expect(select.closest(".cc-field--chip") !== null).toBe(true);
  });

  it("shows the search shortcut only while the search is empty", () => {
    const view = render(<SearchField label="Search agents" value="" onChange={() => {}} shortcut="/" />);
    expect(view.container.querySelector(".cc-search__key")?.textContent).toBe("/");
    view.rerender(<SearchField label="Search agents" value="bil" onChange={() => {}} shortcut="/" />);
    expect(view.container.querySelector(".cc-search__key") === null).toBe(true);
  });
});
