import React from "react";
import { consoleCustomPanelTarget, type ConsolePanelDefinition } from "@console-core";
import { IconButton, Popover, SearchField, StatusMark, splitTechnicalId } from "@console-components";
import type {
  ConsoleDockNode,
  ConsoleDockPanelSplitDirection,
  ConsoleDockSplitDirection,
  ConsoleDockTabView,
  ConsoleDockViewState,
} from "@console-core";
import type { ConsoleAgent } from "../types";
import type { MetaDockTarget } from "../lib/adapters";
import { buildControlTarget, buildDockTarget } from "../lib/adapters";
import { agentDisplayTitle } from "../../../packages/console-core/src/agent-title";
import { agentHealth, isAgentWorking } from "../lib/agent-status";
import { Icon } from "../icon";
import { NAV_ICON, NAV_LABEL, normalizeNavKind, workbenchNavKinds, type NavKind } from "./Sidebar";

interface MetaDockProps {
  viewState: ConsoleDockViewState<MetaDockTarget>;
  agents: ConsoleAgent[];
  renderPanelBody: (panel: { id: string; target?: MetaDockTarget | null }) => React.ReactNode;
  visibleControls?: NavKind[];
  customPanels?: readonly ConsolePanelDefinition[];
  onSelectTab: (tabId: string) => void;
  onCloseTab: (tabId: string) => void;
  onCreateTab: () => void;
  onFocusPanel: (panelId: string) => void;
  onSplitPanel: (panelId: string, direction: ConsoleDockPanelSplitDirection) => void;
  onClosePanel: (panelId: string) => void;
  onResizeSplit: (splitId: string, ratio: number) => void;
  onOpenTargetInPanel: (panelId: string, target: MetaDockTarget) => void;
}

function tabPanelCount(node: ConsoleDockNode | null | undefined): number {
  if (!node) return 0;
  if (node.kind === "panel") return 1;
  return tabPanelCount(node.first) + tabPanelCount(node.second);
}

function findFirstPanelId(node: ConsoleDockNode | null | undefined): string | null {
  if (!node) return null;
  if (node.kind === "panel") return node.panelId;
  return findFirstPanelId(node.first) || findFirstPanelId(node.second);
}

export function MetaDock({
  viewState,
  agents,
  renderPanelBody,
  visibleControls,
  customPanels,
  onSelectTab,
  onCloseTab,
  onCreateTab,
  onFocusPanel,
  onSplitPanel,
  onClosePanel,
  onResizeSplit,
  onOpenTargetInPanel,
}: MetaDockProps): React.JSX.Element {
  const activeTab = viewState.tabs.find((t) => t.id === viewState.activeTabId) || viewState.tabs[0];

  // Keyboard shortcuts
  React.useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod) return;
      const focusedId = viewState.focusedPanelId;
      if (e.key === "d" && !e.shiftKey) {
        if (!focusedId) return;
        e.preventDefault();
        onSplitPanel(focusedId, "right");
      } else if (e.key === "D" && e.shiftKey) {
        if (!focusedId) return;
        e.preventDefault();
        onSplitPanel(focusedId, "down");
      } else if (e.key === "w" && !e.shiftKey) {
        if (!focusedId) return;
        e.preventDefault();
        onClosePanel(focusedId);
      } else if (e.key === "t" && !e.shiftKey) {
        e.preventDefault();
        onCreateTab();
      } else if (e.key === "]" && e.shiftKey) {
        e.preventDefault();
        const idx = viewState.tabs.findIndex((t) => t.id === viewState.activeTabId);
        const next = viewState.tabs[(idx + 1) % viewState.tabs.length];
        if (next) onSelectTab(next.id);
      } else if (e.key === "[" && e.shiftKey) {
        e.preventDefault();
        const idx = viewState.tabs.findIndex((t) => t.id === viewState.activeTabId);
        const prev = viewState.tabs[(idx - 1 + viewState.tabs.length) % viewState.tabs.length];
        if (prev) onSelectTab(prev.id);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewState, onSplitPanel, onClosePanel, onCreateTab, onSelectTab]);

  return (
    <div className="mkdock" data-testid="mkdock">
      {/* Workspace tabs (B16): the panel-tab underline, a pane count, close on
          hover or focus (always on the active tab), and a "+" button. */}
      <div className="wstabs" role="tablist" aria-label="Workspaces">
        {viewState.tabs.map((t) => {
          const isActive = t.id === activeTab?.id;
          const count = tabPanelCount(t.layout);
          const title = t.title || "untitled";
          return (
            <div
              key={t.id}
              className={`wstab cc-reveal-host ${isActive ? "is-active is-selected" : ""}`}
              data-testid={`wstab:${t.id}`}
            >
              <button
                type="button"
                role="tab"
                aria-selected={isActive}
                className="wstab__select"
                onClick={() => onSelectTab(t.id)}
                title={title}
              >
                <span className="wstab__mark" aria-hidden="true" />
                <span className="wstab__name">{title}</span>
                {count > 1 ? (
                  <span className="wstab__count" title={`${count} panes`}>
                    {count}
                    <span className="cc-visually-hidden"> panes</span>
                  </span>
                ) : null}
              </button>
              {viewState.tabs.length > 1 ? (
                <IconButton
                  label={`Close workspace ${title}`}
                  title="Close workspace"
                  glyph="close"
                  size="sm"
                  reveal
                  className="wstab__close"
                  data-testid={`wstab-close:${t.id}`}
                  onClick={(e) => { e.stopPropagation(); onCloseTab(t.id); }}
                />
              ) : null}
            </div>
          );
        })}
        <IconButton
          label="New workspace"
          title="New workspace (⌘T)"
          glyph="plus"
          size="sm"
          className="wstab__add"
          data-testid="wstab-add"
          onClick={onCreateTab}
        />
      </div>

      <div className="dock">
        {activeTab && (
          <DockLayout
            node={activeTab.layout}
            viewState={viewState}
            agents={agents}
            visibleControls={visibleControls}
            customPanels={customPanels}
            renderPanelBody={renderPanelBody}
            onFocusPanel={onFocusPanel}
            onSplitPanel={onSplitPanel}
            onClosePanel={onClosePanel}
            onResizeSplit={onResizeSplit}
            onOpenTargetInPanel={onOpenTargetInPanel}
          />
        )}
      </div>
    </div>
  );
}

interface DockLayoutProps extends Pick<MetaDockProps,
  "viewState" | "agents" | "renderPanelBody" |
  "visibleControls" | "customPanels" | "onFocusPanel" | "onSplitPanel" | "onClosePanel" | "onResizeSplit" | "onOpenTargetInPanel"
> {
  node: ConsoleDockNode;
}

function DockLayout(props: DockLayoutProps): React.JSX.Element | null {
  const { node } = props;
  if (node.kind === "panel") {
    return <PaneView panelId={node.panelId} {...props} />;
  }
  return <SplitView {...props} />;
}

function SplitView(props: DockLayoutProps): React.JSX.Element | null {
  const { node } = props;
  // Every hook before the first return.
  const hostRef = React.useRef<HTMLDivElement>(null);
  if (node.kind !== "split") return null;
  const ratio = typeof node.ratio === "number" ? Math.max(0.1, Math.min(0.9, node.ratio)) : 0.5;
  const direction: ConsoleDockSplitDirection = node.direction;
  const style: React.CSSProperties =
    direction === "horizontal"
      ? { gridTemplateColumns: `${ratio * 100}% 6px ${(1 - ratio) * 100}%` }
      : { gridTemplateRows: `${ratio * 100}% 6px ${(1 - ratio) * 100}%` };

  function startDrag(e: React.PointerEvent<HTMLDivElement>) {
    e.preventDefault();
    const host = hostRef.current;
    if (!host || node.kind !== "split") return;
    const splitId = node.id;
    const rect = host.getBoundingClientRect();
    (e.currentTarget as HTMLDivElement).setPointerCapture(e.pointerId);
    function move(ev: PointerEvent) {
      const r = direction === "horizontal"
        ? (ev.clientX - rect.left) / rect.width
        : (ev.clientY - rect.top) / rect.height;
      props.onResizeSplit(splitId, Math.max(0.1, Math.min(0.9, r)));
    }
    function end() {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
    }
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
  }

  return (
    <div
      ref={hostRef}
      className={`split split--${direction === "horizontal" ? "h" : "v"}`}
      style={style}
    >
      <DockLayout {...props} node={node.first} />
      <div
        className={`split__handle split__handle--${direction === "horizontal" ? "h" : "v"}`}
        onPointerDown={startDrag}
        data-testid={`split-handle:${node.id}`}
      />
      <DockLayout {...props} node={node.second} />
    </div>
  );
}

interface PaneViewProps extends Pick<MetaDockProps,
  "viewState" | "agents" | "renderPanelBody" |
  "visibleControls" | "customPanels" | "onFocusPanel" | "onSplitPanel" | "onClosePanel" | "onOpenTargetInPanel"
> {
  panelId: string;
}

/** The pane title's icon: the view's own, a chat for an agent, a cube for a custom panel. */
function paneTargetIcon(target: MetaDockTarget | null | undefined): string | null {
  if (!target) return null;
  const nav = normalizeNavKind(target.kind);
  if (nav) return NAV_ICON[nav];
  if (target.kind === "agent-chat") return "i-chat";
  if (target.kind === "identity-inspect") return "i-person";
  if (target.kind === "custom/panel") return "i-cube";
  if (target.kind === "operator-panel") return "i-open";
  return null;
}

function PaneView({
  panelId, viewState, agents, renderPanelBody, visibleControls, customPanels,
  onFocusPanel, onSplitPanel, onClosePanel, onOpenTargetInPanel,
}: PaneViewProps): React.JSX.Element | null {
  // Every hook before the first return.
  const [menuOpen, setMenuOpen] = React.useState(false);
  const titleRef = React.useRef<HTMLButtonElement | null>(null);
  const menuId = `${React.useId()}-pane-menu`;
  const panel = viewState.panels.find((p) => p.id === panelId);
  if (!panel) return null;

  const isFocused = viewState.focusedPanelId === panelId;
  const title = panel.title || panel.target?.title || "untitled";
  const target = panel.target;
  const subId = target?.kind === "agent-chat"
    ? (target.identity || target.memberId)
    : target?.kind === "identity-inspect"
      ? target.identity
      : undefined;
  const icon = paneTargetIcon(target);

  return (
    <div
      className={`pane ${isFocused ? "is-focused" : ""}`}
      onMouseDown={() => onFocusPanel(panelId)}
      data-testid={`pane:${panelId}`}
      data-focused={isFocused ? "true" : undefined}
    >
      {/* The pane bar (B16): the title opens the pane menu; split and close
          show on hover or focus of the bar. */}
      <div className="pane__bar cc-reveal-host">
        <button
          ref={titleRef}
          type="button"
          className="pane__title"
          aria-haspopup="dialog"
          aria-expanded={menuOpen}
          aria-controls={menuOpen ? menuId : undefined}
          onClick={(e) => { e.stopPropagation(); onFocusPanel(panelId); setMenuOpen((v) => !v); }}
          data-testid={`pane-title:${panelId}`}
          title="Choose what this pane shows"
        >
          {icon ? <Icon name={icon} className="pane__title-icon" /> : null}
          <span className="pane__title-text" title={subId}>{title}</span>
          <Icon name="i-chevron" className="pane__caret" />
        </button>
        <PaneMenu
          open={menuOpen}
          id={menuId}
          testId={`pane-menu:${panelId}`}
          anchorRef={titleRef}
          agents={agents}
          visibleControls={visibleControls}
          customPanels={customPanels}
          onClose={() => setMenuOpen(false)}
          onPick={(picked) => {
            setMenuOpen(false);
            onOpenTargetInPanel(panelId, picked);
          }}
        />
        <span className="pane__spacer" />
        <IconButton
          label="Split right"
          title="Split right (⌘D)"
          icon={<Icon name="i-split-right" />}
          size="sm"
          reveal
          className="pane__btn"
          data-testid={`pane-split-right:${panelId}`}
          onClick={(e) => { e.stopPropagation(); onSplitPanel(panelId, "right"); }}
        />
        <IconButton
          label="Split down"
          title="Split down (⌘⇧D)"
          icon={<Icon name="i-split-down" />}
          size="sm"
          reveal
          className="pane__btn"
          data-testid={`pane-split-down:${panelId}`}
          onClick={(e) => { e.stopPropagation(); onSplitPanel(panelId, "down"); }}
        />
        <IconButton
          label="Close pane"
          title="Close pane (⌘W)"
          glyph="close"
          size="sm"
          reveal
          className="pane__btn pane__close"
          data-testid={`pane-close:${panelId}`}
          onClick={(e) => { e.stopPropagation(); onClosePanel(panelId); }}
        />
      </div>
      <div className="pane__body">
        {renderPanelBody({ id: panelId, target })}
      </div>
    </div>
  );
}

/**
 * An agent's identity in the pane menu, as demoted detail: it tells apart
 * agents that share a title. Muted small mono, middle-truncated so the
 * distinguishing tail stays visible, the full value in the tooltip. The
 * text content stays exactly the identity.
 */
function PaneMenuIdentity({ value }: { value: string }): React.JSX.Element {
  const parts = splitTechnicalId(value);
  return (
    <span className="pane-menu__id" title={value}>
      <span className="pane-menu__id-head">{parts.head}</span>
      {parts.tail ? <span className="pane-menu__id-tail">{parts.tail}</span> : null}
    </span>
  );
}

/** One line of the pane menu: a group's label, or something it can show. */
type PaneMenuRow =
  | { kind: "label"; key: string; text: string; count?: number }
  | { kind: "view"; key: string; nav: NavKind }
  | { kind: "custom"; key: string; panel: ConsolePanelDefinition }
  | { kind: "agent"; key: string; agent: ConsoleAgent; title: string };

type PaneMenuOption = Exclude<PaneMenuRow, { kind: "label" }>;

/** Row heights the menu's list lays out with; dock.css matches them. */
const PANE_MENU_ROW_HEIGHT = 30;
const PANE_MENU_LABEL_HEIGHT = 28;
/** The list's height when it cannot be measured (no layout). */
const PANE_MENU_FALLBACK_HEIGHT = 440;
const PANE_MENU_OVERSCAN = 6;

function paneMenuRowHeight(row: PaneMenuRow): number {
  return row.kind === "label" ? PANE_MENU_LABEL_HEIGHT : PANE_MENU_ROW_HEIGHT;
}

function agentMatches(agent: ConsoleAgent, title: string, needle: string): boolean {
  return [title, agent.label, agent.identity, agent.member_id, agent.role]
    .some((value) => value?.toLowerCase().includes(needle));
}

interface PaneMenuProps {
  open: boolean;
  id: string;
  testId: string;
  anchorRef: React.RefObject<HTMLElement | null>;
  agents: ConsoleAgent[];
  visibleControls?: NavKind[];
  customPanels?: readonly ConsolePanelDefinition[];
  onClose: () => void;
  onPick: (target: MetaDockTarget) => void;
}

/**
 * The pane menu (B16): every view the sidebar lists, in its order and with
 * its icons, the custom panels, then every agent, under a search field. The
 * list is a listbox the search field drives (arrows move, Enter shows the
 * choice in the pane, Escape closes); a long roster renders only the rows in
 * sight.
 */
function PaneMenu({ open, ...rest }: PaneMenuProps): React.JSX.Element | null {
  if (!open) return null;
  return <PaneMenuBody {...rest} />;
}

function PaneMenuBody({
  id,
  testId,
  anchorRef,
  agents,
  visibleControls,
  customPanels = [],
  onClose,
  onPick,
}: Omit<PaneMenuProps, "open">): React.JSX.Element {
  const [query, setQuery] = React.useState("");
  const [active, setActive] = React.useState(0);
  const [scrollTop, setScrollTop] = React.useState(0);
  const [listHeight, setListHeight] = React.useState(0);
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const needle = query.trim().toLowerCase();
  const navKinds = React.useMemo(() => workbenchNavKinds(visibleControls), [visibleControls]);
  const titledAgents = React.useMemo(
    () => agents.map((agent) => ({ agent, title: agentDisplayTitle(agent) })),
    [agents],
  );
  const rows = React.useMemo<PaneMenuRow[]>(() => {
    const views = navKinds.filter((kind) => !needle || NAV_LABEL[kind].toLowerCase().includes(needle));
    const panels = customPanels.filter((panel) => !needle || panel.title.toLowerCase().includes(needle));
    const matching = titledAgents.filter(({ agent, title }) => !needle || agentMatches(agent, title, needle));
    const out: PaneMenuRow[] = [];
    if (views.length > 0 || panels.length > 0) {
      out.push({ kind: "label", key: "label:views", text: "Views" });
      for (const nav of views) out.push({ kind: "view", key: `view:${nav}`, nav });
      for (const panel of panels) out.push({ kind: "custom", key: `custom:${panel.id}`, panel });
    }
    if (matching.length > 0) {
      out.push({ kind: "label", key: "label:agents", text: "Agents", count: matching.length });
      for (const { agent, title } of matching) out.push({ kind: "agent", key: `agent:${agent.member_id}`, agent, title });
    }
    return out;
  }, [customPanels, navKinds, needle, titledAgents]);
  const options = React.useMemo(
    () => rows.flatMap((row, index) => (row.kind === "label" ? [] : [{ row, index }])),
    [rows],
  );
  const offsets = React.useMemo(() => {
    const tops: number[] = [];
    let total = 0;
    for (const row of rows) {
      tops.push(total);
      total += paneMenuRowHeight(row);
    }
    return { tops, total };
  }, [rows]);
  const activeIndex = Math.min(active, Math.max(0, options.length - 1));
  const activeOption = options[activeIndex];

  React.useLayoutEffect(() => {
    const list = listRef.current;
    if (!list) return;
    setListHeight(list.clientHeight);
  }, [rows]);

  // Keep the active option in sight as the arrows move it.
  React.useLayoutEffect(() => {
    const list = listRef.current;
    if (!list || !activeOption) return;
    const top = offsets.tops[activeOption.index] ?? 0;
    const bottom = top + paneMenuRowHeight(activeOption.row);
    const height = list.clientHeight || PANE_MENU_FALLBACK_HEIGHT;
    if (top < list.scrollTop) list.scrollTop = Math.max(0, top - (activeIndex === 0 ? PANE_MENU_LABEL_HEIGHT : 0));
    else if (bottom > list.scrollTop + height) list.scrollTop = bottom - height;
    setScrollTop(list.scrollTop);
  }, [activeIndex, activeOption, offsets]);

  const viewport = listHeight > 0 ? listHeight : PANE_MENU_FALLBACK_HEIGHT;
  let start = 0;
  while (start < rows.length && (offsets.tops[start] ?? 0) + paneMenuRowHeight(rows[start]!) < scrollTop) start += 1;
  let end = start;
  while (end < rows.length && (offsets.tops[end] ?? 0) < scrollTop + viewport) end += 1;
  start = Math.max(0, start - PANE_MENU_OVERSCAN);
  end = Math.min(rows.length, end + PANE_MENU_OVERSCAN);

  const pick = (row: PaneMenuOption) => {
    if (row.kind === "view") onPick(buildControlTarget(row.nav));
    else if (row.kind === "custom") onPick(consoleCustomPanelTarget(row.panel));
    else onPick(buildDockTarget(row.agent));
  };
  const optionId = (index: number) => `${id}-option-${index}`;

  const onSearchKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (options.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((activeIndex + 1) % options.length);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((activeIndex - 1 + options.length) % options.length);
    } else if (event.key === "PageDown") {
      event.preventDefault();
      setActive(Math.min(options.length - 1, activeIndex + 8));
    } else if (event.key === "PageUp") {
      event.preventDefault();
      setActive(Math.max(0, activeIndex - 8));
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (activeOption) pick(activeOption.row as PaneMenuOption);
    }
  };

  return (
    <Popover
      open
      anchorRef={anchorRef}
      onClose={onClose}
      role="dialog"
      ariaLabel="Show in this pane"
      placement="bottom-start"
      width={320}
      id={id}
      testId={testId}
      className="pane-menu"
      initialFocus={(popover) => popover.querySelector<HTMLElement>("input")}
    >
      <div className="pane-menu__search">
        <SearchField
          label="Find a view or an agent"
          placeholder="Find a view or an agent"
          value={query}
          onChange={(value) => {
            setQuery(value);
            setActive(0);
            if (listRef.current) listRef.current.scrollTop = 0;
            setScrollTop(0);
          }}
          role="combobox"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={activeOption ? optionId(activeOption.index) : undefined}
          onKeyDown={onSearchKeyDown}
          testId="pane-menu-search"
        />
      </div>
      <div
        ref={listRef}
        id={`${id}-list`}
        className="pane-menu__list"
        role="listbox"
        aria-label="Views and agents"
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        {rows.length === 0 ? (
          <div className="pane-menu__empty" role="status">Nothing matches "{query.trim()}".</div>
        ) : (
          <div className="pane-menu__space" style={{ height: `${offsets.total}px` }}>
            {rows.slice(start, end).map((row, offset) => {
              const index = start + offset;
              const top = offsets.tops[index] ?? 0;
              const style: React.CSSProperties = { transform: `translateY(${top}px)` };
              if (row.kind === "label") {
                return (
                  <div key={row.key} className="pane-menu__label" style={style} role="presentation">
                    <span>{row.text}</span>
                    {row.count !== undefined ? <span className="pane-menu__count">{row.count}</span> : null}
                  </div>
                );
              }
              const isActive = activeOption?.index === index;
              const common = {
                id: optionId(index),
                role: "option" as const,
                "aria-selected": isActive,
                className: `pane-menu__item${isActive ? " is-active" : ""}`,
                style,
                onMouseDown: (event: React.MouseEvent) => event.preventDefault(),
                onMouseMove: () => {
                  const at = options.findIndex((option) => option.index === index);
                  if (at >= 0 && at !== activeIndex) setActive(at);
                },
                onClick: () => pick(row),
              };
              if (row.kind === "view") {
                return (
                  <div key={row.key} {...common} data-testid={`pane-menu-view:${row.nav}`}>
                    <Icon name={NAV_ICON[row.nav]} className="pane-menu__icon" />
                    <span className="pane-menu__name">{NAV_LABEL[row.nav]}</span>
                  </div>
                );
              }
              if (row.kind === "custom") {
                return (
                  <div key={row.key} {...common} data-testid={`pane-menu-custom-panel:${row.panel.id}`}>
                    <Icon name="i-cube" className="pane-menu__icon" />
                    <span className="pane-menu__name">{row.panel.title}</span>
                  </div>
                );
              }
              const health = agentHealth(row.agent);
              const working = isAgentWorking(row.agent);
              return (
                <div
                  key={row.key}
                  {...common}
                  data-state={(row.agent.state || "").toLowerCase()}
                  data-testid={`pane-menu-agent:${row.agent.member_id}`}
                >
                  <StatusMark
                    tone={health.problem ? health.tone : working ? "live" : health.tone}
                    className="pane-menu__mark"
                  />
                  <span className="pane-menu__name">{row.title}</span>
                  <PaneMenuIdentity value={row.agent.identity || row.agent.member_id} />
                </div>
              );
            })}
          </div>
        )}
      </div>
    </Popover>
  );
}
