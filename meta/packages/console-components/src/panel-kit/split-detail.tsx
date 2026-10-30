import { useLayoutEffect, useRef, type ReactNode } from "react";

import { cx } from "./cx";

const FOCUSABLE = "button, a[href], input, select, textarea, summary, [tabindex]";

/** The element to focus for a row: the row itself, or its first control. */
function focusTarget(element: Element | null): HTMLElement | null {
  if (!element) return null;
  if (element.matches(FOCUSABLE)) return element as HTMLElement;
  return element.querySelector<HTMLElement>(FOCUSABLE);
}

/**
 * A list with a detail column. In a pane at least 720 px wide the two sit
 * side by side and the detail column always shows `detail` (or
 * `emptyDetail`). Narrower, the list shows alone until `detailOpen`, then
 * the detail replaces it (the list keeps its scroll position) under a Back
 * row.
 *
 * Focus follows: when the detail replaces the list, focus moves to the
 * detail's first heading (or the detail itself). When the detail closes
 * with focus inside it (Back), focus returns to the selected row, the one
 * the detail showed, else to the row that opened it. Side by side, opening
 * leaves focus where it is.
 */
export function SplitDetail({
  list,
  detail,
  detailOpen,
  onBack,
  backLabel = "Back",
  emptyDetail,
  className,
  testId,
}: {
  list: ReactNode;
  detail?: ReactNode;
  /** In a narrow pane: show the detail instead of the list. */
  detailOpen: boolean;
  onBack?: () => void;
  /** "All agents", "All routes". */
  backLabel?: string;
  /** Shown in the wide detail column when there is no detail. */
  emptyDetail?: ReactNode;
  className?: string;
  testId?: string;
}) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const detailRef = useRef<HTMLDivElement | null>(null);
  const wasOpen = useRef(detailOpen);
  // The row focused when the detail opened, and its test ID to find it
  // again if the list re-rendered it.
  const opener = useRef<{ element: HTMLElement; testId: string | null } | null>(null);

  useLayoutEffect(() => {
    const changed = wasOpen.current !== detailOpen;
    wasOpen.current = detailOpen;
    const listEl = listRef.current;
    const detailEl = detailRef.current;
    if (!changed || !listEl || !detailEl) return;
    const doc = listEl.ownerDocument;
    const active = doc.activeElement as HTMLElement | null;
    const focusLost = !active || active === doc.body;
    if (detailOpen) {
      opener.current = active && listEl.contains(active)
        ? { element: active, testId: (active.closest(".cc-row") ?? active).getAttribute("data-testid") }
        : null;
      // Side by side the list stays visible and keeps focus; in a narrow
      // pane it is hidden under the detail, so focus moves into the detail.
      const listHidden = doc.defaultView?.getComputedStyle(listEl).visibility === "hidden";
      if (!listHidden || !(focusLost || listEl.contains(active))) return;
      const heading = detailEl.querySelector<HTMLElement>("h1, h2, h3, h4, h5, h6");
      if (heading && !heading.hasAttribute("tabindex")) heading.setAttribute("tabindex", "-1");
      (heading ?? detailEl).focus();
      return;
    }
    // Closed: Back unmounts the focused Back row, so focus would drop to
    // the page. Return it to the row the detail showed.
    if (!(focusLost || detailEl.contains(active))) return;
    const saved = opener.current;
    const selectedRow = focusTarget(listEl.querySelector('[aria-current="true"]'));
    const openerRow = saved && saved.element.isConnected && listEl.contains(saved.element) ? saved.element : null;
    const target = selectedRow ?? openerRow ?? (saved?.testId
      ? focusTarget(Array.from(listEl.querySelectorAll("[data-testid]")).find((element) => element.getAttribute("data-testid") === saved.testId) ?? null)
      : null);
    target?.focus();
  }, [detailOpen]);

  return (
    <div className={cx("cc-split", detailOpen && "has-detail", className)} data-testid={testId}>
      <div className="cc-split__list" ref={listRef}>{list}</div>
      <div className="cc-split__detail" ref={detailRef} tabIndex={-1}>
        {detailOpen && onBack ? (
          <button type="button" className="cc-split__back" onClick={onBack}>
            <span className="cc-split__back-mark" aria-hidden="true" />
            {backLabel}
          </button>
        ) : null}
        {detail ?? emptyDetail ?? null}
      </div>
    </div>
  );
}
