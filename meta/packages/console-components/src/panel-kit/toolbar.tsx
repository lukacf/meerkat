import { useId, type ChangeEvent, type ComponentPropsWithRef, type ReactNode } from "react";

import { cx } from "./cx";
import { KitGlyph } from "./glyphs";

/** A row of filters and view controls under the header or tabs. Wraps. */
export function Toolbar({
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
    <div className={cx("cc-toolbar", className)} data-testid={testId} role={ariaLabel ? "toolbar" : undefined} aria-label={ariaLabel}>
      {children}
    </div>
  );
}

/** A labelled search input with the search glyph. The label is visually
 * hidden unless `showLabel`; the input always has an accessible name. */
export function SearchField({
  label,
  showLabel,
  value,
  onChange,
  placeholder,
  className,
  inputClassName,
  testId,
  ...rest
}: Omit<ComponentPropsWithRef<"input">, "onChange" | "value"> & {
  label: string;
  showLabel?: boolean;
  value: string;
  onChange: (value: string, event: ChangeEvent<HTMLInputElement>) => void;
  inputClassName?: string;
  testId?: string;
}) {
  const id = useId();
  return (
    <div className={cx("cc-field", "cc-search", className)}>
      <label htmlFor={id} className={showLabel ? "cc-field__label" : "cc-visually-hidden"}>
        {label}
      </label>
      <span className="cc-search__box">
        <KitGlyph name="search" className="cc-search__icon" />
        <input
          id={id}
          type="search"
          className={cx("cc-input", "cc-search__input", inputClassName)}
          value={value}
          placeholder={placeholder}
          onChange={(event) => onChange(event.target.value, event)}
          data-testid={testId}
          {...rest}
        />
      </span>
    </div>
  );
}

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

/** A styled native select with a visible (or visually hidden) label and the
 * kit chevron. The native element keeps keyboard and assistive behaviour. */
export function SelectField({
  label,
  showLabel = true,
  value,
  options,
  onChange,
  className,
  selectClassName,
  testId,
  ariaLabel,
  ...rest
}: Omit<ComponentPropsWithRef<"select">, "onChange" | "value"> & {
  label: string;
  showLabel?: boolean;
  value: string;
  options: ReadonlyArray<SelectOption>;
  onChange: (value: string, event: ChangeEvent<HTMLSelectElement>) => void;
  selectClassName?: string;
  testId?: string;
  /** Accessible name when it must differ from the visible label. */
  ariaLabel?: string;
}) {
  const id = useId();
  return (
    <div className={cx("cc-field", className)}>
      <label htmlFor={id} className={showLabel ? "cc-field__label" : "cc-visually-hidden"}>
        {label}
      </label>
      <span className="cc-select">
        <select
          id={id}
          className={cx("cc-select__control", selectClassName)}
          value={value}
          onChange={(event) => onChange(event.target.value, event)}
          data-testid={testId}
          aria-label={ariaLabel}
          {...rest}
        >
          {options.map((option) => (
            <option key={option.value} value={option.value} disabled={option.disabled}>
              {option.label}
            </option>
          ))}
        </select>
        <KitGlyph name="chevron" className="cc-select__chevron" />
      </span>
    </div>
  );
}

export interface SegmentOption<Id extends string = string> {
  id: Id;
  label: ReactNode;
  count?: number | string | null;
  testId?: string;
  ariaLabel?: string;
}

/** Mutually exclusive view or filter buttons with aria-pressed. */
export function SegmentedControl<Id extends string>({
  options,
  value,
  onChange,
  ariaLabel,
  className,
  testId,
}: {
  options: ReadonlyArray<SegmentOption<Id>>;
  value: Id;
  onChange: (id: Id) => void;
  ariaLabel: string;
  className?: string;
  testId?: string;
}) {
  return (
    <div className={cx("cc-seg", className)} role="group" aria-label={ariaLabel} data-testid={testId}>
      {options.map((option) => {
        const pressed = option.id === value;
        return (
          <button
            key={option.id}
            type="button"
            className={cx("cc-seg__btn", pressed && "is-active")}
            aria-pressed={pressed}
            aria-label={option.ariaLabel}
            data-testid={option.testId}
            onClick={() => onChange(option.id)}
          >
            {option.label}
            {option.count !== null && option.count !== undefined && option.count !== "" ? (
              <>
                {" "}
                <span className="cc-count">{option.count}</span>
              </>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
