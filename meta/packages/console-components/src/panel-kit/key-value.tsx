import type { ReactNode } from "react";

import { cx } from "./cx";

export interface KeyValueItem {
  label: ReactNode;
  /** Empty values (null, undefined, "") are skipped. */
  value: ReactNode;
  key?: string;
  testId?: string;
}

function present(item: KeyValueItem | null | false | undefined): item is KeyValueItem {
  return Boolean(item) && (item as KeyValueItem).value !== null && (item as KeyValueItem).value !== undefined
    && (item as KeyValueItem).value !== "" && (item as KeyValueItem).value !== false;
}

/** Labelled values in two aligned columns (<dl class="cc-kv">); stacks in a
 * narrow pane. Values may be chips, names, times or technical IDs. */
export function KeyValueList({
  items,
  className,
  testId,
}: {
  items: ReadonlyArray<KeyValueItem | null | false | undefined>;
  className?: string;
  testId?: string;
}) {
  const rows = items.filter(present);
  if (!rows.length) return null;
  return (
    <dl className={cx("cc-kv", className)} data-testid={testId}>
      {rows.map((row, index) => (
        <div
          className="cc-kv__row"
          key={row.key ?? (typeof row.label === "string" ? `${row.label}:${index}` : index)}
          data-testid={row.testId}
        >
          <dt>{row.label}</dt>
          <dd>{row.value}</dd>
        </div>
      ))}
    </dl>
  );
}
