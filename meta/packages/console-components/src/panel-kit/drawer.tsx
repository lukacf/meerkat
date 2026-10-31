import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
} from "react";

import { IconButton } from "./button";
import { Count } from "./chip";
import { cx } from "./cx";
import { usePanelPreference } from "./preferences";
import { TechnicalDetails, type TechnicalDetail } from "./technical-details";

/**
 * The layout of a list or canvas with its detail drawer (spec A3.10,
 * SplitDetail v2), by its own width: from 900 px the drawer is a docked
 * right column (340 px by default, resizable 300-440 with a 6 px handle,
 * persisted per panel) that pushes the main view; from 600 to 900 px it
 * overlays the right of the main view (e3); under 600 px it replaces the
 * main view under a Back row, and the main view keeps its scroll.
 */
export function DrawerLayout({
  main,
  drawer,
  open,
  onClose,
  persistKey,
  defaultWidth = 340,
  minWidth = 300,
  maxWidth = 440,
  backLabel = "Back",
  backTestId,
  className,
  testId,
}: {
  main: ReactNode;
  /** Usually a DetailDrawer. */
  drawer?: ReactNode;
  open: boolean;
  /** The Back row (narrow) and the drawer's own close. */
  onClose?: () => void;
  /** Keeps the drawer width per panel: `meta-console-drawer:<key>`. */
  persistKey?: string;
  defaultWidth?: number;
  minWidth?: number;
  maxWidth?: number;
  /** The Back row's words under 600 px ("All agents"). */
  backLabel?: string;
  backTestId?: string;
  className?: string;
  testId?: string;
}) {
  const [stored, setStored] = usePanelPreference<string>(`meta-console-drawer:${persistKey ?? "default"}`, String(defaultWidth));
  const persisted = Number(stored);
  const clamp = (value: number) => Math.min(maxWidth, Math.max(minWidth, Math.round(value)));
  const [width, setWidth] = useState(() => clamp(Number.isFinite(persisted) ? persisted : defaultWidth));
  useEffect(() => {
    if (Number.isFinite(persisted)) setWidth(clamp(persisted));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [persisted]);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const commit = (next: number) => {
    const value = clamp(next);
    setWidth(value);
    if (persistKey) setStored(String(value));
  };
  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    drag.current = { x: event.clientX, width };
    event.currentTarget.setPointerCapture?.(event.pointerId);
    event.preventDefault();
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    setWidth(clamp(drag.current.width + drag.current.x - event.clientX));
  };
  const onPointerUp = (event: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    const start = drag.current;
    drag.current = null;
    event.currentTarget.releasePointerCapture?.(event.pointerId);
    commit(start.width + start.x - event.clientX);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const steps: Record<string, number> = { ArrowLeft: 16, ArrowRight: -16 };
    if (event.key in steps) {
      event.preventDefault();
      commit(width + steps[event.key]);
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      commit(event.key === "Home" ? minWidth : maxWidth);
    }
  };
  const style = { "--_cc-drawer-w": `${width}px` } as CSSProperties;
  const showDrawer = open && drawer;
  return (
    <div className={cx("cc-drawer-layout", showDrawer && "has-drawer", className)} data-testid={testId} style={style}>
      <div className="cc-drawer-layout__inner">
        <div className="cc-drawer-layout__main">{main}</div>
        {showDrawer ? (
          <>
            <div
              className="cc-drawer-layout__handle"
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize details"
              aria-valuemin={minWidth}
              aria-valuemax={maxWidth}
              aria-valuenow={width}
              tabIndex={0}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={onPointerUp}
              onKeyDown={onKeyDown}
            />
            <div className="cc-drawer-layout__drawer">
              {onClose ? (
                <button type="button" className="cc-drawer-layout__back" onClick={onClose} data-testid={backTestId}>
                  <span className="cc-split__back-mark" aria-hidden="true" />
                  {backLabel}
                </button>
              ) : null}
              {drawer}
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}

/**
 * The detail drawer itself (spec A3.10): a header with the entity mark, the
 * entity-size title (the drawer's one heading), the status pill, "Open in
 * pane" and Close; then the body (micro-label sections, as children), the
 * actions and Technical details. Opening moves focus to the heading;
 * Escape or Close calls `onClose` and returns focus to the element that had
 * it when the drawer opened (the node or row), or calls `returnFocus`.
 */
export function DetailDrawer({
  title,
  mark,
  status,
  subtitle,
  onClose,
  closeLabel = "Close details",
  closeTestId,
  onOpenInPane,
  openInPaneLabel = "Open in pane",
  actions,
  technical,
  headingLevel = 3,
  titleClassName,
  returnFocus,
  autoFocus = true,
  ariaLabel,
  className,
  testId,
  children,
}: {
  title: ReactNode;
  mark?: ReactNode;
  status?: ReactNode;
  subtitle?: ReactNode;
  onClose?: () => void;
  closeLabel?: string;
  closeTestId?: string;
  onOpenInPane?: () => void;
  openInPaneLabel?: string;
  /** The action row after the sections (Open chat, Details, Connect to...). */
  actions?: ReactNode;
  /** Technical details at the end (ids, keys, sessions). */
  technical?: ReadonlyArray<TechnicalDetail | null | false | undefined>;
  headingLevel?: 2 | 3 | 4;
  /** Keeps a pinned class on the heading. */
  titleClassName?: string;
  /** Where focus goes on close; the element focused at open otherwise. */
  returnFocus?: () => void;
  /** Move focus to the heading when the drawer opens (default). */
  autoFocus?: boolean;
  ariaLabel?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  const rootRef = useRef<HTMLElement | null>(null);
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const opener = useRef<HTMLElement | null>(null);
  useLayoutEffect(() => {
    const doc = rootRef.current?.ownerDocument;
    const active = doc?.activeElement as HTMLElement | null;
    opener.current = active && active !== doc?.body && !rootRef.current?.contains(active) ? active : null;
    if (autoFocus) headingRef.current?.focus({ preventScroll: true });
    // Once per opening.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const close = () => {
    onClose?.();
    if (returnFocus) returnFocus();
    else if (opener.current?.isConnected) opener.current.focus({ preventScroll: true });
  };
  const Heading = headingLevel === 2 ? "h2" : headingLevel === 4 ? "h4" : "h3";
  return (
    <aside
      ref={rootRef}
      className={cx("cc-drawer", className)}
      aria-label={ariaLabel ?? (typeof title === "string" ? title : undefined)}
      data-testid={testId}
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !onClose) return;
        event.preventDefault();
        event.stopPropagation();
        close();
      }}
    >
      <header className="cc-drawer__head">
        {mark ? <span className="cc-drawer__mark">{mark}</span> : null}
        <div className="cc-drawer__titles">
          <Heading ref={headingRef} className={cx("cc-drawer__title", titleClassName)} tabIndex={-1}>
            {title}
          </Heading>
          {subtitle ? <div className="cc-drawer__sub">{subtitle}</div> : null}
        </div>
        {status ? <span className="cc-drawer__status">{status}</span> : null}
        <span className="cc-drawer__tools">
          {onOpenInPane ? <IconButton glyph="open" label={openInPaneLabel} size="sm" onClick={onOpenInPane} /> : null}
          {onClose ? <IconButton glyph="close" label={closeLabel} size="sm" onClick={close} data-testid={closeTestId} /> : null}
        </span>
      </header>
      <div className="cc-drawer__body">
        {children}
        {actions ? <div className="cc-drawer__actions">{actions}</div> : null}
        {technical ? <TechnicalDetails items={technical} className="cc-drawer__tech" /> : null}
      </div>
    </aside>
  );
}

/** A section of a drawer body: a micro-label (a div, never a heading: the
 * drawer has one heading) with an optional count and action, then its
 * content. */
export function DrawerSection({
  label,
  count,
  action,
  className,
  testId,
  children,
}: {
  label: ReactNode;
  count?: number | string | null;
  /** Right-aligned words or a control ("46 turns", "Show all"). */
  action?: ReactNode;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  return (
    <section className={cx("cc-drawer__section", className)} data-testid={testId}>
      <div className="cc-drawer__section-head">
        <span className="cc-drawer__section-label">
          {label}
          {count !== null && count !== undefined && count !== "" ? (
            <>
              {" "}
              <Count value={count} />
            </>
          ) : null}
        </span>
        {action ? <span className="cc-drawer__section-action">{action}</span> : null}
      </div>
      {children}
    </section>
  );
}
