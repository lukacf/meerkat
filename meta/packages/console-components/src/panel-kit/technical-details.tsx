import { useEffect, useRef, useState, type ReactNode } from "react";

import { copyTextToClipboard } from "../shared";
import { cx } from "./cx";
import { KeyValueList } from "./key-value";
import { usePanelOptions } from "./panel";
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

function present(item: TechnicalDetail | null | false | undefined): item is TechnicalDetail {
  return Boolean(item) && (item as TechnicalDetail).value != null && (item as TechnicalDetail).value !== "";
}

/** "Label: value" lines of the rows whose value is text or a number. */
export function technicalDetailsText(items: ReadonlyArray<TechnicalDetail | null | false | undefined>): string {
  return items
    .filter(present)
    .filter((row) => typeof row.value === "string" || typeof row.value === "number")
    .map((row) => `${row.label}: ${row.value}`)
    .join("\n");
}

function CopyAll({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (timer.current) clearTimeout(timer.current);
  }, []);
  return (
    <button
      type="button"
      className="cc-btn cc-btn--ghost cc-btn--sm cc-tech__copy-all"
      data-copied={copied ? "true" : undefined}
      onClick={() => {
        void copyTextToClipboard(text).then((done) => {
          if (!done) return;
          setCopied(true);
          if (timer.current) clearTimeout(timer.current);
          timer.current = setTimeout(() => setCopied(false), 1600);
        });
      }}
    >
      {copied ? "Copied" : "Copy all"}
    </button>
  );
}

/**
 * A collapsed "Technical details" disclosure: IDs, keys, hashes and sequence
 * numbers that people rarely need, each copyable. Generalises the work item
 * and approval request detail disclosures. The summary shows how many
 * fields it holds ("Technical details · 4") and a "Copy all" button heads
 * the body (it copies "Label: value" lines). `inline`, or the panel's "Show
 * technical IDs" option, shows the ids in a row instead of the disclosure.
 */
export function TechnicalDetails({
  items,
  summary = "Technical details",
  open,
  inline,
  copyAll = true,
  showCount = true,
  children,
  className,
  testId,
}: {
  items?: ReadonlyArray<TechnicalDetail | null | false | undefined>;
  summary?: ReactNode;
  open?: boolean;
  /** Ids in a row, no disclosure; follows the panel's option when unset. */
  inline?: boolean;
  /** The "Copy all" button (shown with two or more text fields). */
  copyAll?: boolean;
  /** The field count after the summary. */
  showCount?: boolean;
  children?: ReactNode;
  className?: string;
  testId?: string;
}) {
  const options = usePanelOptions();
  const rows = (items ?? []).filter(present);
  if (!rows.length && !children) return null;
  const inlineMode = inline ?? options.showTechnicalIds;
  const renderValue = (row: TechnicalDetail) =>
    typeof row.value === "string" && !row.plain
      ? <TechnicalId value={row.value} label={row.label} />
      : typeof row.value === "number"
        ? String(row.value)
        : row.value;
  if (inlineMode) {
    return (
      <div className={cx("cc-tech", "cc-tech--inline", className)} data-testid={testId}>
        {rows.map((row, index) => (
          <span className="cc-tech__pair" key={`${row.label}:${index}`} data-testid={row.testId}>
            <span className="cc-tech__key">{row.label}</span>
            {renderValue(row)}
          </span>
        ))}
        {children}
      </div>
    );
  }
  const copyText = copyAll ? technicalDetailsText(rows) : "";
  const copyRows = copyText ? copyText.split("\n").length : 0;
  return (
    <details className={cx("cc-tech", className)} open={open} data-testid={testId}>
      <summary className="cc-tech__summary">
        <span className="cc-tech__label">{summary}</span>
        {showCount && rows.length ? <span className="cc-tech__count">{` · ${rows.length}`}</span> : null}
      </summary>
      <div className="cc-tech__body">
        {copyRows >= 2 ? (
          <div className="cc-tech__tools">
            <CopyAll text={copyText} />
          </div>
        ) : null}
        <KeyValueList
          items={rows.map((row, index) => ({
            // Labels repeat (one "Session ID" per binding): key by position too.
            key: `${row.label}:${index}`,
            label: row.label,
            testId: row.testId,
            value: renderValue(row),
          }))}
        />
        {children}
      </div>
    </details>
  );
}
