import type { ReactNode } from "react";

import { cx } from "./cx";

/**
 * A list with a detail column. In a pane at least 720 px wide the two sit
 * side by side and the detail column always shows `detail` (or
 * `emptyDetail`). Narrower, the list shows alone until `detailOpen`, then
 * the detail replaces it (the list keeps its scroll position) under a Back
 * row.
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
  return (
    <div className={cx("cc-split", detailOpen && "has-detail", className)} data-testid={testId}>
      <div className="cc-split__list">{list}</div>
      <div className="cc-split__detail">
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
