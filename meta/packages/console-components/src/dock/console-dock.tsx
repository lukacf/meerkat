import clsx from "clsx";
import {
  useEffect,
  useRef,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";

import {
  collectConsoleDockPanelIds,
  normalizeConsoleDockViewState,
  type ConsoleDockTarget,
  type ConsoleDockNode,
  type ConsoleDockSplitNode,
  type ConsoleDockPanelView,
  type ConsoleDockPanelSplitDirection,
  type ConsoleDockTabView,
  type ConsoleDockViewState,
} from "@console-core";

import type { IconRenderer } from "../shared";
import { acquireResizeLock, releaseResizeLock } from "./resize-lock";

export type ConsoleDockProps<TTarget extends ConsoleDockTarget = ConsoleDockTarget> = {
  viewState: ConsoleDockViewState<TTarget>;
  Icon?: IconRenderer | null;
  className?: string;
  tabActions?: ReactNode;
  renderEmptyState?: () => ReactNode;
  renderPanelBody: (panel: ConsoleDockPanelView<TTarget>) => ReactNode;
  renderPanelFooter?: (panel: ConsoleDockPanelView<TTarget>) => ReactNode;
  onCreateTab?: () => void;
  onClosePanel?: (panel: ConsoleDockPanelView<TTarget>) => void;
  onCloseTab?: (tab: ConsoleDockTabView) => void;
  onFocusPanel?: (panel: ConsoleDockPanelView<TTarget>) => void;
  onResizeSplit?: (splitId: string, ratio: number) => void;
  onSelectTab?: (tab: ConsoleDockTabView) => void;
  onSplitPanel?: (panel: ConsoleDockPanelView<TTarget>, direction: ConsoleDockPanelSplitDirection) => void;
};

function splitGlyph(direction: ConsoleDockPanelSplitDirection): string {
  switch (direction) {
    case "left":
      return "←";
    case "right":
      return "→";
    case "up":
      return "↑";
    case "down":
      return "↓";
    default:
      return "+";
  }
}

function PanelActionButton({
  direction,
  panelId,
  onClick,
}: {
  direction: ConsoleDockPanelSplitDirection;
  panelId: string;
  onClick?: () => void;
}) {
  return (
    <button
      aria-label={`Split ${direction}`}
      className="cc-dock-panel__icon-action"
      data-testid={`dock-split:${panelId}:${direction}`}
      type="button"
      onClick={onClick}
    >
      <span aria-hidden="true">{splitGlyph(direction)}</span>
    </button>
  );
}

type PanelNodeViewProps<TTarget extends ConsoleDockTarget> = {
  node: ConsoleDockNode;
  panelsById: Map<string, ConsoleDockPanelView<TTarget>>;
  focusedPanelId: string | null;
  isSinglePanelLayout: boolean;
  Icon?: IconRenderer | null;
  onClosePanel?: (panel: ConsoleDockPanelView<TTarget>) => void;
  onFocusPanel?: (panel: ConsoleDockPanelView<TTarget>) => void;
  onResizeSplit?: (splitId: string, ratio: number) => void;
  onSplitPanel?: (panel: ConsoleDockPanelView<TTarget>, direction: ConsoleDockPanelSplitDirection) => void;
  renderPanelBody: (panel: ConsoleDockPanelView<TTarget>) => ReactNode;
  renderPanelFooter?: (panel: ConsoleDockPanelView<TTarget>) => ReactNode;
};

function PanelNodeView<TTarget extends ConsoleDockTarget>({
  node,
  panelsById,
  focusedPanelId,
  isSinglePanelLayout,
  Icon,
  onClosePanel,
  onFocusPanel,
  onResizeSplit,
  onSplitPanel,
  renderPanelBody,
  renderPanelFooter,
}: PanelNodeViewProps<TTarget>) {
  if (node.kind === "panel") {
    const panel = panelsById.get(node.panelId);

    if (!panel) {
      return null;
    }

    const panelActions = (
      <>
        {(["left", "right", "up", "down"] as const).map((direction) => (
          <PanelActionButton
            direction={direction}
            key={direction}
            panelId={panel.id}
            onClick={() => onSplitPanel?.(panel, direction)}
          />
        ))}
        {panel.closable !== false ? (
          <button
            aria-label="Close panel"
            className="cc-dock-panel__icon-action is-close"
            data-testid={`dock-close:${panel.id}`}
            type="button"
            onClick={() => onClosePanel?.(panel)}
          >
            <span aria-hidden="true">×</span>
          </button>
        ) : null}
      </>
    );

    return (
      <section
        className={clsx(
          "cc-dock-panel",
          isSinglePanelLayout && "is-solitary",
          focusedPanelId === panel.id && "is-focused",
          panel.mode === "terminal" && "is-terminal",
        )}
        data-testid={`dock-panel:${panel.id}`}
        data-panel-id={panel.id}
        onMouseDown={() => onFocusPanel?.(panel)}
      >
        {isSinglePanelLayout ? (
          // Single-panel mode is the pristine conversation surface: no floating
          // split arrows hovering over the first (right-aligned) user message.
          // Layout changes happen through the compact layout control in the tab
          // bar; a solitary panel has nothing to close.
          null
        ) : (
          <header className="cc-dock-panel__header">
            <div className="cc-dock-panel__copy">
              <div className="cc-dock-panel__title-row">
                {panel.iconName && Icon ? (
                  <span className="cc-dock-panel__icon" aria-hidden="true">
                    <Icon name={panel.iconName} />
                  </span>
                ) : null}
                <span className="cc-dock-panel__title">{panel.title}</span>
                {panel.badgeLabel ? <span className="cc-dock-panel__badge">{panel.badgeLabel}</span> : null}
              </div>
              {panel.subtitle || panel.statusLabel ? (
                <div className="cc-dock-panel__meta">
                  {panel.subtitle ? <span>{panel.subtitle}</span> : null}
                  {panel.statusLabel ? <span>{panel.statusLabel}</span> : null}
                </div>
              ) : null}
            </div>
            <div className="cc-dock-panel__actions">
              {panelActions}
            </div>
          </header>
        )}
        <div className="cc-dock-panel__body">
          {renderPanelBody(panel)}
        </div>
        {renderPanelFooter ? (
          <div className="cc-dock-panel__footer">
            {renderPanelFooter(panel)}
          </div>
        ) : null}
      </section>
    );
  }

  return (
    <SplitNodeView
      focusedPanelId={focusedPanelId}
      Icon={Icon}
      isSinglePanelLayout={isSinglePanelLayout}
      node={node}
      onClosePanel={onClosePanel}
      onFocusPanel={onFocusPanel}
      onResizeSplit={onResizeSplit}
      onSplitPanel={onSplitPanel}
      panelsById={panelsById}
      renderPanelBody={renderPanelBody}
      renderPanelFooter={renderPanelFooter}
    />
  );
}

function SplitNodeView<TTarget extends ConsoleDockTarget>({
  node: splitNode,
  panelsById,
  focusedPanelId,
  isSinglePanelLayout,
  Icon,
  onClosePanel,
  onFocusPanel,
  onResizeSplit,
  onSplitPanel,
  renderPanelBody,
  renderPanelFooter,
}: Omit<PanelNodeViewProps<TTarget>, "node"> & { node: ConsoleDockSplitNode }) {
  const splitRef = useRef<HTMLDivElement | null>(null);
  const resizeCleanupRef = useRef<(() => void) | null>(null);
  const firstFlex = typeof splitNode.ratio === "number" && splitNode.ratio > 0 && splitNode.ratio < 1 ? splitNode.ratio : 0.5;
  const secondFlex = 1 - firstFlex;

  useEffect(() => () => {
    resizeCleanupRef.current?.();
    resizeCleanupRef.current = null;
  }, []);

  function handleResizeStart(event: ReactPointerEvent<HTMLButtonElement>) {
    if (!onResizeSplit || !splitRef.current) {
      return;
    }

    resizeCleanupRef.current?.();
    resizeCleanupRef.current = null;

    event.preventDefault();
    event.stopPropagation();
    acquireResizeLock();

    const divider = event.currentTarget;
    const pointerId = event.pointerId;
    let isActive = true;

    const updateRatio = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId !== pointerId) {
        return;
      }
      const splitElement = splitRef.current;
      if (!splitElement || !isActive) {
        return;
      }
      const rect = splitElement.getBoundingClientRect();
      const size = splitNode.direction === "horizontal" ? rect.width : rect.height;
      if (size <= 0) {
        return;
      }
      const offset = splitNode.direction === "horizontal"
        ? pointerEvent.clientX - rect.left
        : pointerEvent.clientY - rect.top;
      const ratio = offset / size;
      onResizeSplit(splitNode.id, Math.min(0.88, Math.max(0.12, ratio)));
    };

    const handlePointerMove = (pointerEvent: PointerEvent) => {
      updateRatio(pointerEvent);
    };

    const cleanup = () => {
      if (!isActive) {
        return;
      }
      isActive = false;
      releaseResizeLock();
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerCancel);
      divider.removeEventListener("lostpointercapture", handleLostPointerCapture);
      if ("hasPointerCapture" in divider && divider.hasPointerCapture(event.pointerId)) {
        divider.releasePointerCapture(event.pointerId);
      }
      resizeCleanupRef.current = null;
    };

    const handlePointerUp = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId !== pointerId) {
        return;
      }
      cleanup();
    };

    const handlePointerCancel = (pointerEvent: PointerEvent) => {
      if (pointerEvent.pointerId !== pointerId) {
        return;
      }
      cleanup();
    };

    const handleLostPointerCapture = () => {
      cleanup();
    };

    updateRatio(event.nativeEvent);
    if ("setPointerCapture" in divider) {
      divider.setPointerCapture(event.pointerId);
    }
    resizeCleanupRef.current = cleanup;
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerCancel);
    divider.addEventListener("lostpointercapture", handleLostPointerCapture);
  }

  return (
    <div className={clsx("cc-dock-split", `is-${splitNode.direction}`)} ref={splitRef}>
      <div className="cc-dock-split__slot" style={{ flex: `${firstFlex} 1 0%` }}>
        <PanelNodeView
          focusedPanelId={focusedPanelId}
          Icon={Icon}
          isSinglePanelLayout={isSinglePanelLayout}
          node={splitNode.first}
          onClosePanel={onClosePanel}
          onFocusPanel={onFocusPanel}
          onResizeSplit={onResizeSplit}
          onSplitPanel={onSplitPanel}
          panelsById={panelsById}
          renderPanelBody={renderPanelBody}
          renderPanelFooter={renderPanelFooter}
        />
      </div>
      <button
        aria-label={`Resize ${splitNode.direction} split`}
        className="cc-dock-split__divider"
        data-testid={`dock-divider:${splitNode.id}`}
        type="button"
        onPointerDown={handleResizeStart}
      >
        <span className="cc-dock-split__divider-line" aria-hidden="true" />
      </button>
      <div className="cc-dock-split__slot" style={{ flex: `${secondFlex} 1 0%` }}>
        <PanelNodeView
          focusedPanelId={focusedPanelId}
          Icon={Icon}
          isSinglePanelLayout={isSinglePanelLayout}
          node={splitNode.second}
          onClosePanel={onClosePanel}
          onFocusPanel={onFocusPanel}
          onResizeSplit={onResizeSplit}
          onSplitPanel={onSplitPanel}
          panelsById={panelsById}
          renderPanelBody={renderPanelBody}
          renderPanelFooter={renderPanelFooter}
        />
      </div>
    </div>
  );
}

export function ConsoleDock<TTarget extends ConsoleDockTarget = ConsoleDockTarget>({
  viewState,
  Icon,
  className,
  tabActions = null,
  renderEmptyState,
  renderPanelBody,
  renderPanelFooter,
  onCreateTab,
  onClosePanel,
  onCloseTab,
  onFocusPanel,
  onResizeSplit,
  onSelectTab,
  onSplitPanel,
}: ConsoleDockProps<TTarget>) {
  const normalized = normalizeConsoleDockViewState<TTarget>(viewState);
  const activeTab = normalized.tabs.find((tab) => tab.id === normalized.activeTabId) || null;
  const activePanelCount = activeTab ? collectConsoleDockPanelIds(activeTab.layout).length : 0;
  const hasMultipleTabs = normalized.tabs.length > 1;
  const hasTabToolbar = Boolean(tabActions) || Boolean(onCreateTab);
  const panelsById = new Map<string, ConsoleDockPanelView<TTarget>>(
    normalized.panels.map((panel) => [panel.id, panel] as const),
  );

  return (
    <section
      className={clsx(
        "cc-theme-scope",
        "cc-dock",
        className,
        !hasMultipleTabs && "is-single-tab",
        activePanelCount <= 1 && "is-single-panel",
      )}
    >
      {hasMultipleTabs || hasTabToolbar ? (
        <header className={clsx("cc-dock__tab-strip", !hasMultipleTabs && normalized.tabs.length === 0 && "is-toolbar-only")}>
          {normalized.tabs.length > 0 ? (
            <div className="cc-dock__tabs" role="tablist" aria-label="Dock tabs">
              {normalized.tabs.map((tab) => (
                <div
                  className={clsx("cc-dock-tab", tab.id === normalized.activeTabId && "is-active")}
                  key={tab.id}
                >
                  <button
                    aria-selected={tab.id === normalized.activeTabId}
                    className="cc-dock-tab__button"
                    role="tab"
                    title={tab.subtitle ? `${tab.title} - ${tab.subtitle}` : tab.title}
                    type="button"
                    onClick={() => onSelectTab?.(tab)}
                  >
                    {Icon && tab.iconName ? (
                      <span className="cc-dock-tab__icon" aria-hidden="true">
                        <Icon name={tab.iconName} />
                      </span>
                    ) : null}
                    <span className="cc-dock-tab__copy">
                      <span className="cc-dock-tab__title">{tab.title}</span>
                    </span>
                    {tab.badgeLabel ? <span className="cc-dock-tab__badge">{tab.badgeLabel}</span> : null}
                  </button>
                  {tab.closable !== false ? (
                    <button
                      aria-label={`Close ${tab.title}`}
                      className="cc-dock-tab__close"
                      type="button"
                      onClick={() => onCloseTab?.(tab)}
                    >
                      <span aria-hidden="true">×</span>
                    </button>
                  ) : null}
                </div>
              ))}
            </div>
          ) : null}
          <div className="cc-dock__tab-actions">
            {tabActions}
            {onCreateTab ? (
              <button aria-label="New tab" className="cc-dock__new-tab" type="button" onClick={onCreateTab}>
                <span aria-hidden="true">+</span>
              </button>
            ) : null}
          </div>
        </header>
      ) : null}
      <div className="cc-dock__body">
        {activeTab ? (
          <PanelNodeView<TTarget>
            focusedPanelId={normalized.focusedPanelId}
            Icon={Icon}
            isSinglePanelLayout={activePanelCount <= 1}
            node={activeTab.layout}
            onClosePanel={onClosePanel}
            onFocusPanel={onFocusPanel}
            onResizeSplit={onResizeSplit}
            onSplitPanel={onSplitPanel}
            panelsById={panelsById}
            renderPanelBody={renderPanelBody}
            renderPanelFooter={renderPanelFooter}
          />
        ) : (
          renderEmptyState ? renderEmptyState() : <div className="cc-dock__empty">Open a tab to start arranging panels.</div>
        )}
      </div>
    </section>
  );
}
