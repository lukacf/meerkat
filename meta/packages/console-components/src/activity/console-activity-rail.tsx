import clsx from "clsx";
import type { RefObject, ReactNode } from "react";

import type {
  ConsoleActivityFeedPanel,
  ConsoleActivityFeedSlot,
  ConsoleActivityItem,
  ConsoleActivityPanel,
  ConsoleActivityPulsePanel,
  ConsoleActivityRailViewState,
  ConsoleActivityRosterPanel,
} from "@console-core";

import { toneStyle, type IconRenderer } from "../shared";

export type ConsoleActivityRailProps = {
  Icon: IconRenderer;
  viewState: ConsoleActivityRailViewState;
  addPanelButtonRef?: RefObject<HTMLButtonElement | null>;
  onTogglePicker: () => void;
  onCollapse: () => void;
  onEmptyAction?: () => void;
  onFooterAction?: () => void;
  onIngressSelect?: () => void;
  onRemovePanel?: (panelId: string) => void;
  onSelectItem?: (focusId: string) => void;
  onTogglePin?: (pinId: string, pinned: boolean) => void;
  onPanelAction?: (panelId: string, actionId: string) => void;
  renderSlotPreview: (slot: ConsoleActivityFeedSlot) => ReactNode;
};

function legacyPanelId(panel: Pick<ConsoleActivityPanel, "id" | "kind">): string | undefined {
  if (panel.id === "chorus" || (panel.kind === "roster" && panel.id.includes("chorus"))) {
    return "watchRailChorusPanel";
  }
  if (panel.id === "busy_member" || (panel.kind === "feed" && panel.id.includes("busy"))) {
    return "watchRailBusyMemberPanel";
  }
  if (panel.id === "pulse" || panel.kind === "pulse") {
    return "watchRailPulsePanel";
  }
  if (panel.id === "jobs" || (panel.kind === "roster" && panel.id.includes("jobs"))) {
    return "watchRailJobsPanel";
  }
  return undefined;
}

function PinButton({
  Icon,
  item,
  onTogglePin,
}: {
  Icon: IconRenderer;
  item: Pick<ConsoleActivityItem, "pinId" | "pinned" | "title">;
  onTogglePin?: (pinId: string, pinned: boolean) => void;
}) {
  if (!item.pinId || !onTogglePin) {
    return null;
  }

  return (
    <button
      type="button"
      className={clsx("cc-activity-rail__pin", item.pinned && "is-active")}
      title={item.pinned ? `Unpin ${item.title}` : `Pin ${item.title}`}
      aria-label={item.pinned ? `Unpin ${item.title}` : `Pin ${item.title}`}
      onClick={(event) => {
        event.stopPropagation();
        onTogglePin(item.pinId as string, Boolean(item.pinned));
      }}
    >
      <Icon name="i-pin" />
    </button>
  );
}

function RosterPanel({
  Icon,
  panel,
  onRemovePanel,
  onSelectItem,
  onTogglePin,
  onPanelAction,
}: {
  Icon: IconRenderer;
  panel: ConsoleActivityRosterPanel;
  onRemovePanel?: (panelId: string) => void;
  onSelectItem?: (focusId: string) => void;
  onTogglePin?: (pinId: string, pinned: boolean) => void;
  onPanelAction?: (panelId: string, actionId: string) => void;
}) {
  return (
    <section className="cc-activity-rail__section" id={legacyPanelId(panel)} key={panel.id}>
      <div className="cc-activity-rail__section-row">
        <h2>{panel.title}</h2>
        <div className="cc-activity-rail__section-actions">
          {panel.meta ? <div className="cc-activity-rail__section-meta">{panel.meta}</div> : null}
          {panel.actions?.map((action) => (
            <button
              key={action.id}
              type="button"
              className={clsx("cc-activity-rail__section-action", action.active && "is-active")}
              data-testid={`activity-action:${panel.id}:${action.id}`}
              onClick={() => onPanelAction?.(panel.id, action.id)}
            >
              {action.label}
            </button>
          ))}
          {onRemovePanel && panel.removable !== false ? (
            <button
              type="button"
              className="cc-activity-rail__section-action"
              onClick={() => onRemovePanel(panel.id)}
            >
              Hide
            </button>
          ) : null}
        </div>
      </div>
      {panel.groups.length ? (
        <div className="cc-activity-rail__roster-groups">
          {panel.groups.map((group) => (
            <section className={clsx("cc-activity-rail__roster-group", group.inactive && "is-inactive")} key={group.id}>
              <div className="cc-activity-rail__roster-group-header">
                <span className="cc-activity-rail__roster-group-title">{group.title}</span>
                {group.meta ? <span className="cc-activity-rail__roster-group-meta">{group.meta}</span> : null}
              </div>
              <div className="cc-activity-rail__roster-grid">
                {group.items.map((item) => (
                  <div
                    className={clsx("cc-activity-rail__roster-item", item.selected && "is-selected")}
                    data-workspace-member-key={panel.itemsRepresentMembers !== false ? (item.focusId || undefined) : undefined}
                    key={item.id}
                    style={toneStyle(item.tone)}
                  >
                    <button
                      type="button"
                      className="cc-activity-rail__roster-main workspace-watch-roster-main"
                      title={item.tooltip || item.subtitle || item.title}
                      onClick={() => item.focusId && onSelectItem?.(item.focusId)}
                    >
                      <span className="cc-activity-rail__roster-status" />
                      <span className="cc-activity-rail__roster-copy">
                        <span className="cc-activity-rail__roster-label">{item.title}</span>
                        <span className="cc-activity-rail__roster-meta">{item.subtitle || item.meta}</span>
                      </span>
                    </button>
                    <PinButton Icon={Icon} item={item} onTogglePin={onTogglePin} />
                  </div>
                ))}
              </div>
            </section>
          ))}
        </div>
      ) : panel.emptyText ? <div className="cc-activity-rail__empty">{panel.emptyText}</div> : null}
    </section>
  );
}

function PulsePanel({
  Icon,
  panel,
  onRemovePanel,
  onPanelAction,
  onSelectItem,
  onTogglePin,
}: {
  Icon: IconRenderer;
  panel: ConsoleActivityPulsePanel;
  onRemovePanel?: (panelId: string) => void;
  onPanelAction?: (panelId: string, actionId: string) => void;
  onSelectItem?: (focusId: string) => void;
  onTogglePin?: (pinId: string, pinned: boolean) => void;
}) {
  return (
    <section className="cc-activity-rail__section" id={legacyPanelId(panel)} key={panel.id}>
      <div className="cc-activity-rail__section-row">
        <h2>{panel.title}</h2>
        <div className="cc-activity-rail__section-actions">
          {panel.meta ? <div className="cc-activity-rail__section-meta">{panel.meta}</div> : null}
          {panel.actions?.map((action) => (
            <button
              key={action.id}
              type="button"
              className={clsx("cc-activity-rail__section-action", action.active && "is-active")}
              data-testid={`activity-action:${panel.id}:${action.id}`}
              onClick={() => onPanelAction?.(panel.id, action.id)}
            >
              {action.label}
            </button>
          ))}
          {onRemovePanel ? (
            <button
              type="button"
              className="cc-activity-rail__section-action"
              onClick={() => onRemovePanel(panel.id)}
            >
              Hide
            </button>
          ) : null}
        </div>
      </div>
      <div className="cc-activity-rail__pulse-list">
        {panel.items.length ? panel.items.map((item) => (
          <div
            className={clsx("cc-activity-rail__pulse-row", item.selected && "is-selected")}
            key={item.id}
            style={toneStyle(item.tone)}
          >
            <button
              type="button"
              className="cc-activity-rail__pulse-main"
              data-testid={`activity-item:${panel.id}:${item.id}`}
              title={item.tooltip || `${item.title} · ${item.line}`}
              onClick={() => item.focusId && onSelectItem?.(item.focusId)}
            >
              <span className="cc-activity-rail__pulse-status" />
              <span className="cc-activity-rail__pulse-copy">
                <span className="cc-activity-rail__pulse-head">
                  <span className="cc-activity-rail__pulse-label">{item.title}</span>
                  <span className="cc-activity-rail__pulse-time">{item.meta}</span>
                </span>
                <span className="cc-activity-rail__pulse-line">{item.line}</span>
              </span>
            </button>
            <PinButton Icon={Icon} item={item} onTogglePin={onTogglePin} />
          </div>
        )) : (
          <div className="cc-activity-rail__empty">{panel.emptyText}</div>
        )}
      </div>
    </section>
  );
}

function FeedPanel({
  Icon,
  panel,
  onRemovePanel,
  onSelectItem,
  onTogglePin,
  onPanelAction,
  renderSlotPreview,
}: {
  Icon: IconRenderer;
  panel: ConsoleActivityFeedPanel;
  onRemovePanel?: (panelId: string) => void;
  onSelectItem?: (focusId: string) => void;
  onTogglePin?: (pinId: string, pinned: boolean) => void;
  onPanelAction?: (panelId: string, actionId: string) => void;
  renderSlotPreview: (slot: ConsoleActivityFeedSlot) => ReactNode;
}) {
  return (
    <section className="cc-activity-rail__section cc-activity-rail__section--feed" id={legacyPanelId(panel)} key={panel.id}>
      <div className="cc-activity-rail__section-row">
        <h2>{panel.title}</h2>
        <div className="cc-activity-rail__section-actions">
          {panel.actions?.map((action) => (
            <button
              key={action.id}
              type="button"
              className={clsx("cc-activity-rail__section-action", action.active && "is-active")}
              onClick={() => onPanelAction?.(panel.id, action.id)}
            >
              {action.label}
            </button>
          ))}
          {onRemovePanel ? (
            <button
              type="button"
              className="cc-activity-rail__section-action"
              onClick={() => onRemovePanel(panel.id)}
            >
              Hide
            </button>
          ) : null}
        </div>
      </div>
      <div className="cc-activity-rail__feed-slots">
        {panel.slots.map((slot) => (
          <section
            className={clsx("cc-activity-rail__feed-item", slot.selected && "is-selected", slot.focusId && "has-item")}
            key={slot.id}
            style={toneStyle(slot.tone)}
          >
            <div className="cc-activity-rail__feed-frame">
              <button
                type="button"
                className="cc-activity-rail__feed-button"
                title={slot.subtitle || slot.title}
                onClick={() => slot.focusId && onSelectItem?.(slot.focusId)}
                disabled={!slot.focusId}
              >
                <div className="cc-activity-rail__feed-canvas">
                  {renderSlotPreview(slot)}
                  <div className="cc-activity-rail__feed-overlay">
                    <div className="cc-activity-rail__feed-overlay-top">
                      <span className="cc-activity-rail__feed-eyebrow">{slot.eyebrow}</span>
                      <span className="cc-activity-rail__feed-title">{slot.title}</span>
                      <span className="cc-activity-rail__feed-meta">{slot.meta}</span>
                    </div>
                  </div>
                </div>
              </button>
              <div className="cc-activity-rail__feed-actions">
                <PinButton
                  Icon={Icon}
                  item={{
                    pinId: slot.pinId,
                    pinned: slot.pinned,
                    title: slot.title,
                  }}
                  onTogglePin={onTogglePin}
                />
              </div>
            </div>
          </section>
        ))}
      </div>
    </section>
  );
}

function renderPanel({
  Icon,
  panel,
  onRemovePanel,
  onSelectItem,
  onTogglePin,
  onPanelAction,
  renderSlotPreview,
}: {
  Icon: IconRenderer;
  panel: ConsoleActivityPanel;
  onRemovePanel?: (panelId: string) => void;
  onSelectItem?: (focusId: string) => void;
  onTogglePin?: (pinId: string, pinned: boolean) => void;
  onPanelAction?: (panelId: string, actionId: string) => void;
  renderSlotPreview: (slot: ConsoleActivityFeedSlot) => ReactNode;
}) {
  if (panel.kind === "roster") {
    return (
      <RosterPanel
          Icon={Icon}
          key={panel.id}
          onRemovePanel={onRemovePanel}
          onPanelAction={onPanelAction}
          onSelectItem={onSelectItem}
          onTogglePin={onTogglePin}
          panel={panel}
        />
    );
  }

  if (panel.kind === "pulse") {
    return (
      <PulsePanel
        Icon={Icon}
        key={panel.id}
        onRemovePanel={onRemovePanel}
        onPanelAction={onPanelAction}
        onSelectItem={onSelectItem}
        onTogglePin={onTogglePin}
        panel={panel}
      />
    );
  }

  return (
    <FeedPanel
      Icon={Icon}
      key={panel.id}
      onPanelAction={onPanelAction}
      onRemovePanel={onRemovePanel}
      onSelectItem={onSelectItem}
      onTogglePin={onTogglePin}
      panel={panel}
      renderSlotPreview={renderSlotPreview}
    />
  );
}

export function ConsoleActivityRail({
  Icon,
  viewState,
  addPanelButtonRef,
  onTogglePicker,
  onCollapse,
  onEmptyAction,
  onFooterAction,
  onIngressSelect,
  onRemovePanel,
  onSelectItem,
  onTogglePin,
  onPanelAction,
  renderSlotPreview,
}: ConsoleActivityRailProps) {
  return (
    <div className="cc-theme-scope cc-activity-rail" id="threadWatchRail">
      <div className="cc-activity-rail__controls">
        <button
          ref={addPanelButtonRef}
          className="cc-activity-rail__control"
          type="button"
          title="Add panel"
          aria-label="Add panel"
          onClick={onTogglePicker}
        >
          <Icon name="i-plus" />
        </button>
        <button
          className="cc-activity-rail__control"
          type="button"
          title="Collapse live panels"
          aria-label="Collapse live panels"
          onClick={onCollapse}
        >
          <Icon name="i-sidebar-toggle" />
        </button>
      </div>
      <aside className="cc-activity-rail__rail">
        <div className="cc-activity-rail__scroll">
          {viewState.ingress ? (
            <div className="cc-activity-rail__ingress">
              <button
                id="watchRailIngressBtn"
                type="button"
                className={clsx(
                  "cc-activity-rail__ingress-button",
                  viewState.ingress.active && "is-active",
                  viewState.ingress.prominent && "is-prominent",
                )}
                onClick={onIngressSelect}
              >
                <span className="cc-activity-rail__ingress-status" aria-hidden="true" />
                <span className="cc-activity-rail__ingress-copy">
                  <span className="cc-activity-rail__ingress-title">{viewState.ingress.label}</span>
                  <span className="cc-activity-rail__ingress-meta">{viewState.ingress.meta}</span>
                </span>
              </button>
            </div>
          ) : null}
          {viewState.panels.length ? (
            <div className="cc-activity-rail__content">
              {viewState.panels.map((panel) => renderPanel({
                Icon,
                panel,
                onRemovePanel,
                onSelectItem,
                onTogglePin,
                onPanelAction,
                renderSlotPreview,
              }))}
            </div>
          ) : viewState.emptyState ? (
            <div className="cc-activity-rail__empty-shell">
              <div className="cc-activity-rail__empty-title">{viewState.emptyState.title}</div>
              <div className="cc-activity-rail__empty-copy">{viewState.emptyState.description}</div>
              <button
                id={viewState.emptyState.actionLabel === "Restore helpers" ? "threadWatchRestoreBtn" : undefined}
                type="button"
                className="cc-activity-rail__empty-action"
                onClick={onEmptyAction || onTogglePicker}
              >
                {viewState.emptyState.actionLabel}
              </button>
            </div>
          ) : null}
        </div>
        {viewState.footerActionLabel && onFooterAction ? (
          <div className="cc-activity-rail__footer">
            <button className="cc-activity-rail__footer-button" type="button" onClick={onFooterAction}>
              <Icon name="i-team" />
              <span>{viewState.footerActionLabel}</span>
            </button>
          </div>
        ) : null}
      </aside>
    </div>
  );
}
