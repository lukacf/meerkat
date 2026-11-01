import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ComponentPropsWithRef,
  type KeyboardEvent,
  type ReactNode,
} from "react";

import { Button } from "./button";
import { Count } from "./chip";
import { cx } from "./cx";
import { OverflowMenu, type MenuEntry } from "./menu";
import { RelativeTime } from "./relative-time";
import { StatusMark } from "./status-mark";
import { SegmentedControl, type SegmentOption } from "./toolbar";
import type { TimeInput } from "./format";
import type { Tone } from "./tone";

export type PanelDensity = "comfortable" | "compact";

/** What a panel's view options set for every part inside it. */
export interface PanelOptions {
  density: PanelDensity;
  /** "Show technical IDs" (A3.6): ids render inline instead of inside the
   * Technical details disclosure. */
  showTechnicalIds: boolean;
}

const PanelOptionsContext = createContext<PanelOptions>({ density: "comfortable", showTechnicalIds: false });

/** The density and technical-id option of the nearest PanelShell. */
export function usePanelOptions(): PanelOptions {
  return useContext(PanelOptionsContext);
}

/**
 * The frame of every panel: header, optional tabs, the reserved status row
 * and a scrolling body. An inline-size container, so the parts adapt to the
 * pane's width rather than the window's. `className` keeps panel-specific
 * classes that tests pin. `density` (comfortable or compact, A4.3) and
 * `showTechnicalIds` (A3.6) apply to every kit part inside.
 */
export function PanelShell({
  className,
  testId,
  ariaLabel,
  density,
  showTechnicalIds,
  children,
}: {
  className?: string;
  testId?: string;
  ariaLabel?: string;
  density?: PanelDensity;
  showTechnicalIds?: boolean;
  children?: ReactNode;
}) {
  const section = (
    <section
      className={cx("cc-panel", className)}
      data-testid={testId}
      aria-label={ariaLabel}
      data-density={density === "compact" ? "compact" : undefined}
    >
      {children}
    </section>
  );
  if (density === undefined && showTechnicalIds === undefined) return section;
  return (
    <PanelOptionsContext.Provider value={{ density: density ?? "comfortable", showTechnicalIds: Boolean(showTechnicalIds) }}>
      {section}
    </PanelOptionsContext.Provider>
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

/** One fact of a panel's live summary line (A3.1). */
export interface SummaryFact {
  key?: string;
  /** What the line shows ("7 agents", "2 working now"). */
  text: ReactNode;
  /** The words announced and shown in the tooltip when `text` is not plain
   * text. */
  words?: string;
  /** A toned fact starts with its StatusMark and takes the tone's ink. */
  tone?: Tone;
  title?: string;
  /** Count changes of this fact are announced (politely, at most every 30
   * seconds). Default true. */
  announce?: boolean;
  testId?: string;
}

/** The panel's one view switch (A3.1): a segmented control that folds into
 * the overflow menu as a radio group in a pane under 480 px. */
export interface PanelViewSwitch<Id extends string = string> {
  ariaLabel: string;
  options: ReadonlyArray<SegmentOption<Id>>;
  value: Id;
  onChange: (id: Id) => void;
  testId?: string;
}

export interface PanelHeaderMenu {
  /** The trigger's accessible name: "More actions for Topology". */
  label: string;
  items: ReadonlyArray<MenuEntry | null | false | undefined>;
  testId?: string;
  triggerTestId?: string;
}

export interface PanelFreshness {
  /** When the shown data was read: "Updated 2m ago". */
  at?: TimeInput;
  /** A refresh is running: the fact reads "Updating". */
  updating?: boolean;
  /** Tooltip; the absolute time otherwise. */
  title?: string;
  testId?: string;
}

const ANNOUNCE_INTERVAL_MS = 30_000;

function factWords(fact: SummaryFact): string {
  if (fact.words) return fact.words;
  return typeof fact.text === "string" || typeof fact.text === "number" ? String(fact.text) : "";
}

/** The summary's polite announcement: count changes only, at most every
 * 30 seconds, never the first render. */
function useThrottledAnnouncement(text: string): string {
  const [announced, setAnnounced] = useState("");
  const first = useRef(text);
  const lastAt = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    if (text === first.current && !lastAt.current) return undefined;
    first.current = "";
    const wait = lastAt.current + ANNOUNCE_INTERVAL_MS - Date.now();
    if (timer.current) clearTimeout(timer.current);
    if (wait <= 0) {
      lastAt.current = Date.now();
      setAnnounced(text);
      return undefined;
    }
    timer.current = setTimeout(() => {
      lastAt.current = Date.now();
      setAnnounced(text);
    }, wait);
    return () => {
      if (timer.current) clearTimeout(timer.current);
    };
  }, [text]);
  return announced;
}

function PanelSummary({
  facts,
  freshness,
  testId,
}: {
  facts: ReadonlyArray<SummaryFact>;
  freshness?: PanelFreshness;
  testId?: string;
}) {
  const announced = useThrottledAnnouncement(
    facts
      .filter((fact) => fact.announce !== false)
      .map(factWords)
      .filter(Boolean)
      .join(", "),
  );
  const parts: ReactNode[] = facts.map((fact, index) => (
    <span
      key={fact.key ?? index}
      className="cc-panel-head__fact"
      data-tone={fact.tone}
      title={fact.title}
      data-testid={fact.testId}
    >
      {fact.tone ? <StatusMark tone={fact.tone} /> : null}
      {fact.text}
    </span>
  ));
  if (freshness && (freshness.updating || freshness.at !== undefined)) {
    parts.push(
      freshness.updating ? (
        <span key="freshness" className="cc-panel-head__fact cc-panel-head__fresh" data-testid={freshness.testId}>
          Updating
        </span>
      ) : (
        <RelativeTime
          key="freshness"
          value={freshness.at}
          prefix="Updated"
          title={freshness.title}
          className="cc-panel-head__fact cc-panel-head__fresh"
          testId={freshness.testId}
        />
      ),
    );
  }
  return (
    <>
      <div className="cc-panel-head__sub cc-panel-head__summary" data-testid={testId}>
        {parts.map((part, index) => (
          <span className="cc-panel-head__part" key={index}>
            {index > 0 ? (
              <span className="cc-panel-head__sep" aria-hidden="true">
                {" · "}
              </span>
            ) : null}
            {part}
          </span>
        ))}
      </div>
      <span className="cc-visually-hidden" aria-live="polite">
        {announced}
      </span>
    </>
  );
}

/**
 * Title (one heading per panel), a subtitle in sentence words with tabular
 * numbers, an optional leading mark and the actions. Actions wrap below the
 * title in a narrow pane. `divider={false}` when tabs follow (the tab row
 * draws the rule).
 *
 * The v2 header (A3.1) adds: `scope` (the quiet team chip by the title),
 * `summary` (the live facts line, ending with `freshness`), `refreshing`
 * (the 1 px accent hairline), and on the right, in this order, `view` (the
 * one view switch), `tools` (icon buttons, hover-revealed) and `menu` (the
 * overflow trigger, always present). `actions` keeps the older always-shown
 * slot. Every older prop renders exactly as before.
 */
export function PanelHeader({
  title,
  subtitle,
  lead,
  actions,
  scope,
  summary,
  summaryTestId,
  freshness,
  refreshing,
  view,
  tools,
  menu,
  divider = true,
  headingLevel = 2,
  className,
  testId,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  lead?: ReactNode;
  actions?: ReactNode;
  /** The scope chip next to the title (a SelectField variant="chip"). */
  scope?: ReactNode;
  summary?: ReadonlyArray<SummaryFact | null | false | undefined>;
  summaryTestId?: string;
  freshness?: PanelFreshness;
  refreshing?: boolean;
  // Each panel types its own view ids at its call site.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  view?: PanelViewSwitch<any>;
  tools?: ReactNode;
  menu?: PanelHeaderMenu;
  divider?: boolean;
  headingLevel?: 2 | 3;
  className?: string;
  testId?: string;
}) {
  const Heading = headingLevel === 3 ? "h3" : "h2";
  const facts = (summary ?? []).filter((fact): fact is SummaryFact => Boolean(fact));
  const hasSummary = facts.length > 0 || Boolean(freshness && (freshness.updating || freshness.at !== undefined));
  const v2 = Boolean(scope || summary || freshness || view || tools || menu || refreshing !== undefined);
  const heading = <Heading className="cc-panel-head__title">{title}</Heading>;
  const menuItems: Array<MenuEntry | null | false | undefined> = [];
  if (view) {
    menuItems.push({
      kind: "radio",
      id: "panel-view",
      label: view.ariaLabel,
      value: view.value,
      foldOnly: true,
      options: view.options.map((option) => ({
        id: option.id,
        label: option.label,
        disabled: option.disabled,
        testId: option.testId ? `${option.testId}:menu` : undefined,
      })),
      onChange: (id) => view.onChange(id),
    });
    if (menu?.items.some(Boolean)) menuItems.push({ kind: "separator", id: "panel-view-sep", foldOnly: true });
  }
  if (menu) menuItems.push(...menu.items);
  const showMenu = Boolean(menu || view);
  const hasRight = Boolean(actions || view || tools || showMenu);
  return (
    <header
      className={cx(
        "cc-panel-head",
        !divider && "cc-panel-head--flush",
        v2 && "cc-panel-head--v2 cc-reveal-host",
        refreshing && "is-refreshing",
        className,
      )}
      data-testid={testId}
    >
      {lead ? <div className="cc-panel-head__lead">{lead}</div> : null}
      <div className="cc-panel-head__text">
        {scope ? (
          <div className="cc-panel-head__title-row">
            {heading}
            <span className="cc-panel-head__scope">{scope}</span>
          </div>
        ) : (
          heading
        )}
        {subtitle ? <div className="cc-panel-head__sub">{subtitle}</div> : null}
        {hasSummary ? <PanelSummary facts={facts} freshness={freshness} testId={summaryTestId} /> : null}
      </div>
      {hasRight ? (
        <div className="cc-panel-head__actions">
          {view ? (
            <SegmentedControl
              options={view.options}
              value={view.value}
              onChange={view.onChange}
              ariaLabel={view.ariaLabel}
              testId={view.testId}
              className="cc-panel-head__view"
            />
          ) : null}
          {actions}
          {tools ? <span className="cc-panel-head__tools cc-reveal">{tools}</span> : null}
          {showMenu ? (
            <OverflowMenu
              label={menu?.label ?? `${view?.ariaLabel ?? "View"} options`}
              items={menuItems}
              quiet
              testId={menu?.testId}
              triggerTestId={menu?.triggerTestId}
              triggerClassName={cx("cc-panel-head__menu", !menu && "cc-panel-head__menu--fold")}
            />
          ) : null}
        </div>
      ) : null}
      {refreshing ? <span className="cc-panel-head__progress" aria-hidden="true" /> : null}
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
 * The status toast (A3.1): a refresh error shown over the top of the body
 * without reserving space, so nothing moves when it appears or goes. The
 * crit mark, the error in a role="alert" span (the panel's one alert), how
 * old the shown data is, and Retry.
 */
export function StatusToast({
  error,
  errorTestId,
  updatedAt,
  updatedTitle,
  onRetry,
  retryLabel = "Retry",
  className,
  testId,
}: {
  error?: string | null;
  errorTestId?: string;
  /** When the data still shown was read: "Showing data from 3m ago." */
  updatedAt?: TimeInput;
  updatedTitle?: string;
  onRetry?: () => void;
  retryLabel?: string;
  className?: string;
  testId?: string;
}) {
  if (!error) return null;
  return (
    <div className={cx("cc-status-toast-anchor", className)} data-testid={testId}>
      <div className="cc-status-toast">
        <StatusMark tone="crit" className="cc-status-toast__mark" />
        <span className="cc-status-toast__text">
          <span className="cc-status-row__error cc-status-toast__error" role="alert" title={error} data-testid={errorTestId}>
            {error}
          </span>
          {updatedAt !== null && updatedAt !== undefined ? (
            <span className="cc-status-toast__stale">
              {" Showing data from "}
              <RelativeTime value={updatedAt} title={updatedTitle} />.
            </span>
          ) : null}
        </span>
        {onRetry ? (
          <Button size="sm" onClick={onRetry} className="cc-status-toast__retry">
            {retryLabel}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/**
 * The panel's reserved status line, always one line high so a failed refresh
 * or its recovery never moves the controls around it. It shows the error
 * (role="alert", the only alert in the panel) or, when healthy, how fresh
 * the data is.
 *
 * `mode="toast"` (for panels whose header shows freshness, A3.1) reserves
 * nothing and shows an error as the StatusToast over the top of the body,
 * with `updatedAt` as the age of the data still shown and `onRetry`.
 */
export function StatusRow({
  error,
  errorTestId,
  updatedAt,
  updatedTitle,
  children,
  className,
  mode = "row",
  onRetry,
  retryLabel,
}: {
  error?: string | null;
  errorTestId?: string;
  updatedAt?: TimeInput;
  /** Tooltip for the freshness line; the absolute time otherwise. */
  updatedTitle?: string;
  /** Shown instead of the freshness line when healthy. */
  children?: ReactNode;
  className?: string;
  mode?: "row" | "toast";
  onRetry?: () => void;
  retryLabel?: string;
}) {
  if (mode === "toast") {
    return (
      <StatusToast
        error={error}
        errorTestId={errorTestId}
        updatedAt={updatedAt}
        updatedTitle={updatedTitle}
        onRetry={onRetry}
        retryLabel={retryLabel}
        className={className}
      />
    );
  }
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
