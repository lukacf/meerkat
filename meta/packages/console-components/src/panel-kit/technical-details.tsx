import type { ReactNode } from "react";

import { cx } from "./cx";
import { KeyValueList } from "./key-value";
import { TechnicalId } from "./technical-id";

export interface TechnicalDetail {
  label: string;
  /** Strings render as a copyable TechnicalId; numbers and nodes render as
   * given. Empty values are skipped. */
  value: string | number | ReactNode | null | undefined;
  /** Plain text instead of a TechnicalId (kinds, short words). */
  plain?: boolean;
  testId?: string;
}

/**
 * A collapsed "Technical details" disclosure: IDs, keys, hashes and sequence
 * numbers that people rarely need, each copyable. Generalises the work item
 * and approval request detail disclosures.
 */
export function TechnicalDetails({
  items,
  summary = "Technical details",
  open,
  children,
  className,
  testId,
}: {
  items?: ReadonlyArray<TechnicalDetail | null | false | undefined>;
  summary?: ReactNode;
  open?: boolean;
  children?: ReactNode;
  className?: string;
  testId?: string;
}) {
  const rows = (items ?? []).filter(
    (item): item is TechnicalDetail =>
      Boolean(item) && (item as TechnicalDetail).value != null && (item as TechnicalDetail).value !== "",
  );
  if (!rows.length && !children) return null;
  return (
    <details className={cx("cc-tech", className)} open={open} data-testid={testId}>
      <summary className="cc-tech__summary">{summary}</summary>
      <div className="cc-tech__body">
        <KeyValueList
          items={rows.map((row) => ({
            key: row.label,
            label: row.label,
            testId: row.testId,
            value:
              typeof row.value === "string" && !row.plain
                ? <TechnicalId value={row.value} label={row.label} />
                : typeof row.value === "number"
                  ? String(row.value)
                  : row.value,
          }))}
        />
        {children}
      </div>
    </details>
  );
}
