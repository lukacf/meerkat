import { act, fireEvent, render } from "@testing-library/react";
import { useRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";

import {
  AgentAvatar,
  AgentName,
  ConsoleNavigationProvider,
  Countdown,
  EntityCard,
  EntityLink,
  EntityRow,
  FilterBar,
  NewItemsPill,
  RelativeTimeChip,
  TrustLadder,
  countdownTone,
  formatCountdown,
  Button,
  Chip,
  DetailDrawer,
  DrawerLayout,
  DrawerSection,
  EmptyIllustration,
  EmptyState,
  LoadingState,
  Notice,
  TechnicalDetails,
  IconButton,
  OverflowMenu,
  PanelDensityDefaultProvider,
  PanelHeader,
  PanelPreferencesProvider,
  PanelShell,
  Popover,
  SearchField,
  SelectField,
  StatusDot,
  StatusMark,
  StatusPill,
  StatusRow,
  ViewOptionsMenu,
  standardPanelMenu,
  usePanelDensity,
  usePanelOptions,
  useShowTechnicalIds,
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

function PopoverProbe({ onClose, onFocusMove }: { onClose: (reason: string) => void; onFocusMove?: (popover: HTMLElement) => void }) {
  const anchorRef = useRef<HTMLButtonElement | null>(null);
  return (
    <>
      <button type="button" ref={anchorRef}>
        Anchor
      </button>
      <Popover
        open
        anchorRef={anchorRef}
        onClose={onClose}
        ariaLabel="Probe"
        initialFocus={(popover) => {
          onFocusMove?.(popover);
          return popover.querySelector<HTMLElement>("button");
        }}
      >
        <button type="button">Inside</button>
      </Popover>
    </>
  );
}

describe("popover", () => {
  it("can take focus while it is placed: transparent, never visibility hidden", () => {
    // Browsers skip focus inside a visibility: hidden subtree (jsdom does
    // not), so a popover hidden that way kept focus on its trigger and
    // Escape never reached it. The style itself is pinned here.
    const seen: string[] = [];
    const view = render(<PopoverProbe onClose={() => {}} onFocusMove={(popover) => seen.push(popover.style.visibility)} />);
    expect(seen).toEqual([""]);
    expect(document.activeElement === view.getByRole("button", { name: "Inside" })).toBe(true);
  });

  it("closes when a scroll takes its trigger out of sight", () => {
    const onClose = vi.fn();
    const view = render(<PopoverProbe onClose={onClose} />);
    const anchor = view.getByRole("button", { name: "Anchor" });
    fireEvent.scroll(window);
    expect(onClose).not.toHaveBeenCalled();
    anchor.getBoundingClientRect = () =>
      ({ top: -120, bottom: -92, left: 10, right: 38, width: 28, height: 28, x: 10, y: -120, toJSON: () => ({}) }) as DOMRect;
    fireEvent.scroll(window);
    expect(onClose).toHaveBeenCalledWith("outside");
    expect(document.activeElement === anchor).toBe(true);
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

const NOW_MS = Date.UTC(2026, 9, 30, 12, 0, 0);

describe("panel header v2", () => {
  it("renders every older header byte for byte", () => {
    const html = render(<PanelHeader title="Memory" subtitle="12 records" actions={<Button>Refresh</Button>} className="memory-head" />).container.innerHTML;
    expect(html).toBe(
      '<header class="cc-panel-head memory-head"><div class="cc-panel-head__text"><h2 class="cc-panel-head__title">Memory</h2>'
        + '<div class="cc-panel-head__sub">12 records</div></div><div class="cc-panel-head__actions"><button type="button" class="cc-btn">Refresh</button></div></header>',
    );
  });

  it("shows the live summary with toned facts and the freshness last", () => {
    const view = render(
      <PanelHeader
        title="Topology"
        summary={[
          { key: "agents", text: "8 agents" },
          { key: "live", text: "1 working now", tone: "live" },
          { key: "warn", text: "1 degraded", tone: "warn", testId: "summary-warn" },
        ]}
        freshness={{ at: NOW_MS - 12_000, title: "Snapshot" }}
        summaryTestId="summary"
      />,
    );
    const summary = view.getByTestId("summary");
    expect(summary.className).toBe("cc-panel-head__sub cc-panel-head__summary");
    expect(summary.textContent?.startsWith("8 agents · 1 working now · 1 degraded · Updated")).toBe(true);
    const warn = view.getByTestId("summary-warn");
    expect(warn.getAttribute("data-tone")).toBe("warn");
    expect(warn.querySelector('.cc-mark[data-tone="warn"]') !== null).toBe(true);
    expect(view.container.querySelector("time")?.getAttribute("title")).toBe("Snapshot");
    view.rerender(<PanelHeader title="Topology" summary={[{ text: "8 agents" }]} freshness={{ updating: true }} refreshing summaryTestId="summary" />);
    expect(view.getByTestId("summary").textContent).toBe("8 agents · Updating");
    expect(view.container.querySelector(".cc-panel-head__progress") !== null).toBe(true);
  });

  it("announces count changes politely, never the first render", () => {
    vi.useFakeTimers();
    try {
      const view = render(<PanelHeader title="Roster" summary={[{ text: "8 agents" }]} />);
      const live = view.container.querySelector('[aria-live="polite"]');
      expect(live?.textContent).toBe("");
      view.rerender(<PanelHeader title="Roster" summary={[{ text: "9 agents" }]} />);
      expect(live?.textContent).toBe("9 agents");
      view.rerender(<PanelHeader title="Roster" summary={[{ text: "10 agents" }]} />);
      expect(live?.textContent).toBe("9 agents");
      act(() => {
        vi.advanceTimersByTime(30_000);
      });
      expect(live?.textContent).toBe("10 agents");
    } finally {
      vi.useRealTimers();
    }
  });

  it("puts the view switch, tools and the overflow menu on the right", () => {
    const onChange = vi.fn();
    const view = render(
      <PanelHeader
        title="WorkGraph"
        scope={<SelectField label="Work Graph team" showLabel={false} variant="chip" value="ops" options={[{ value: "ops", label: "Operations team" }]} onChange={() => {}} />}
        view={{
          ariaLabel: "WorkGraph view",
          value: "graph",
          onChange,
          options: [
            { id: "graph", label: "Graph", glyph: "graph", testId: "workgraph-view-toggle:graph" },
            { id: "tree", label: "Outline", glyph: "tree", testId: "workgraph-view-toggle:tree" },
          ],
        }}
        tools={<IconButton label="Refresh work items" glyph="refresh" />}
        menu={{ label: "More actions for WorkGraph", items: [{ kind: "item", id: "refresh", label: "Refresh now", onSelect: () => {} }], triggerTestId: "workgraph-menu" }}
      />,
    );
    expect(view.getByRole("heading", { level: 2 }).outerHTML).toBe('<h2 class="cc-panel-head__title">WorkGraph</h2>');
    expect(view.getByRole("combobox", { name: "Work Graph team" }) !== null).toBe(true);
    const graph = view.getByTestId("workgraph-view-toggle:graph");
    expect(graph.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(view.getByTestId("workgraph-view-toggle:tree"));
    expect(onChange).toHaveBeenCalledWith("tree");
    expect(view.getByRole("button", { name: "Refresh work items" }).closest(".cc-reveal") !== null).toBe(true);
    // The switch stays in the header at every width (pinned scenarios press
    // it in a 180 px pane): no folded copy in the menu. Its words stay the
    // names and become tooltips, for the narrow pane's glyph-only buttons.
    expect(graph.getAttribute("title")).toBe("Graph");
    expect(graph.querySelector(".cc-seg__label")?.textContent).toBe("Graph");
    expect(view.getByRole("button", { name: "Outline" }).getAttribute("data-testid")).toBe("workgraph-view-toggle:tree");
    fireEvent.click(view.getByTestId("workgraph-menu"));
    expect(view.getByRole("menu").querySelector('[role="menuitemradio"]') === null).toBe(true);
    expect(view.getAllByTestId("workgraph-view-toggle:graph").length).toBe(1);
    // A view without a menu adds no overflow trigger.
    view.rerender(
      <PanelHeader
        title="WorkGraph"
        view={{ ariaLabel: "WorkGraph view", value: "graph", onChange, options: [{ id: "graph", label: "Graph", glyph: "graph" }, { id: "tree", label: "Outline" }] }}
      />,
    );
    expect(view.container.querySelector(".cc-menu-trigger") === null).toBe(true);
    // An option without a glyph keeps its plain words.
    expect(view.getByRole("button", { name: "Outline" }).innerHTML).toBe("Outline");
  });

  it("wraps its actions to their own row instead of folding or overflowing a narrow pane", async () => {
    const fs = await import("node:fs");
    const path = await import("node:path");
    const css = fs.readFileSync(path.join(__dirname, "../styles/panel-kit.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    const block = (start: number) => {
      let depth = 0;
      for (let index = css.indexOf("{", start); index < css.length; index += 1) {
        if (css[index] === "{") depth += 1;
        else if (css[index] === "}" && --depth === 0) return css.slice(start, index + 1);
      }
      return "";
    };
    const narrow = block(css.indexOf("@container cc-panel (max-width: 479px)"));
    const rule = (source: string, selector: string) => {
      const match = new RegExp(`(^|[{}])\\s*${selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s*\\{([^}]*)\\}`).exec(source);
      return match ? match[2] : "";
    };
    // Everywhere: the v2 header wraps, the title keeps 240 px before the actions move down.
    expect(rule(css, ".cc-panel-head--v2")).toMatch(/flex-wrap:\s*wrap/);
    expect(rule(css, ".cc-panel-head--v2 .cc-panel-head__text")).toMatch(/flex:\s*1 1 240px/);
    // Under 480 px: the actions take a full row and wrap inside it; the switch is never hidden.
    expect(narrow.length > 0).toBe(true);
    expect(rule(narrow, ".cc-panel-head__actions")).toMatch(/flex-basis:\s*100%/);
    expect(rule(narrow, ".cc-panel-head--v2 .cc-panel-head__actions")).toMatch(/flex-wrap:\s*wrap/);
    expect(/\.cc-panel-head__view\s*\{[^}]*display:\s*none/.test(narrow)).toBe(false);
    // Glyph-only options hide their words visually, never from the name.
    const label = rule(narrow, ".cc-panel-head__view .cc-seg__btn.has-glyph .cc-seg__label");
    expect(label).toMatch(/clip-path:\s*inset\(50%\)/);
    expect(/display:\s*none|visibility:\s*hidden/.test(label)).toBe(false);
  });
});

describe("status toast", () => {
  it("keeps the reserved row by default", () => {
    const html = render(<StatusRow error="Memory is unavailable" errorTestId="memory-error" />).container.innerHTML;
    expect(html).toBe('<div class="cc-status-row"><span class="cc-status-row__error" role="alert" title="Memory is unavailable" data-testid="memory-error">Memory is unavailable</span></div>');
  });

  it("overlays the error without reserving space, with the data age and Retry", () => {
    const onRetry = vi.fn();
    const view = render(<StatusRow mode="toast" updatedAt={NOW_MS - 180_000} onRetry={onRetry} />);
    expect(view.container.innerHTML).toBe("");
    view.rerender(<StatusRow mode="toast" error="Couldn't refresh." errorTestId="gating-audit-error" updatedAt={NOW_MS - 180_000} onRetry={onRetry} />);
    const alert = view.getByRole("alert");
    expect(alert.getAttribute("data-testid")).toBe("gating-audit-error");
    expect(alert.textContent).toBe("Couldn't refresh.");
    expect(view.container.querySelector(".cc-status-toast__stale")?.textContent?.startsWith(" Showing data from ")).toBe(true);
    fireEvent.click(view.getByRole("button", { name: "Retry" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });
});

function DensityProbe({ kind, testId }: { kind: string; testId: string }) {
  const [density, setDensity] = usePanelDensity(kind);
  const [ids, setIds] = useShowTechnicalIds(kind);
  return (
    <button type="button" data-testid={testId} data-ids={String(ids)} onClick={() => { setDensity(density === "compact" ? "comfortable" : "compact"); setIds(!ids); }}>
      {density}
    </button>
  );
}

function OptionsProbe() {
  const options = usePanelOptions();
  return <span data-testid="options">{`${options.density} ${options.showTechnicalIds}`}</span>;
}

describe("panel preferences", () => {
  it("persists density and technical ids per kind under the preference scope and keeps panels in step", () => {
    window.localStorage.clear();
    const view = render(
      <PanelPreferencesProvider scope="principal-1">
        <DensityProbe kind="topology" testId="a" />
        <DensityProbe kind="topology" testId="b" />
        <DensityProbe kind="memory" testId="c" />
      </PanelPreferencesProvider>,
    );
    expect(view.getByTestId("a").textContent).toBe("comfortable");
    act(() => {
      fireEvent.click(view.getByTestId("a"));
    });
    expect(window.localStorage.getItem("meta-console-density:topology:principal-1")).toBe("compact");
    expect(window.localStorage.getItem("meta-console-tech-ids:topology:principal-1")).toBe("on");
    expect(view.getByTestId("b").textContent).toBe("compact");
    expect(view.getByTestId("b").getAttribute("data-ids")).toBe("true");
    expect(view.getByTestId("c").textContent).toBe("comfortable");
    act(() => {
      fireEvent.click(view.getByTestId("b"));
    });
    // The default is not stored.
    expect(window.localStorage.getItem("meta-console-density:topology:principal-1") === null).toBe(true);
  });

  it("follows the display density while a kind has no choice of its own", () => {
    window.localStorage.clear();
    const tree = (density: "comfortable" | "compact") => (
      <PanelPreferencesProvider scope="principal-1">
        <PanelDensityDefaultProvider density={density}>
          <DensityProbe kind="topology" testId="a" />
          <DensityProbe kind="memory" testId="b" />
        </PanelDensityDefaultProvider>
      </PanelPreferencesProvider>
    );
    const view = render(tree("compact"));
    expect(view.getByTestId("a").textContent).toBe("compact");
    expect(view.getByTestId("b").textContent).toBe("compact");
    view.rerender(tree("comfortable"));
    expect(view.getByTestId("a").textContent).toBe("comfortable");
    expect(view.getByTestId("b").textContent).toBe("comfortable");
    // Nothing is written for a kind that only follows the default.
    expect(window.localStorage.getItem("meta-console-density:topology:principal-1") === null).toBe(true);
    // Without a provider the default is comfortable.
    const bare = render(<DensityProbe kind="workgraph" testId="c" />);
    expect(bare.getByTestId("c").textContent).toBe("comfortable");
  });

  it("keeps a kind's stored choice over the display density, and clears it on the default", () => {
    window.localStorage.clear();
    window.localStorage.setItem("meta-console-density:topology:principal-1", "comfortable");
    const tree = (density: "comfortable" | "compact") => (
      <PanelPreferencesProvider scope="principal-1">
        <PanelDensityDefaultProvider density={density}>
          <DensityProbe kind="topology" testId="a" />
          <DensityProbe kind="memory" testId="b" />
        </PanelDensityDefaultProvider>
      </PanelPreferencesProvider>
    );
    const view = render(tree("compact"));
    expect(view.getByTestId("a").textContent).toBe("comfortable");
    expect(view.getByTestId("b").textContent).toBe("compact");
    // Choosing the default (compact) again clears the stored choice.
    act(() => {
      fireEvent.click(view.getByTestId("a"));
    });
    expect(view.getByTestId("a").textContent).toBe("compact");
    expect(window.localStorage.getItem("meta-console-density:topology:principal-1") === null).toBe(true);
    view.rerender(tree("comfortable"));
    expect(view.getByTestId("a").textContent).toBe("comfortable");
    // A choice away from the default is stored and outlives a change of the default.
    act(() => {
      fireEvent.click(view.getByTestId("b"));
    });
    expect(window.localStorage.getItem("meta-console-density:memory:principal-1")).toBe("compact");
    view.rerender(tree("compact"));
    view.rerender(tree("comfortable"));
    expect(view.getByTestId("b").textContent).toBe("compact");
  });

  it("gives the shell's density and id option to every part inside", () => {
    const view = render(
      <PanelShell density="compact" showTechnicalIds testId="shell">
        <OptionsProbe />
      </PanelShell>,
    );
    expect(view.getByTestId("shell").getAttribute("data-density")).toBe("compact");
    expect(view.getByTestId("options").textContent).toBe("compact true");
    const plain = render(<PanelShell testId="plain" />).container.innerHTML;
    expect(plain).toBe('<section class="cc-panel" data-testid="plain"></section>');
  });

  it("builds the standard overflow menu in the spec's order", () => {
    const entries = standardPanelMenu({
      viewOptions: [{ kind: "checkbox", id: "helpers", label: "Show helpers", checked: true, onChange: () => {} }],
      density: "comfortable",
      onDensityChange: () => {},
      showTechnicalIds: false,
      onShowTechnicalIdsChange: () => {},
      onOpenInSplit: () => {},
      getCopyData: () => ({ agents: 8 }),
      onRefresh: () => {},
      testIdPrefix: "topology",
    });
    expect(entries.map((entry) => `${entry.kind}:${entry.id ?? ""}`)).toEqual([
      "checkbox:helpers",
      "radio:density",
      "checkbox:technical-ids",
      "separator:commands",
      "item:split",
      "item:copy-json",
      "item:refresh",
    ]);
    const density = entries[1];
    expect(density.kind === "radio" ? density.options.map((option) => option.testId) : []).toEqual([
      "topology-menu:density:comfortable",
      "topology-menu:density:compact",
    ]);
  });
});

function DrawerFixture() {
  const [open, setOpen] = useState(false);
  return (
    <DrawerLayout
      persistKey="topology"
      open={open}
      onClose={() => setOpen(false)}
      testId="layout"
      main={
        <button type="button" data-testid="node" onClick={() => setOpen(true)}>
          Triage
        </button>
      }
      drawer={
        <DetailDrawer
          title="Triage"
          subtitle="Coordinator · Operations team"
          status={<StatusPill tone="live">Working</StatusPill>}
          mark={<AgentAvatar name="Triage" size={28} live />}
          onClose={() => setOpen(false)}
          headingLevel={4}
          titleClassName="workgraph-graph__detail-title"
          actions={<Button variant="primary">Open chat</Button>}
          technical={[{ label: "Identity", value: "identity:triage" }]}
          testId="drawer"
        >
          <DrawerSection label="Connections" count={4} action="12 / h">
            <p>Router · wired</p>
          </DrawerSection>
        </DetailDrawer>
      }
    />
  );
}

describe("detail drawer", () => {
  it("opens with focus on its one heading and returns focus to the opener on Escape", () => {
    window.localStorage.clear();
    const view = render(<DrawerFixture />);
    const node = view.getByTestId("node");
    node.focus();
    fireEvent.click(node);
    const drawer = view.getByTestId("drawer");
    const heading = view.getByRole("heading", { level: 4, name: "Triage" });
    expect(heading.className).toBe("cc-drawer__title workgraph-graph__detail-title");
    expect(document.activeElement === heading).toBe(true);
    expect(drawer.querySelectorAll("h1, h2, h3, h4, h5, h6").length).toBe(1);
    expect(drawer.querySelector(".cc-drawer__section-label")?.textContent).toBe("Connections 4");
    expect(view.getByTestId("layout").classList.contains("has-drawer")).toBe(true);
    fireEvent.keyDown(heading, { key: "Escape" });
    expect(view.queryByTestId("drawer") === null).toBe(true);
    expect(document.activeElement === node).toBe(true);
  });

  it("names its close button and Back row for tests", () => {
    const view = render(
      <DrawerLayout
        open
        onClose={() => {}}
        backTestId="memory-detail-back-row"
        main={<span />}
        drawer={<DetailDrawer title="Record" onClose={() => {}} closeTestId="memory-detail-back" autoFocus={false} />}
      />,
    );
    expect(view.getByTestId("memory-detail-back").getAttribute("aria-label")).toBe("Close details");
    expect(view.getByTestId("memory-detail-back-row").textContent).toBe("Back");
  });

  it("closes from its close button and resizes from the keyboard, persisting the width", () => {
    window.localStorage.clear();
    const view = render(<DrawerFixture />);
    fireEvent.click(view.getByTestId("node"));
    const handle = view.getByRole("separator", { name: "Resize details" });
    expect(handle.getAttribute("aria-valuenow")).toBe("340");
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(handle.getAttribute("aria-valuenow")).toBe("356");
    fireEvent.keyDown(handle, { key: "End" });
    expect(handle.getAttribute("aria-valuenow")).toBe("440");
    expect(window.localStorage.getItem("meta-console-drawer:topology:default")).toBe("440");
    fireEvent.click(view.getByRole("button", { name: "Close details" }));
    expect(view.queryByTestId("drawer") === null).toBe(true);
  });
});

describe("states and illustrations", () => {
  it("draws an illustration above the title, panel or inline size", () => {
    const view = render(
      <div>
        <EmptyState kind="empty" illustration="connections" title="No connections yet" body="These agents are not wired." testId="empty" />
        <EmptyState kind="error" illustration="error" inline title="Couldn't load approvals" details={[{ label: "Code", value: "-32602" }]} testId="error" />
      </div>,
    );
    const art = view.getByTestId("empty").querySelector("svg.cc-illus");
    expect(art?.getAttribute("data-illustration")).toBe("connections");
    expect(art?.getAttribute("aria-hidden")).toBe("true");
    expect(view.getByTestId("empty").querySelector(".cc-panel-empty__mark") === null).toBe(true);
    expect(view.getByTestId("error").querySelector("svg.cc-illus")?.getAttribute("data-size")).toBe("inline");
    expect(view.getByTestId("error").getAttribute("data-kind")).toBe("error");
    expect(view.getByTestId("error").querySelector("details.cc-tech") !== null).toBe(true);
  });

  it("knows every illustration name", () => {
    const names = ["agents", "connections", "work", "memory", "approvals-clear", "timeline", "routes", "search", "no-access", "not-configured", "error"] as const;
    const view = render(<div>{names.map((name) => <EmptyIllustration key={name} name={name} />)}</div>);
    expect(view.container.querySelectorAll("svg.cc-illus").length).toBe(names.length);
    expect(Array.from(view.container.querySelectorAll("svg.cc-illus")).every((svg) => svg.childElementCount > 0)).toBe(true);
  });

  it("gives a notice its tone's shape, and a live notice the live mark", () => {
    const view = render(
      <>
        <Notice tone="warn" testId="warn">2 records wait for approval.</Notice>
        <Notice tone="live" testId="live">Triage is replying now.</Notice>
      </>,
    );
    expect(view.getByTestId("warn").querySelector(".cc-glyph")?.getAttribute("data-glyph")).toBe("warning");
    const live = view.getByTestId("live");
    expect(live.querySelector(".cc-glyph") === null).toBe(true);
    expect(live.querySelector('.cc-mark[data-tone="live"]') !== null).toBe(true);
  });

  it("draws graph and timeline skeletons, announced politely", () => {
    const view = render(
      <div>
        <LoadingState variant="graph" label="Loading the team map" testId="graph" />
        <LoadingState variant="timeline" label="Loading today" testId="timeline" />
      </div>,
    );
    const graph = view.getByTestId("graph");
    expect(graph.getAttribute("role")).toBe("status");
    expect(graph.classList.contains("cc-skeleton--graph")).toBe(true);
    expect(graph.querySelectorAll(".cc-skeleton__node").length).toBe(9);
    expect(view.getByTestId("timeline").querySelectorAll(".cc-skeleton__bar").length).toBe(10);
    expect(graph.textContent).toBe("Loading the team map");
  });

  it("rings a live avatar and counts technical details with Copy all", () => {
    const view = render(
      <div>
        <AgentAvatar name="Billing" size={36} live />
        <TechnicalDetails testId="tech" items={[{ label: "Identity", value: "identity:billing" }, { label: "Session", value: "session-1" }]} />
        <TechnicalDetails testId="inline" inline items={[{ label: "Identity", value: "identity:billing" }]} />
      </div>,
    );
    const avatar = view.container.querySelector(".cc-agent__avatar");
    expect(avatar?.className).toBe("cc-agent__avatar is-live");
    expect(avatar?.getAttribute("data-size")).toBe("36");
    expect(view.getByTestId("tech").querySelector("summary")?.textContent).toBe("Technical details · 2");
    expect(view.getByRole("button", { name: "Copy all" }) !== null).toBe(true);
    expect(view.getByTestId("inline").tagName).toBe("DIV");
    expect(view.getByTestId("inline").querySelector(".cc-tech-id") !== null).toBe(true);
  });
});

describe("entities", () => {
  it("makes the title the entity's button and keeps actions beside it", () => {
    const onActivate = vi.fn();
    const onAction = vi.fn();
    const view = render(
      <EntityCard
        kind="agent"
        title="Billing"
        subtitle="Lead · Domains"
        mark={<AgentAvatar name="Billing" size={28} />}
        status={<StatusPill tone="neutral">Idle</StatusPill>}
        now={{ text: "Replying to Support Triage", live: true }}
        trend={{ values: [1, 2, 4], label: "12 turns today" }}
        figures={["12 turns today"]}
        actions={<Button size="sm" onClick={onAction}>Open chat</Button>}
        onActivate={onActivate}
        selected
        live
        testId="card"
      />,
    );
    const card = view.getByTestId("card");
    expect(card.className).toBe("cc-entity cc-entity--card cc-reveal-host is-interactive is-selected is-live");
    const title = view.getByRole("button", { name: "Billing" });
    expect(title.getAttribute("aria-current")).toBe("true");
    fireEvent.click(view.getByRole("button", { name: "Open chat" }));
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onActivate).toHaveBeenCalledTimes(0);
    fireEvent.click(card.querySelector(".cc-entity__facts") as Element);
    expect(onActivate).toHaveBeenCalledTimes(1);
    expect(card.querySelector('.cc-entity__now .cc-mark[data-tone="live"]') !== null).toBe(true);
  });

  it("lays a row out in one line and shows the trust ladder in words", () => {
    const view = render(
      <EntityRow kind="memory" title="Prefers short replies" subtitle="Billing's memory" figures={[<TrustLadder key="t" level={4} />]} testId="row" />,
    );
    expect(view.getByTestId("row").className).toBe("cc-entity cc-entity--row cc-reveal-host");
    expect(view.getByRole("img", { name: "Trust: Steward verified, 4 of 5" }) !== null).toBe(true);
    expect(view.getByTestId("row").querySelectorAll(".cc-trust__bar.is-on").length).toBe(4);
  });
});

describe("time", () => {
  it("words a countdown and tones it by distance", () => {
    const MIN = 60_000;
    expect(formatCountdown(45_000)).toBe("in 45s");
    expect(formatCountdown(90_000)).toBe("in 1m 30s");
    expect(formatCountdown(25 * MIN)).toBe("in 25m");
    expect(formatCountdown(125 * MIN)).toBe("in 2h 5m");
    expect(formatCountdown((2 * 24 + 4) * 60 * MIN)).toBe("in 2d 4h");
    expect(formatCountdown(-5 * MIN)).toBe("5m overdue");
    expect([countdownTone(2 * 60 * MIN), countdownTone(30 * MIN), countdownTone(5 * MIN), countdownTone(-1)]).toEqual(["neutral", "info", "warn", "crit"]);
    const now = Date.UTC(2026, 9, 30, 12, 0, 0);
    const view = render(<Countdown to={now + 5 * MIN} now={now} chip testId="countdown" />);
    expect(view.getByTestId("countdown").getAttribute("data-tone")).toBe("warn");
    expect(view.getByTestId("countdown").textContent).toBe("in 5m");
  });

  it("ticks every second under two minutes and stops when it unmounts", () => {
    vi.useFakeTimers();
    try {
      const start = Date.now();
      const view = render(<Countdown to={start + 65_000} testId="countdown" />);
      expect(view.getByTestId("countdown").textContent).toBe("in 1m 5s");
      act(() => {
        vi.advanceTimersByTime(3000);
      });
      expect(view.getByTestId("countdown").textContent).toBe("in 1m 2s");
      view.unmount();
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("puts a relative time in a chip with the clock glyph", () => {
    const now = Date.UTC(2026, 9, 30, 12, 0, 0);
    const view = render(<RelativeTimeChip value={now - 4 * 60_000} now={now} testId="chip" />);
    expect(view.getByTestId("chip").textContent).toBe("4m ago");
    expect(view.getByTestId("chip").querySelector('svg[data-glyph="clock"]') !== null).toBe(true);
  });
});

function FilterFixture() {
  const [query, setQuery] = useState("");
  const [blocked, setBlocked] = useState(false);
  return (
    <section className="cc-panel">
      <button type="button" data-testid="elsewhere">Elsewhere</button>
      <FilterBar
        search={{ label: "Search agents", value: query, onChange: setQuery, testId: "filter-search" }}
        chips={[{ label: "Blocked", count: 2, tone: "warn", pressed: blocked, onToggle: setBlocked, testId: "filter-blocked" }]}
        onClear={() => {
          setQuery("");
          setBlocked(false);
        }}
        testId="filters"
      />
    </section>
  );
}

describe("filter bar", () => {
  it("toggles chips, shows Clear only when something differs, and focuses search on /", () => {
    const view = render(<FilterFixture />);
    expect(view.queryByRole("button", { name: "Clear" }) === null).toBe(true);
    const chip = view.getByTestId("filter-blocked");
    fireEvent.click(chip);
    expect(chip.getAttribute("aria-pressed")).toBe("true");
    expect(chip.textContent).toBe("Blocked2");
    fireEvent.click(view.getByRole("button", { name: "Clear" }));
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    view.getByTestId("elsewhere").focus();
    fireEvent.keyDown(view.getByTestId("elsewhere"), { key: "/" });
    expect(document.activeElement === view.getByTestId("filter-search")).toBe(true);
  });

  it("takes each chip's key as its React key, never spread into the chip", () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const view = render(
        <FilterBar
          ariaLabel="Memory filters"
          chips={[
            { key: "facts", label: "Facts", count: 12, pressed: false, onToggle: () => {} },
            { key: "gotchas", label: "Gotchas", count: 3, pressed: true, onToggle: () => {} },
          ]}
        />,
      );
      expect(view.getAllByRole("button", { pressed: true }).map((chip) => chip.textContent)).toEqual(["Gotchas3"]);
      const spread = error.mock.calls.filter((call) => String(call[0]).includes('"key" prop is being spread'));
      expect(spread.length).toBe(0);
    } finally {
      error.mockRestore();
    }
  });
});

describe("navigation", () => {
  it("links names only when asked and when the host can open them", () => {
    const openAgent = vi.fn();
    const openWorkItem = vi.fn();
    const plain = render(<AgentName id="identity:billing" link />);
    expect(plain.container.querySelector("button") === null).toBe(true);
    const view = render(
      <ConsoleNavigationProvider value={{ openAgent, openWorkItem }}>
        <AgentName id="identity:billing" link testId="name" />
        <AgentName id="identity:support" testId="unlinked" />
        <EntityLink kind="work-item" id="item-1">Publish to all regions</EntityLink>
        <EntityLink kind="approval" id="ap-1">Deploy</EntityLink>
      </ConsoleNavigationProvider>,
    );
    const name = view.getByTestId("name");
    expect(name.tagName).toBe("BUTTON");
    fireEvent.click(name);
    expect(openAgent).toHaveBeenLastCalledWith("identity:billing", "details");
    fireEvent.click(name, { ctrlKey: true });
    expect(openAgent).toHaveBeenLastCalledWith("identity:billing", "chat");
    expect(view.getByTestId("unlinked").tagName).toBe("SPAN");
    fireEvent.click(view.getByRole("button", { name: "Publish to all regions" }));
    expect(openWorkItem).toHaveBeenCalledWith("item-1");
    expect(view.queryByRole("button", { name: "Deploy" }) === null).toBe(true);
  });

  it("offers the new-items pill only when there is something new", () => {
    const onClick = vi.fn();
    const view = render(<NewItemsPill count={0} onClick={onClick} />);
    expect(view.container.innerHTML).toBe("");
    view.rerender(<NewItemsPill count={3} onClick={onClick} />);
    fireEvent.click(view.getByRole("button", { name: "3 new events" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});

/** Selector specificity as [ids, classes and pseudo-classes, types]. */
function specificity(selector: string): [number, number, number] {
  const body = selector.replace(/::[\w-]+/g, "");
  const ids = (body.match(/#[\w-]+/g) ?? []).length;
  const classes = (body.match(/\.[\w-]+|\[[^\]]*\]|:(?!:)[\w-]+(\([^)]*\))?/g) ?? []).length;
  const types = (body.replace(/\[[^\]]*\]|\([^)]*\)/g, "").match(/(^|[\s>+~])[a-z][\w-]*/gi) ?? []).length;
  return [ids, classes, types];
}

function outranks(a: [number, number, number], b: [number, number, number]): boolean {
  for (let index = 0; index < 3; index += 1) if (a[index] !== b[index]) return a[index] > b[index];
  return false;
}

describe("kit stylesheet", () => {
  it("draws the glance strip's rules over the figures' border reset", async () => {
    const fs = await import("node:fs");
    const path = await import("node:path");
    const css = fs.readFileSync(path.join(__dirname, "../styles/panel-kit.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({ selector: match[1].trim(), body: match[2] }));
    const divider = rules.find((rule) => rule.selector.startsWith(".cc-glance >") && /border-left:\s*1px solid/.test(rule.body));
    const reset = rules.find((rule) => rule.selector === ".cc-figure" && /border:\s*0/.test(rule.body));
    expect(Boolean(divider && reset)).toBe(true);
    // Equal specificity would leave the rule to source order, and the reset
    // comes after it: the divider must outrank the reset outright.
    expect(outranks(specificity(divider!.selector), specificity(reset!.selector))).toBe(true);
  });

  it("keeps a loading figure's boxes: hidden, never removed, and the foot line always there", async () => {
    const fs = await import("node:fs");
    const path = await import("node:path");
    const css = fs.readFileSync(path.join(__dirname, "../styles/panel-kit.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({ selector: match[1].trim().replace(/\s+/g, " "), body: match[2] }));
    const body = (selector: string) => rules.filter((rule) => rule.selector === selector).map((rule) => rule.body).join(" ");
    expect(body(".cc-figure__foot")).toMatch(/min-height:\s*1\.4em/);
    expect(body(".cc-figure__foot")).toMatch(/line-height:\s*1\.4/);
    expect(body(".cc-figure__skeleton")).toMatch(/position:\s*absolute/);
    expect(body(".cc-figure.is-loading .cc-figure__value")).toMatch(/position:\s*relative/);
    expect(body(".cc-figure.is-loading .cc-figure__meter, .cc-figure.is-loading .cc-figure__foot > *")).toMatch(/visibility:\s*hidden/);
    // Nothing a loading tile holds is taken out of the layout.
    const loadingRules = rules.filter((rule) => rule.selector.includes(".is-loading"));
    expect(loadingRules.length > 0).toBe(true);
    expect(loadingRules.filter((rule) => /display:\s*none|height:/.test(rule.body)).map((rule) => rule.selector)).toEqual([]);
  });

  it("wraps four or more figures to two rows between 480 and 719 px, rules kept", async () => {
    const fs = await import("node:fs");
    const path = await import("node:path");
    const css = fs.readFileSync(path.join(__dirname, "../styles/panel-kit.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
    const start = css.indexOf("@container cc-panel (min-width: 480px) and (max-width: 719px)");
    expect(start >= 0).toBe(true);
    let depth = 0;
    let end = start;
    for (let index = css.indexOf("{", start); index < css.length; index += 1) {
      if (css[index] === "{") depth += 1;
      else if (css[index] === "}" && --depth === 0) {
        end = index;
        break;
      }
    }
    const rules = [...css.slice(css.indexOf("{", start) + 1, end).matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({ selector: match[1].trim(), body: match[2] }));
    const body = (selector: string) => rules.find((rule) => rule.selector === selector)?.body ?? "";
    const four = ".cc-glance:has(> :nth-child(4))";
    const five = ".cc-glance:has(> :nth-child(5))";
    const onlyFour = `${four}:not(:has(> :nth-child(5)))`;
    expect(body(four)).toMatch(/grid-template-columns:\s*repeat\(2,/);
    expect(body(five)).toMatch(/grid-template-columns:\s*repeat\(3,/);
    expect(body(`${onlyFour} > :nth-child(2n + 1)`)).toMatch(/border-left:\s*0/);
    expect(body(`${onlyFour} > :nth-child(n + 3)`)).toMatch(/border-top:\s*1px solid/);
    expect(body(`${five} > :nth-child(3n + 1)`)).toMatch(/border-left:\s*0/);
    expect(body(`${five} > :nth-child(n + 4)`)).toMatch(/border-top:\s*1px solid/);
    // A row's first figure drops the divider: the reset outranks it.
    expect(outranks(specificity(`${five} > :nth-child(3n + 1)`), specificity(".cc-glance > :nth-child(n + 2)"))).toBe(true);
  });
});
