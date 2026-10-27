import React from "react";
import { consoleCustomPanelTarget, type ConsolePanelDefinition } from "@console-core";
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
import type { NavKind } from "./Sidebar";

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
      <div className="wstabs">
        {viewState.tabs.map((t) => {
          const isActive = t.id === activeTab?.id;
          const count = tabPanelCount(t.layout);
          return (
            <div
              key={t.id}
              className={`wstab ${isActive ? "is-active" : ""}`}
              onClick={() => onSelectTab(t.id)}
              data-testid={`wstab:${t.id}`}
            >
              <span className="wstab__mark" />
              <span className="wstab__name">{t.title || "untitled"}</span>
              {count > 1 && <span className="wstab__count">{count}</span>}
              {viewState.tabs.length > 1 && (
                <button
                  className="wstab__close"
                  onClick={(e) => { e.stopPropagation(); onCloseTab(t.id); }}
                  data-testid={`wstab-close:${t.id}`}
                  aria-label="Close workspace"
                >
                  ×
                </button>
              )}
            </div>
          );
        })}
        <button
          className="wstab__add"
          onClick={onCreateTab}
          data-testid="wstab-add"
          title="New workspace (⌘T)"
          aria-label="New workspace"
        >
          +
        </button>
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
  return <SplitView node={node} {...props} />;
}

function SplitView(props: DockLayoutProps): React.JSX.Element | null {
  const { node } = props;
  if (node.kind !== "split") return null;
  const ratio = typeof node.ratio === "number" ? Math.max(0.1, Math.min(0.9, node.ratio)) : 0.5;
  const direction: ConsoleDockSplitDirection = node.direction;
  const style: React.CSSProperties =
    direction === "horizontal"
      ? { gridTemplateColumns: `${ratio * 100}% 6px ${(1 - ratio) * 100}%` }
      : { gridTemplateRows: `${ratio * 100}% 6px ${(1 - ratio) * 100}%` };

  const hostRef = React.useRef<HTMLDivElement>(null);

  function startDrag(e: React.PointerEvent<HTMLDivElement>) {
    e.preventDefault();
    const host = hostRef.current;
    if (!host) return;
    const rect = host.getBoundingClientRect();
    (e.currentTarget as HTMLDivElement).setPointerCapture(e.pointerId);
    function move(ev: PointerEvent) {
      const r = direction === "horizontal"
        ? (ev.clientX - rect.left) / rect.width
        : (ev.clientY - rect.top) / rect.height;
      props.onResizeSplit(node.id, Math.max(0.1, Math.min(0.9, r)));
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

function PaneView({
  panelId, viewState, agents, renderPanelBody, visibleControls, customPanels,
  onFocusPanel, onSplitPanel, onClosePanel, onOpenTargetInPanel,
}: PaneViewProps): React.JSX.Element | null {
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

  const [menuOpen, setMenuOpen] = React.useState(false);

  return (
    <div
      className={`pane ${isFocused ? "is-focused" : ""}`}
      onMouseDown={() => onFocusPanel(panelId)}
      data-testid={`pane:${panelId}`}
    >
      <div className="pane__bar">
        <button
          className="pane__title"
          onClick={(e) => { e.stopPropagation(); onFocusPanel(panelId); setMenuOpen((v) => !v); }}
          data-testid={`pane-title:${panelId}`}
          title="Retarget pane"
        >
          <span className="pane__title-text">{title}</span>
          <span className="pane__caret">▾</span>
        </button>
        {subId && <span className="pane__id">{subId}</span>}
        <span className="pane__spacer" />
        <button
          className="pane__btn"
          onClick={(e) => { e.stopPropagation(); onSplitPanel(panelId, "right"); }}
          title="Split right (⌘D)"
          data-testid={`pane-split-right:${panelId}`}
        >
          ◨
        </button>
        <button
          className="pane__btn"
          onClick={(e) => { e.stopPropagation(); onSplitPanel(panelId, "down"); }}
          title="Split down (⌘⇧D)"
          data-testid={`pane-split-down:${panelId}`}
        >
          ⬓
        </button>
        <button
          className="pane__btn pane__close"
          onClick={(e) => { e.stopPropagation(); onClosePanel(panelId); }}
          title="Close pane (⌘W)"
          data-testid={`pane-close:${panelId}`}
        >
          ×
        </button>

        {menuOpen && (
          <PaneMenu
            agents={agents}
            visibleControls={visibleControls}
            customPanels={customPanels}
            onClose={() => setMenuOpen(false)}
            onPick={(target) => {
              setMenuOpen(false);
              onOpenTargetInPanel(panelId, target);
            }}
          />
        )}
      </div>
      <div className="pane__body">
        {renderPanelBody({ id: panelId, target })}
      </div>
    </div>
  );
}

interface PaneMenuProps {
  agents: ConsoleAgent[];
  visibleControls?: NavKind[];
  customPanels?: readonly ConsolePanelDefinition[];
  onClose: () => void;
  onPick: (target: MetaDockTarget) => void;
}

function PaneMenu({ agents, visibleControls, customPanels = [], onClose, onPick }: PaneMenuProps): React.JSX.Element {
  const controls = ([
    ["topology", "Topology"],
    ["timeline", "Today"],
    ["gating", "Approvals"],
    ["roster", "Roster"],
    ["routing", "Routing"],
    ["logs", "Logs"],
    ["health", "Health"],
  ] as const).filter(([kind]) => !visibleControls || visibleControls.includes(kind));
  return (
    <>
      <div className="pane-menu__scrim" onMouseDown={onClose} />
      <div className="pane-menu" onMouseDown={(e) => e.stopPropagation()}>
        <div className="pane-menu__label">Views</div>
        {controls.map(([kind, label]) => (
          <button
            key={kind}
            className="pane-menu__item"
            onClick={() => onPick(buildControlTarget(kind))}
            data-testid={`pane-menu-view:${kind}`}
          >
            <span />
            <span>{label}</span>
            <span className="pane-menu__id">view</span>
          </button>
        ))}
        {customPanels.map(panel => <button key={panel.id} className="pane-menu__item"
          onClick={() => onPick(consoleCustomPanelTarget(panel))} data-testid={`pane-menu-custom-panel:${panel.id}`}>
          <span /><span>{panel.title}</span><span className="pane-menu__id">view</span>
        </button>)}
        <div className="pane-menu__sep" />
        <div className="pane-menu__label">Agents</div>
        {agents.slice(0, 14).map((a) => (
          <button
            key={a.member_id}
            className="pane-menu__item"
            data-state={(a.state || "").toLowerCase()}
            onClick={() => onPick(buildDockTarget(a))}
            data-testid={`pane-menu-agent:${a.member_id}`}
          >
            <span className="agent__dot" />
            <span>{a.label}</span>
            <span className="pane-menu__id">{a.identity || a.member_id}</span>
          </button>
        ))}
      </div>
    </>
  );
}
