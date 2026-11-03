import clsx from "clsx";
import {
  Fragment,
  type ButtonHTMLAttributes,
  type DragEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type Ref,
} from "react";

import {
  normalizeConsoleSidebarViewState,
  type ConsoleSidebarAction,
  type ConsoleSidebarBlock,
  type ConsoleSidebarItem,
  type ConsoleSidebarSection,
  type ConsoleSidebarViewState,
} from "@console-core";

import type { IconRenderer } from "../shared";

export type ConsoleSidebarSectionHeaderRenderArgs = {
  block: ConsoleSidebarBlock;
  section: ConsoleSidebarSection;
  defaultHeader: ReactNode;
};

export type ConsoleSidebarSectionContainerRenderArgs = {
  block: ConsoleSidebarBlock;
  section: ConsoleSidebarSection;
  defaultSection: ReactNode;
};

export type ConsoleSidebarSectionItemsRenderArgs = {
  block: ConsoleSidebarBlock;
  section: ConsoleSidebarSection;
  /**
   * Fully wired row nodes in the same order as `section.items`.
   * Consumers may group or annotate them without reimplementing row
   * interaction, drag-and-drop, actions, or keyboard behavior.
   */
  defaultItems: ReactNode[];
};

export type ConsoleSidebarItemTrailingRenderArgs = {
  block: ConsoleSidebarBlock;
  section: ConsoleSidebarSection;
  item: ConsoleSidebarItem;
};

type SidebarActionButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  ref?: Ref<HTMLButtonElement>;
};

export type ConsoleSidebarActionButtonScope =
  | {
      kind: "block";
      block: ConsoleSidebarBlock;
      action: ConsoleSidebarAction;
    }
  | {
      kind: "section";
      block: ConsoleSidebarBlock;
      section: ConsoleSidebarSection;
      action: ConsoleSidebarAction;
    }
  | {
      kind: "item";
      block: ConsoleSidebarBlock;
      section: ConsoleSidebarSection;
      item: ConsoleSidebarItem;
      action: ConsoleSidebarAction;
    };

export type ConsoleSidebarProps = {
  viewState: ConsoleSidebarViewState;
  Icon?: IconRenderer | null;
  className?: string;
  getActionButtonProps?: (scope: ConsoleSidebarActionButtonScope) => SidebarActionButtonProps;
  renderSectionHeader?: (args: ConsoleSidebarSectionHeaderRenderArgs) => ReactNode;
  renderSectionContainer?: (args: ConsoleSidebarSectionContainerRenderArgs) => ReactNode;
  renderSectionItems?: (args: ConsoleSidebarSectionItemsRenderArgs) => ReactNode;
  renderItemTrailing?: (args: ConsoleSidebarItemTrailingRenderArgs) => ReactNode;
  onBlockAction?: (block: ConsoleSidebarBlock, action: ConsoleSidebarAction) => void;
  onSelectSection?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection) => void;
  onSectionAction?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, action: ConsoleSidebarAction) => void;
  onSelectItem?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, item: ConsoleSidebarItem) => void;
  onItemAction?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    action: ConsoleSidebarAction,
  ) => void;
  onItemContextMenu?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: MouseEvent<HTMLDivElement>,
  ) => void;
  isItemDraggable?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, item: ConsoleSidebarItem) => boolean;
  isItemDropTarget?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event?: DragEvent<HTMLDivElement>,
  ) => boolean;
  onItemDragStart?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
  onItemDragEnd?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
  onItemDrop?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
};

function SectionIconButton({
  action,
  Icon,
  buttonProps,
  className,
  onClick,
}: {
  action: ConsoleSidebarAction;
  Icon?: IconRenderer | null;
  buttonProps?: SidebarActionButtonProps;
  className?: string;
  onClick?: () => void;
}) {
  return (
    <button
      {...buttonProps}
      aria-label={action.label}
      className={clsx("cc-sidebar-icon-action", action.active && "is-active", className, buttonProps?.className)}
      disabled={action.disabled || buttonProps?.disabled}
      title={action.label}
      type="button"
      onClick={(event) => {
        event.stopPropagation();
        buttonProps?.onClick?.(event);
        if (!event.defaultPrevented) {
          onClick?.();
        }
      }}
    >
      {Icon && action.iconName ? <Icon name={action.iconName} /> : null}
    </button>
  );
}

function ActionStrip({
  block,
  Icon,
  getActionButtonProps,
  onBlockAction,
}: {
  block: ConsoleSidebarBlock;
  Icon?: IconRenderer | null;
  getActionButtonProps?: (scope: ConsoleSidebarActionButtonScope) => SidebarActionButtonProps;
  onBlockAction?: (block: ConsoleSidebarBlock, action: ConsoleSidebarAction) => void;
}) {
  if (!block.actions?.length) {
    return null;
  }

  return (
    <section className="cc-sidebar-block cc-sidebar-block--action-strip">
      <div className="cc-sidebar-action-strip">
        {block.actions.map((action, actionIndex) => {
          const buttonProps = getActionButtonProps?.({ kind: "block", block, action });

          return (
            <Fragment key={`${block.id}:${action.id}:${actionIndex}`}>
              <button
                {...buttonProps}
                className={clsx("cc-sidebar-action-strip__button", action.active && "is-active", buttonProps?.className)}
                disabled={action.disabled || buttonProps?.disabled}
                type="button"
                onClick={(event) => {
                  buttonProps?.onClick?.(event);
                  if (!event.defaultPrevented) {
                    onBlockAction?.(block, action);
                  }
                }}
              >
                {Icon && action.iconName ? (
                  <span className="cc-sidebar-action-strip__icon" aria-hidden="true">
                    <Icon name={action.iconName} />
                  </span>
                ) : null}
                <span>{action.label}</span>
              </button>
            </Fragment>
          );
        })}
      </div>
    </section>
  );
}

function BlockHeader({
  block,
  Icon,
  getActionButtonProps,
  onBlockAction,
}: {
  block: ConsoleSidebarBlock;
  Icon?: IconRenderer | null;
  getActionButtonProps?: (scope: ConsoleSidebarActionButtonScope) => SidebarActionButtonProps;
  onBlockAction?: (block: ConsoleSidebarBlock, action: ConsoleSidebarAction) => void;
}) {
  if (!block.title && !block.meta?.length && !block.actions?.length) {
    return null;
  }

  return (
    <div className="cc-sidebar-block__header">
      <div className="cc-sidebar-block__copy">
        {block.title ? <h2 className="cc-sidebar-block__title">{block.title}</h2> : null}
        {block.meta?.length ? (
          <div className="cc-sidebar-block__meta">
            {block.meta.map((meta) => (
              <span className={clsx("cc-sidebar-meta", meta.tone && `is-${meta.tone}`)} key={meta.id || meta.label}>
                {Icon && meta.iconName ? <Icon className="cc-sidebar-meta__icon" name={meta.iconName} /> : null}
                <span>{meta.label}</span>
              </span>
            ))}
          </div>
        ) : null}
      </div>
      {block.actions?.length ? (
        <div className="cc-sidebar-block__actions">
          {block.actions.map((action, actionIndex) => (
            <SectionIconButton
              action={action}
              buttonProps={getActionButtonProps?.({ kind: "block", block, action })}
              Icon={Icon}
              key={`${block.id}:${action.id}:${actionIndex}`}
              onClick={() => onBlockAction?.(block, action)}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function hasVisibleSectionHeader(section: ConsoleSidebarSection): boolean {
  return Boolean(
    section.title
    || section.subtitle
    || section.iconName
    || section.meta?.length
    || section.actions?.length,
  );
}

function DefaultSectionHeader({
  block,
  section,
  Icon,
  getActionButtonProps,
  onSelectSection,
  onSectionAction,
}: {
  block: ConsoleSidebarBlock;
  section: ConsoleSidebarSection;
  Icon?: IconRenderer | null;
  getActionButtonProps?: (scope: ConsoleSidebarActionButtonScope) => SidebarActionButtonProps;
  onSelectSection?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection) => void;
  onSectionAction?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, action: ConsoleSidebarAction) => void;
}) {
  if (!hasVisibleSectionHeader(section)) {
    return null;
  }

  return (
    <div className={clsx("cc-sidebar-section__header", section.selected && "is-selected")}>
      <button
        className="cc-sidebar-section__header-main"
        type="button"
        onClick={() => onSelectSection?.(block, section)}
      >
        {section.iconName && Icon ? (
          <span className="cc-sidebar-section__header-icon" aria-hidden="true">
            <Icon name={section.iconName} />
          </span>
        ) : null}
        <span className="cc-sidebar-section__header-copy">
          <span className="cc-sidebar-section__header-title-row">
            <span className="cc-sidebar-section__header-title">{section.title}</span>
            {section.meta?.length ? (
              <span className="cc-sidebar-section__header-meta">
                {section.meta.map((meta) => (
                  <span className={clsx("cc-sidebar-meta", meta.tone && `is-${meta.tone}`)} key={meta.id || meta.label}>
                    {Icon && meta.iconName ? <Icon className="cc-sidebar-meta__icon" name={meta.iconName} /> : null}
                    <span>{meta.label}</span>
                  </span>
                ))}
              </span>
            ) : null}
          </span>
          {section.subtitle ? (
            <span className="cc-sidebar-section__header-subtitle">{section.subtitle}</span>
          ) : null}
        </span>
      </button>
      {section.actions?.length ? (
        <div className="cc-sidebar-section__header-actions">
          {section.actions.map((action, actionIndex) => (
            <SectionIconButton
              action={action}
              className="cc-sidebar-section__action"
              Icon={Icon}
              buttonProps={getActionButtonProps?.({ kind: "section", block, section, action })}
              key={`${section.id}:${action.id}:${actionIndex}`}
              onClick={() => onSectionAction?.(block, section, action)}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function SidebarRow({
  block,
  section,
  item,
  Icon,
  getActionButtonProps,
  trailingContent,
  onSelectItem,
  onItemAction,
  onItemContextMenu,
  isItemDraggable,
  isItemDropTarget,
  onItemDragStart,
  onItemDragEnd,
  onItemDrop,
}: {
  block: ConsoleSidebarBlock;
  section: ConsoleSidebarSection;
  item: ConsoleSidebarItem;
  Icon?: IconRenderer | null;
  getActionButtonProps?: (scope: ConsoleSidebarActionButtonScope) => SidebarActionButtonProps;
  trailingContent?: ReactNode;
  onSelectItem?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, item: ConsoleSidebarItem) => void;
  onItemAction?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    action: ConsoleSidebarAction,
  ) => void;
  onItemContextMenu?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: MouseEvent<HTMLDivElement>,
  ) => void;
  isItemDraggable?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, item: ConsoleSidebarItem) => boolean;
  isItemDropTarget?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event?: DragEvent<HTMLDivElement>,
  ) => boolean;
  onItemDragStart?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
  onItemDragEnd?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
  onItemDrop?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
}) {
  const draggable = Boolean(!item.disabled && isItemDraggable?.(block, section, item));

  function handleKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (!item.disabled) {
        onSelectItem?.(block, section, item);
      }
    }
  }

  return (
    <div
      // No aria-label: per accname rules it would REPLACE the content-derived
      // accessible name, silencing subtitle, unread badge, and meta chips
      // (including the member-health chip) for screen readers.
      className={clsx(
        "cc-sidebar-row",
        "thread-row",
        item.selected && "is-selected",
        item.unread && "is-unread",
        item.disabled && "is-disabled",
      )}
      data-console-sidebar-part="row"
      data-selected={item.selected ? "true" : "false"}
      data-unread={item.unread ? "true" : "false"}
      data-disabled={item.disabled ? "true" : "false"}
      draggable={draggable}
      role="button"
      tabIndex={item.disabled ? -1 : 0}
      onClick={() => {
        if (!item.disabled) {
          onSelectItem?.(block, section, item);
        }
      }}
      onContextMenu={(event) => onItemContextMenu?.(block, section, item, event)}
      onDragEnd={(event) => {
        if (draggable) {
          onItemDragEnd?.(block, section, item, event);
        }
      }}
      onDragOver={(event) => {
        if (isItemDropTarget?.(block, section, item, event)) {
          event.preventDefault();
          event.dataTransfer.dropEffect = "link";
        }
      }}
      onDragStart={(event) => {
        if (!draggable) {
          event.preventDefault();
          return;
        }
        event.dataTransfer.effectAllowed = "linkMove";
        event.dataTransfer.setData("text/plain", item.id);
        event.dataTransfer.setData("application/x-console-sidebar-item-id", item.id);
        onItemDragStart?.(block, section, item, event);
      }}
      onDrop={(event) => {
        if (isItemDropTarget?.(block, section, item, event)) {
          event.preventDefault();
          onItemDrop?.(block, section, item, event);
        }
      }}
      onKeyDown={handleKeyDown}
    >
      <span className="cc-sidebar-row__main">
        <span className="cc-sidebar-row__copy">
          <span className="cc-sidebar-row__title-row">
            {item.iconName && Icon ? (
              <span className="cc-sidebar-row__icon" aria-hidden="true">
                <Icon name={item.iconName} />
              </span>
            ) : null}
            <span className="cc-sidebar-row__title">{item.title}</span>
          </span>
          {item.subtitle ? <span className="cc-sidebar-row__subtitle">{item.subtitle}</span> : null}
        </span>
      </span>
      <span className="cc-sidebar-row__meta">
        {item.badgeIconName && Icon ? (
          <span className="cc-sidebar-row__badge" aria-hidden="true">
            <Icon name={item.badgeIconName} />
          </span>
        ) : null}
        {item.badgeLabel ? <span className="cc-sidebar-row__badge-label">{item.badgeLabel}</span> : null}
        {item.meta?.map((meta) => (
          <span className={clsx("cc-sidebar-row__meta-item", meta.tone && `is-${meta.tone}`)} key={meta.id || meta.label}>
            {Icon && meta.iconName ? <Icon className="cc-sidebar-row__meta-icon" name={meta.iconName} /> : null}
            <span>{meta.label}</span>
          </span>
        ))}
      </span>
      {(trailingContent || (item.actions?.length && onItemAction)) ? (
        <span className="cc-sidebar-row__trailing">
          {trailingContent ? <span className="cc-sidebar-row__trailing-content">{trailingContent}</span> : null}
          {item.actions?.length && onItemAction ? (
            <span className="cc-sidebar-row__actions">
              {item.actions.map((action, actionIndex) => (
                <SectionIconButton
                  action={action}
                  className="cc-sidebar-row__action"
                  Icon={Icon}
                  buttonProps={getActionButtonProps?.({ kind: "item", block, section, item, action })}
                  key={`${item.id}:${action.id}:${actionIndex}`}
                  onClick={() => onItemAction(block, section, item, action)}
                />
              ))}
            </span>
          ) : null}
        </span>
      ) : null}
    </div>
  );
}

function ListBlock({
  block,
  Icon,
  getActionButtonProps,
  renderSectionHeader,
  renderSectionContainer,
  renderSectionItems,
  renderItemTrailing,
  onBlockAction,
  onSelectSection,
  onSectionAction,
  onSelectItem,
  onItemAction,
  onItemContextMenu,
  isItemDraggable,
  isItemDropTarget,
  onItemDragStart,
  onItemDragEnd,
  onItemDrop,
}: {
  block: ConsoleSidebarBlock;
  Icon?: IconRenderer | null;
  getActionButtonProps?: (scope: ConsoleSidebarActionButtonScope) => SidebarActionButtonProps;
  renderSectionHeader?: (args: ConsoleSidebarSectionHeaderRenderArgs) => ReactNode;
  renderSectionContainer?: (args: ConsoleSidebarSectionContainerRenderArgs) => ReactNode;
  renderSectionItems?: (args: ConsoleSidebarSectionItemsRenderArgs) => ReactNode;
  renderItemTrailing?: (args: ConsoleSidebarItemTrailingRenderArgs) => ReactNode;
  onBlockAction?: (block: ConsoleSidebarBlock, action: ConsoleSidebarAction) => void;
  onSelectSection?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection) => void;
  onSectionAction?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, action: ConsoleSidebarAction) => void;
  onSelectItem?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, item: ConsoleSidebarItem) => void;
  onItemAction?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    action: ConsoleSidebarAction,
  ) => void;
  onItemContextMenu?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: MouseEvent<HTMLDivElement>,
  ) => void;
  isItemDraggable?: (block: ConsoleSidebarBlock, section: ConsoleSidebarSection, item: ConsoleSidebarItem) => boolean;
  isItemDropTarget?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event?: DragEvent<HTMLDivElement>,
  ) => boolean;
  onItemDragStart?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
  onItemDragEnd?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
  onItemDrop?: (
    block: ConsoleSidebarBlock,
    section: ConsoleSidebarSection,
    item: ConsoleSidebarItem,
    event: DragEvent<HTMLDivElement>,
  ) => void;
}) {
  return (
    <section className="cc-sidebar-block cc-sidebar-block--list">
      <BlockHeader
        Icon={Icon}
        block={block}
        getActionButtonProps={getActionButtonProps}
        onBlockAction={onBlockAction}
      />
      <div className="cc-sidebar-list">
        {(block.sections || []).map((section) => {
          const defaultHeader = (
            <DefaultSectionHeader
              Icon={Icon}
              block={block}
              getActionButtonProps={getActionButtonProps}
              onSectionAction={onSectionAction}
              onSelectSection={onSelectSection}
              section={section}
            />
          );
          const header = renderSectionHeader
            ? renderSectionHeader({ block, section, defaultHeader })
            : defaultHeader;
          const defaultItems = section.items.map((item) => (
            <SidebarRow
              Icon={Icon}
              block={block}
              getActionButtonProps={getActionButtonProps}
              item={item}
              isItemDraggable={isItemDraggable}
              isItemDropTarget={isItemDropTarget}
              key={item.id}
              onItemAction={onItemAction}
              onItemContextMenu={onItemContextMenu}
              onItemDragEnd={onItemDragEnd}
              onItemDragStart={onItemDragStart}
              onItemDrop={onItemDrop}
              onSelectItem={onSelectItem}
              section={section}
              trailingContent={renderItemTrailing?.({ block, section, item })}
            />
          ));
          const defaultSection = (
            <section className={clsx("cc-sidebar-section", hasVisibleSectionHeader(section) && "has-header")} key={section.id}>
              {header}
              <div className="cc-sidebar-section__rows">
                {renderSectionItems
                  ? renderSectionItems({ block, section, defaultItems })
                  : defaultItems}
              </div>
            </section>
          );

          return (
            <Fragment key={section.id}>
              {renderSectionContainer
                ? renderSectionContainer({ block, section, defaultSection })
                : defaultSection}
            </Fragment>
          );
        })}
      </div>
    </section>
  );
}

export function ConsoleSidebar({
  viewState,
  Icon,
  className,
  getActionButtonProps,
  renderSectionHeader,
  renderSectionContainer,
  renderSectionItems,
  renderItemTrailing,
  onBlockAction,
  onSelectSection,
  onSectionAction,
  onSelectItem,
  onItemAction,
  onItemContextMenu,
  isItemDraggable,
  isItemDropTarget,
  onItemDragStart,
  onItemDragEnd,
  onItemDrop,
}: ConsoleSidebarProps) {
  const normalizedViewState = normalizeConsoleSidebarViewState(viewState);

  return (
    <div className={clsx("cc-theme-scope", "cc-console-sidebar", className)}>
      {normalizedViewState.blocks.map((block) => (
        block.kind === "action_strip" ? (
          <ActionStrip
            Icon={Icon}
            block={block}
            getActionButtonProps={getActionButtonProps}
            key={block.id}
            onBlockAction={onBlockAction}
          />
        ) : (
          <ListBlock
            Icon={Icon}
            block={block}
            getActionButtonProps={getActionButtonProps}
            isItemDraggable={isItemDraggable}
            isItemDropTarget={isItemDropTarget}
            key={block.id}
            onBlockAction={onBlockAction}
            onItemAction={onItemAction}
            onItemContextMenu={onItemContextMenu}
            onItemDragEnd={onItemDragEnd}
            onItemDragStart={onItemDragStart}
            onItemDrop={onItemDrop}
            onSelectItem={onSelectItem}
            onSectionAction={onSectionAction}
            onSelectSection={onSelectSection}
            renderItemTrailing={renderItemTrailing}
            renderSectionHeader={renderSectionHeader}
            renderSectionContainer={renderSectionContainer}
            renderSectionItems={renderSectionItems}
          />
        )
      ))}
    </div>
  );
}
