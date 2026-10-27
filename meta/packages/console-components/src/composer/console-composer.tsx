import clsx from "clsx";
import {
  type ButtonHTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type Ref,
} from "react";

import type {
  ConsoleComposerToolbarItem,
  ConsoleComposerToolbarItemKind,
  ConsoleComposerViewState,
} from "@console-core";

import type { IconRenderer } from "../shared";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type ConsoleComposerToolbarButtonScope = {
  zone: "main" | "footer-left" | "footer-right";
  item: ConsoleComposerToolbarItem;
};

type ToolbarButtonExtraProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  ref?: Ref<HTMLButtonElement>;
};

export type ConsoleComposerProps = {
  viewState: ConsoleComposerViewState;
  Icon?: IconRenderer | null;
  className?: string;
  shellClassName?: string;
  footerClassName?: string;
  inputId?: string;
  inputDescriptionId?: string;
  inputRef?: Ref<HTMLTextAreaElement>;
  shellId?: string;
  submitButtonId?: string;

  /**
   * Per-item customisation callback (refs, ids, click overrides).
   * Follows the ConsoleSidebar `getActionButtonProps` pattern.
   */
  getToolbarButtonProps?: (
    scope: ConsoleComposerToolbarButtonScope,
  ) => ToolbarButtonExtraProps;

  /** Replace the entire main-row zone. Receives the visible items and default markup. */
  renderMainRow?: (args: {
    items: ConsoleComposerToolbarItem[];
    defaultMainRow: ReactNode;
  }) => ReactNode;

  /** Replace the entire footer zone. Receives the visible items and default markup. */
  renderFooter?: (args: {
    leftItems: ConsoleComposerToolbarItem[];
    rightItems: ConsoleComposerToolbarItem[];
    defaultFooter: ReactNode;
  }) => ReactNode;

  onChange: (value: string) => void;
  onFocus?: () => void;
  onKeyDown?: (event: ReactKeyboardEvent<HTMLTextAreaElement>) => void;
  onSubmit: () => void;
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function kindToClassName(kind: ConsoleComposerToolbarItemKind, zone: string): string {
  switch (kind) {
    case "pill":
      return "cc-composer__pill";
    case "pill-icon":
      return "cc-composer__pill-icon";
    case "sub-pill":
      return "cc-composer__sub-pill";
    case "icon":
      return zone === "main" ? "cc-composer__icon-ghost" : "cc-composer__footer-icon";
    default:
      return "cc-composer__pill";
  }
}

function visible(items: ConsoleComposerToolbarItem[]): ConsoleComposerToolbarItem[] {
  return items.filter((item) => !item.hidden);
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function ConsoleComposer({
  viewState,
  Icon,
  className,
  shellClassName,
  footerClassName,
  inputId,
  inputDescriptionId,
  inputRef,
  shellId,
  submitButtonId,
  getToolbarButtonProps,
  renderMainRow,
  renderFooter,
  onChange,
  onFocus,
  onKeyDown,
  onSubmit,
}: ConsoleComposerProps) {
  const {
    value,
    disabled = false,
    placeholder,
    submitDisabled = false,
    submitLabel = "Send prompt",
    mainRowItems,
    footerLeftItems,
    footerRightItems,
  } = viewState;

  const visibleMain = visible(mainRowItems);
  const visibleFooterLeft = visible(footerLeftItems);
  const visibleFooterRight = visible(footerRightItems);
  const hasFooter = visibleFooterLeft.length > 0 || visibleFooterRight.length > 0;

  // ------- Render a single toolbar button -------
  function renderToolbarButton(
    item: ConsoleComposerToolbarItem,
    zone: "main" | "footer-left" | "footer-right",
  ) {
    const baseClass = kindToClassName(item.kind, zone);
    const extra = getToolbarButtonProps?.({ zone, item });
    const { ref, className: extraClass, ...restExtra } = extra ?? {};

    return (
      <button
        key={item.id}
        className={clsx(baseClass, item.hasMenu && "has-menu", extraClass)}
        disabled={item.disabled}
        type="button"
        ref={ref}
        {...restExtra}
      >
        {item.iconName && Icon ? <><Icon name={item.iconName} />{" "}</> : null}
        {item.label ?? null}
        {item.hasMenu && Icon ? <>{" "}<Icon className="chev" name="i-chevron" /></> : null}
      </button>
    );
  }

  // ------- Main row (inside the shell) -------
  const defaultMainRow = (
    <div className="cc-composer__main-row">
      {visibleMain.map((item) => renderToolbarButton(item, "main"))}
      <button
        className="cc-composer__send-btn"
        disabled={submitDisabled}
        id={submitButtonId}
        title={submitLabel}
        aria-label={submitLabel}
        type="button"
        onClick={onSubmit}
      >
        ↑
      </button>
    </div>
  );

  const mainRow = renderMainRow
    ? renderMainRow({ items: visibleMain, defaultMainRow })
    : defaultMainRow;

  // ------- Footer -------
  const defaultFooter = hasFooter ? (
    <div className={clsx("cc-composer__footer", footerClassName)}>
      <div className="cc-composer__footer-left">
        {visibleFooterLeft.map((item) => renderToolbarButton(item, "footer-left"))}
      </div>
      <div className="cc-composer__footer-right">
        {visibleFooterRight.map((item) => renderToolbarButton(item, "footer-right"))}
      </div>
    </div>
  ) : null;

  const footer = renderFooter
    ? renderFooter({
        leftItems: visibleFooterLeft,
        rightItems: visibleFooterRight,
        defaultFooter,
      })
    : defaultFooter;

  // ------- Root -------
  return (
    <section className={clsx("cc-composer", className)}>
      <div className={clsx("cc-composer__shell", shellClassName)} id={shellId}>
        <textarea
          ref={inputRef}
          className="cc-composer__textarea"
          disabled={disabled}
          id={inputId}
          aria-describedby={inputDescriptionId}
          placeholder={placeholder}
          value={value}
          onChange={(event) => onChange(event.currentTarget.value)}
          onFocus={onFocus}
          onKeyDown={onKeyDown}
        />
        {mainRow}
      </div>
      {footer}
    </section>
  );
}
