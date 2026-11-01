import {
  useCallback,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";

import { IconButton } from "./button";
import { cx } from "./cx";
import { KitGlyph, type KitGlyphName } from "./glyphs";
import { Popover, type PopoverCloseReason, type PopoverPlacement } from "./popover";

/** A command: activating it closes the menu. */
export interface MenuCommandEntry {
  kind: "item";
  id: string;
  label: ReactNode;
  /** The text typeahead matches when `label` is not a string. */
  textValue?: string;
  onSelect: () => void;
  disabled?: boolean;
  /** Negative ink (Retire, Delete). */
  destructive?: boolean;
  /** A shortcut hint at the end of the row ("F", "0"). */
  shortcut?: string;
  glyph?: KitGlyphName;
  testId?: string;
}

/** An on/off option: toggling it keeps the menu open. */
export interface MenuCheckboxEntry {
  kind: "checkbox";
  id: string;
  label: ReactNode;
  textValue?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  testId?: string;
}

export interface MenuRadioOption {
  id: string;
  label: ReactNode;
  textValue?: string;
  disabled?: boolean;
  testId?: string;
}

/** One choice of several, under a visible micro-label: choosing keeps the
 * menu open. `foldOnly` shows it only in a pane under 480 px (a view switch
 * folded into the menu). */
export interface MenuRadioEntry {
  kind: "radio";
  id: string;
  label: string;
  value: string;
  options: ReadonlyArray<MenuRadioOption>;
  onChange: (id: string) => void;
  foldOnly?: boolean;
}

export interface MenuSeparatorEntry {
  kind: "separator";
  id?: string;
  foldOnly?: boolean;
}

export type MenuEntry = MenuCommandEntry | MenuCheckboxEntry | MenuRadioEntry | MenuSeparatorEntry;

/** Props to spread on a custom menu trigger. */
export interface MenuTriggerProps {
  ref: (element: HTMLButtonElement | null) => void;
  id: string;
  "aria-haspopup": "menu";
  "aria-expanded": boolean;
  "aria-controls": string | undefined;
  onClick: (event: MouseEvent<HTMLButtonElement>) => void;
  onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void;
}

const ITEM_SELECTOR = '[role="menuitem"], [role="menuitemcheckbox"], [role="menuitemradio"]';

function visibleItems(menu: HTMLElement | null): HTMLElement[] {
  if (!menu) return [];
  return Array.from(menu.querySelectorAll<HTMLElement>(ITEM_SELECTOR)).filter((item) => {
    if (item.getAttribute("aria-disabled") === "true") return false;
    const check = (item as HTMLElement & { checkVisibility?: () => boolean }).checkVisibility;
    return typeof check === "function" ? check.call(item) : true;
  });
}

function itemText(item: HTMLElement): string {
  return (item.getAttribute("data-text") || item.textContent || "").trim().toLowerCase();
}

/**
 * A menu of panel commands behind a quiet icon button (spec A3.2): commands,
 * checkboxes, radio groups and separators, with the WAI-ARIA menu button
 * keyboard (Enter, Space or ArrowDown opens on the first item, ArrowUp on
 * the last; Up and Down wrap; Home and End; first-letter typeahead; Escape
 * closes and returns focus; Tab closes and moves on).
 */
export function OverflowMenu({
  label,
  items,
  placement = "bottom-end",
  size = "md",
  glyph = "ellipsis",
  quiet,
  reveal,
  badge,
  testId,
  triggerTestId,
  triggerClassName,
  className,
  renderTrigger,
  onOpenChange,
  minWidth = 200,
}: {
  /** The trigger's accessible name: "More actions for Topology". */
  label: string;
  items: ReadonlyArray<MenuEntry | null | false | undefined>;
  placement?: PopoverPlacement;
  /** The default trigger: 28 px (`md`, headers) or 24 px (`sm`). */
  size?: "md" | "sm";
  glyph?: KitGlyphName;
  quiet?: boolean;
  reveal?: boolean;
  badge?: boolean;
  /** On the menu. */
  testId?: string;
  triggerTestId?: string;
  triggerClassName?: string;
  /** On the menu. */
  className?: string;
  /** Replaces the default icon button; spread the props on your button. */
  renderTrigger?: (props: MenuTriggerProps, open: boolean) => ReactNode;
  onOpenChange?: (open: boolean) => void;
  minWidth?: number;
}) {
  const [open, setOpenState] = useState(false);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const focusOnOpen = useRef<"first" | "last">("first");
  const baseId = useId();
  const menuId = `${baseId}-menu`;
  const triggerId = `${baseId}-trigger`;
  const entries = items.filter((item): item is MenuEntry => Boolean(item));

  const setOpen = useCallback(
    (next: boolean) => {
      setOpenState(next);
      onOpenChange?.(next);
    },
    [onOpenChange],
  );

  const close = useCallback(
    (reason: PopoverCloseReason) => {
      setOpen(false);
      if (reason === "select") triggerRef.current?.focus();
    },
    [setOpen],
  );

  const onTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || ((event.key === "Enter" || event.key === " ") && !open)) {
      event.preventDefault();
      focusOnOpen.current = event.key === "ArrowUp" ? "last" : "first";
      if (open) {
        const list = visibleItems(menuRef.current);
        (event.key === "ArrowUp" ? list[list.length - 1] : list[0])?.focus();
      } else {
        setOpen(true);
      }
    }
  };

  const onMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const list = visibleItems(menuRef.current);
    if (!list.length) return;
    const current = list.findIndex((item) => item === event.target);
    const focusAt = (index: number) => list[(index + list.length) % list.length]?.focus();
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        focusAt(current + 1);
        return;
      case "ArrowUp":
        event.preventDefault();
        focusAt(current < 0 ? list.length - 1 : current - 1);
        return;
      case "Home":
        event.preventDefault();
        focusAt(0);
        return;
      case "End":
        event.preventDefault();
        focusAt(list.length - 1);
        return;
      case "Tab":
        setOpen(false);
        return;
      default:
        break;
    }
    if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey && /\S/.test(event.key)) {
      const key = event.key.toLowerCase();
      for (let step = 1; step <= list.length; step += 1) {
        const candidate = list[(Math.max(current, 0) + step) % list.length];
        if (candidate && itemText(candidate).startsWith(key)) {
          event.preventDefault();
          candidate.focus();
          return;
        }
      }
    }
  };

  const triggerProps: MenuTriggerProps = {
    ref: (element) => {
      triggerRef.current = element;
    },
    id: triggerId,
    "aria-haspopup": "menu",
    "aria-expanded": open,
    "aria-controls": open ? menuId : undefined,
    onClick: () => {
      focusOnOpen.current = "first";
      setOpen(!open);
    },
    onKeyDown: onTriggerKeyDown,
  };

  return (
    <>
      {renderTrigger ? (
        renderTrigger(triggerProps, open)
      ) : (
        <IconButton
          {...triggerProps}
          label={label}
          glyph={glyph}
          size={size}
          quiet={quiet}
          reveal={reveal}
          badge={badge}
          className={cx("cc-menu-trigger", triggerClassName)}
          data-testid={triggerTestId}
        />
      )}
      <Popover
        open={open}
        anchorRef={triggerRef}
        onClose={close}
        role="menu"
        ariaLabel={renderTrigger ? label : undefined}
        ariaLabelledBy={renderTrigger ? undefined : triggerId}
        placement={placement}
        minWidth={minWidth}
        id={menuId}
        className={cx("cc-menu", className)}
        testId={testId}
        onKeyDown={onMenuKeyDown}
        initialFocus={(popover) => {
          menuRef.current = popover as HTMLDivElement;
          const list = visibleItems(popover);
          return (focusOnOpen.current === "last" ? list[list.length - 1] : list[0]) ?? undefined;
        }}
      >
        {entries.map((entry, index) => (
          <MenuEntryView key={entry.id ?? `separator-${index}`} entry={entry} onCommand={() => close("select")} />
        ))}
      </Popover>
    </>
  );
}

function MenuLead({ glyph, checked, radio }: { glyph?: KitGlyphName; checked?: boolean; radio?: boolean }) {
  return (
    <span className="cc-menu__lead" aria-hidden="true">
      {radio ? <span className={cx("cc-menu__radio", checked && "is-checked")} /> : checked ? <KitGlyph name="check" /> : glyph ? <KitGlyph name={glyph} /> : null}
    </span>
  );
}

function MenuEntryView({ entry, onCommand }: { entry: MenuEntry; onCommand: () => void }) {
  switch (entry.kind) {
    case "separator":
      return <div role="separator" className={cx("cc-menu__sep", entry.foldOnly && "cc-menu__fold")} />;
    case "item":
      return (
        <button
          type="button"
          role="menuitem"
          tabIndex={-1}
          className={cx("cc-menu__item", entry.destructive && "is-destructive")}
          aria-disabled={entry.disabled || undefined}
          data-text={entry.textValue ?? (typeof entry.label === "string" ? entry.label : undefined)}
          data-testid={entry.testId}
          onClick={() => {
            if (entry.disabled) return;
            entry.onSelect();
            onCommand();
          }}
        >
          <MenuLead glyph={entry.glyph} />
          <span className="cc-menu__text">{entry.label}</span>
          {entry.shortcut ? <kbd className="cc-menu__shortcut">{entry.shortcut}</kbd> : null}
        </button>
      );
    case "checkbox":
      return (
        <button
          type="button"
          role="menuitemcheckbox"
          aria-checked={entry.checked}
          tabIndex={-1}
          className="cc-menu__item"
          aria-disabled={entry.disabled || undefined}
          data-text={entry.textValue ?? (typeof entry.label === "string" ? entry.label : undefined)}
          data-testid={entry.testId}
          onClick={() => {
            if (!entry.disabled) entry.onChange(!entry.checked);
          }}
        >
          <MenuLead checked={entry.checked} />
          <span className="cc-menu__text">{entry.label}</span>
        </button>
      );
    case "radio":
      return (
        <div role="group" aria-label={entry.label} className={cx("cc-menu__group", entry.foldOnly && "cc-menu__fold")}>
          <div className="cc-menu__label" aria-hidden="true">{entry.label}</div>
          {entry.options.map((option) => {
            const checked = option.id === entry.value;
            return (
              <button
                key={option.id}
                type="button"
                role="menuitemradio"
                aria-checked={checked}
                tabIndex={-1}
                className="cc-menu__item"
                aria-disabled={option.disabled || undefined}
                data-text={option.textValue ?? (typeof option.label === "string" ? option.label : undefined)}
                data-testid={option.testId}
                onClick={() => {
                  if (!option.disabled && !checked) entry.onChange(option.id);
                }}
              >
                <MenuLead radio checked={checked} />
                <span className="cc-menu__text">{option.label}</span>
              </button>
            );
          })}
        </div>
      );
    default:
      return null;
  }
}
