import { useRef, type ComponentPropsWithRef, type KeyboardEvent, type ReactNode } from "react";

import { Count } from "./chip";
import { cx } from "./cx";
import { RelativeTime } from "./relative-time";
import type { TimeInput } from "./format";

/**
 * The frame of every panel: header, optional tabs, the reserved status row
 * and a scrolling body. An inline-size container, so the parts adapt to the
 * pane's width rather than the window's. `className` keeps panel-specific
 * classes that tests pin.
 */
export function PanelShell({
  className,
  testId,
  ariaLabel,
  children,
}: {
  className?: string;
  testId?: string;
  ariaLabel?: string;
  children?: ReactNode;
}) {
  return (
    <section className={cx("cc-panel", className)} data-testid={testId} aria-label={ariaLabel}>
      {children}
    </section>
  );
}

/**
 * The ids that tie a panel tab to its panel: give PanelTabs (`idBase`) and
 * the panel (PanelBody's `tabPanel`, or a PanelTabPanel) the same base,
 * such as one from React's useId().
 */
export function panelTabIds(idBase: string, tab: string): { tabId: string; panelId: string } {
  const key = tab.replace(/\s+/g, "-");
  return { tabId: `${idBase}-tab-${key}`, panelId: `${idBase}-panel-${key}` };
}

/** The panel of the active tab, for content that sits inside the panel body
 * (PanelBody's `tabPanel` makes the body itself the panel). */
export function PanelTabPanel({
  idBase,
  tab,
  ...rest
}: ComponentPropsWithRef<"div"> & { idBase: string; tab: string }) {
  const ids = panelTabIds(idBase, tab);
  return <div {...rest} role="tabpanel" id={ids.panelId} aria-labelledby={ids.tabId} />;
}

/**
 * The scrolling body below the header, tabs and status row. Under tabs,
 * `tabPanel` makes it the active tab's panel (role="tabpanel", labelled by
 * the tab that controls it).
 */
export function PanelBody({
  className,
  testId,
  tabPanel,
  children,
}: {
  className?: string;
  testId?: string;
  /** The PanelTabs `idBase` and the active tab. */
  tabPanel?: { idBase: string; tab: string };
  children?: ReactNode;
}) {
  const ids = tabPanel ? panelTabIds(tabPanel.idBase, tabPanel.tab) : null;
  return (
    <div
      className={cx("cc-panel__body", className)}
      data-testid={testId}
      role={ids ? "tabpanel" : undefined}
      id={ids?.panelId}
      aria-labelledby={ids?.tabId}
    >
      {children}
    </div>
  );
}

/**
 * Title (one heading per panel), a subtitle in sentence words with tabular
 * numbers, an optional leading mark and the actions. Actions wrap below the
 * title in a narrow pane. `divider={false}` when tabs follow (the tab row
 * draws the rule).
 */
export function PanelHeader({
  title,
  subtitle,
  lead,
  actions,
  divider = true,
  headingLevel = 2,
  className,
  testId,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  lead?: ReactNode;
  actions?: ReactNode;
  divider?: boolean;
  headingLevel?: 2 | 3;
  className?: string;
  testId?: string;
}) {
  const Heading = headingLevel === 3 ? "h3" : "h2";
  return (
    <header className={cx("cc-panel-head", !divider && "cc-panel-head--flush", className)} data-testid={testId}>
      {lead ? <div className="cc-panel-head__lead">{lead}</div> : null}
      <div className="cc-panel-head__text">
        <Heading className="cc-panel-head__title">{title}</Heading>
        {subtitle ? <div className="cc-panel-head__sub">{subtitle}</div> : null}
      </div>
      {actions ? <div className="cc-panel-head__actions">{actions}</div> : null}
    </header>
  );
}

export interface PanelTab<Id extends string = string> {
  id: Id;
  label: ReactNode;
  count?: number | string | null;
  testId?: string;
  className?: string;
  /** Accessible name when the label is not plain text. */
  ariaLabel?: string;
}

/**
 * Panel tabs: a tablist of tab buttons with aria-selected and the is-active
 * class, the accent underline on the active tab and an optional count.
 * Arrow keys, Home and End move focus; Enter or Space selects. Tabs never
 * hold actions (refresh lives in the header). With `idBase`, each tab
 * controls its panel (aria-controls): render the active tab's content in a
 * PanelBody with `tabPanel`, or in a PanelTabPanel, with the same base.
 */
export function PanelTabs<Id extends string>({
  tabs,
  active,
  onSelect,
  ariaLabel,
  idBase,
  className,
  tabClassName,
  testId,
}: {
  tabs: ReadonlyArray<PanelTab<Id>>;
  active: Id;
  onSelect: (id: NoInfer<Id>) => void;
  ariaLabel?: string;
  /** Ties each tab to its panel; see panelTabIds. */
  idBase?: string;
  className?: string;
  tabClassName?: string;
  testId?: string;
}) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const keys = ["ArrowRight", "ArrowLeft", "Home", "End"];
    if (!keys.includes(event.key)) return;
    const buttons = Array.from(listRef.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]') ?? []);
    const current = buttons.findIndex((button) => button === event.target);
    if (current < 0 || !buttons.length) return;
    event.preventDefault();
    const next =
      event.key === "Home" ? 0
        : event.key === "End" ? buttons.length - 1
          : event.key === "ArrowRight" ? (current + 1) % buttons.length
            : (current - 1 + buttons.length) % buttons.length;
    buttons[next]?.focus();
  };
  return (
    <div
      ref={listRef}
      role="tablist"
      aria-label={ariaLabel}
      className={cx("cc-tabs", className)}
      onKeyDown={onKeyDown}
      data-testid={testId}
    >
      {tabs.map((tab) => {
        const selected = tab.id === active;
        const ids = idBase ? panelTabIds(idBase, tab.id) : null;
        return (
          <button
            key={tab.id}
            type="button"
            role="tab"
            id={ids?.tabId}
            aria-controls={ids?.panelId}
            aria-selected={selected}
            aria-label={tab.ariaLabel}
            tabIndex={selected ? 0 : -1}
            className={cx("cc-tab", selected && "is-active", tabClassName, tab.className)}
            data-testid={tab.testId}
            onClick={() => onSelect(tab.id)}
          >
            {tab.label}
            {tab.count !== null && tab.count !== undefined && tab.count !== "" ? (
              <>
                {" "}
                <Count value={tab.count} />
              </>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}

/**
 * The panel's reserved status line, always one line high so a failed refresh
 * or its recovery never moves the controls around it. It shows the error
 * (role="alert", the only alert in the panel) or, when healthy, how fresh
 * the data is.
 */
export function StatusRow({
  error,
  errorTestId,
  updatedAt,
  updatedTitle,
  children,
  className,
}: {
  error?: string | null;
  errorTestId?: string;
  updatedAt?: TimeInput;
  /** Tooltip for the freshness line; the absolute time otherwise. */
  updatedTitle?: string;
  /** Shown instead of the freshness line when healthy. */
  children?: ReactNode;
  className?: string;
}) {
  return (
    <div className={cx("cc-status-row", className)}>
      {error ? (
        <span className="cc-status-row__error" role="alert" title={error} data-testid={errorTestId}>
          {error}
        </span>
      ) : children ? (
        <span className="cc-status-row__text">{children}</span>
      ) : updatedAt !== null && updatedAt !== undefined ? (
        <RelativeTime value={updatedAt} prefix="Updated" title={updatedTitle} className="cc-status-row__text" />
      ) : null}
    </div>
  );
}

/**
 * The single section label: uppercase micro-label with an optional count
 * and a right-aligned action. An h3 by default; pass `as="div"` where a test
 * expects a single heading in the region.
 */
export function SectionHeader({
  label,
  count,
  action,
  as = "h3",
  id,
  className,
  testId,
}: {
  label: ReactNode;
  count?: number | string | null;
  action?: ReactNode;
  as?: "h3" | "h4" | "div";
  id?: string;
  className?: string;
  testId?: string;
}) {
  const Element = as;
  return (
    <div className={cx("cc-section-head", className)} data-testid={testId}>
      <Element className="cc-section-head__label" id={id}>
        {label}
        {count !== null && count !== undefined && count !== "" ? (
          <>
            {" "}
            <Count value={count} />
          </>
        ) : null}
      </Element>
      {action ? <div className="cc-section-head__action">{action}</div> : null}
    </div>
  );
}

/** A section of a panel body: a SectionHeader and its content, separated
 * from the previous section by space and a rule, never boxed. */
export function PanelSection({
  label,
  count,
  action,
  headerAs,
  padded,
  className,
  testId,
  children,
}: {
  label?: ReactNode;
  count?: number | string | null;
  action?: ReactNode;
  headerAs?: "h3" | "h4" | "div";
  /** Inset the content by the panel padding (cards, tiles, prose). */
  padded?: boolean;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  return (
    <section className={cx("cc-section", className)} data-testid={testId}>
      {label ? <SectionHeader label={label} count={count} action={action} as={headerAs} /> : null}
      <div className={cx("cc-section__content", padded && "cc-section__content--padded")}>{children}</div>
    </section>
  );
}
