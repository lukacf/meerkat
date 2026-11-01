import { useEffect, useRef, useState, type ReactNode } from "react";

import { IconButton } from "./button";
import { cx } from "./cx";
import { KitGlyph, type KitGlyphName } from "./glyphs";
import { StatusMark } from "./status-mark";
import { SearchField } from "./toolbar";
import type { Tone } from "./tone";

export interface FilterChipProps {
  label: ReactNode;
  count?: number | string | null;
  pressed: boolean;
  onToggle: (pressed: boolean) => void;
  /** A status filter carries its tone's mark ("Blocked" warn). */
  tone?: Tone;
  /** A kind filter carries its glyph (memory kinds). */
  glyph?: KitGlyphName;
  /** The accessible name when the label is not plain text. */
  ariaLabel?: string;
  testId?: string;
  className?: string;
}

/** A toggle pill with a count (spec A3.11): "Blocked 2", "Mine". */
export function FilterChip({ label, count, pressed, onToggle, tone, glyph, ariaLabel, testId, className }: FilterChipProps) {
  return (
    <button
      type="button"
      className={cx("cc-filter-chip", pressed && "is-pressed", className)}
      aria-pressed={pressed}
      aria-label={ariaLabel}
      data-tone={tone}
      data-testid={testId}
      onClick={() => onToggle(!pressed)}
    >
      {tone ? <StatusMark tone={tone} /> : glyph ? <KitGlyph name={glyph} className="cc-filter-chip__glyph" /> : null}
      <span className="cc-filter-chip__label">{label}</span>
      {count !== null && count !== undefined && count !== "" ? <span className="cc-count">{count}</span> : null}
    </button>
  );
}

export interface FilterBarSearch {
  /** The input's accessible name ("Search agents"). */
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  testId?: string;
}

/**
 * The filter row under a panel's header or tabs (spec A3.11): a search
 * field ("/" focuses it while focus is in the panel), then filter chips,
 * then a quiet Clear that shows only while something is not default. The
 * filters scope everything below them. In a pane under 480 px the search
 * folds into an icon button that opens it over the row.
 */
export function FilterBar({
  search,
  chips,
  onClear,
  clearLabel = "Clear",
  modified: modifiedProp,
  ariaLabel = "Filters",
  className,
  testId,
  children,
}: {
  search?: FilterBarSearch;
  chips?: ReadonlyArray<FilterChipProps & { key?: string }>;
  /** Restores the defaults; Clear shows while anything differs. */
  onClear?: () => void;
  clearLabel?: string;
  /** Whether anything differs from the default; by default a search text
   * or a pressed chip. */
  modified?: boolean;
  ariaLabel?: string;
  className?: string;
  testId?: string;
  /** More controls at the end (a scope select, a sort). */
  children?: ReactNode;
}) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const modified = modifiedProp ?? Boolean((search && search.value) || chips?.some((chip) => chip.pressed));
  useEffect(() => {
    const root = rootRef.current;
    if (!root || !search) return undefined;
    const scope: HTMLElement | Document = root.closest<HTMLElement>(".cc-panel") ?? root.ownerDocument;
    const onKeyDown = (event: Event) => {
      const key = event as KeyboardEvent;
      if (key.key !== "/" || key.ctrlKey || key.metaKey || key.altKey) return;
      const target = key.target as HTMLElement | null;
      if (target?.closest?.("input, textarea, select, [contenteditable='true']")) return;
      const input = root.querySelector<HTMLInputElement>(".cc-filterbar__search input");
      if (!input) return;
      key.preventDefault();
      setSearchOpen(true);
      input.focus();
    };
    scope.addEventListener("keydown", onKeyDown);
    return () => scope.removeEventListener("keydown", onKeyDown);
  }, [search]);
  return (
    <div
      ref={rootRef}
      className={cx("cc-filterbar", searchOpen && "is-search-open", className)}
      role="toolbar"
      aria-label={ariaLabel}
      data-testid={testId}
    >
      {search ? (
        <>
          <IconButton
            label={search.label}
            glyph="search"
            size="sm"
            className="cc-filterbar__search-toggle"
            aria-expanded={searchOpen}
            onClick={() => {
              setSearchOpen(true);
              requestAnimationFrame(() => rootRef.current?.querySelector<HTMLInputElement>(".cc-filterbar__search input")?.focus());
            }}
          />
          <SearchField
            label={search.label}
            value={search.value}
            onChange={(value) => search.onChange(value)}
            placeholder={search.placeholder ?? search.label}
            shortcut="/"
            testId={search.testId}
            className="cc-filterbar__search"
            onBlur={() => {
              if (!search.value) setSearchOpen(false);
            }}
          />
        </>
      ) : null}
      {chips?.length ? (
        <span className="cc-filterbar__chips">
          {chips.map((chip, index) => (
            <FilterChip key={chip.key ?? chip.testId ?? index} {...chip} />
          ))}
        </span>
      ) : null}
      {children}
      {onClear && modified ? (
        <button type="button" className="cc-filterbar__clear" onClick={onClear}>
          {clearLabel}
        </button>
      ) : null}
    </div>
  );
}
