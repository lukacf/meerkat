import { useId, useRef, useState, type ReactNode, type RefObject } from "react";

import { IconButton } from "./button";
import { cx } from "./cx";
import { Popover, type PopoverCloseReason, type PopoverPlacement } from "./popover";
import { SegmentedControl, type SegmentOption } from "./toolbar";

export interface ViewOptionsControl<Id extends string = string> {
  /** The visible micro-label above the control ("Group by"). */
  label: string;
  /** The control's accessible name; the label when absent. */
  ariaLabel?: string;
  options: ReadonlyArray<SegmentOption<Id>>;
  value: Id;
  onChange: (id: Id) => void;
  testId?: string;
  /** A meta line under the control ("Manual order: drag agents to reorder"). */
  hint?: ReactNode;
}

export interface ViewOptionsProps {
  open: boolean;
  anchorRef: RefObject<HTMLElement | null>;
  onClose: (reason: PopoverCloseReason) => void;
  /** The popover's name for assistive tech ("Roster view"). */
  ariaLabel: string;
  // Any control's own option ids: each control is typed at its call site.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  controls: ReadonlyArray<ViewOptionsControl<any>>;
  /** Restores the default view; disabled while the view is the default. */
  onReset: () => void;
  resetDisabled: boolean;
  resetLabel?: string;
  resetTestId?: string;
  /** A meta line above the footer. */
  hint?: ReactNode;
  placement?: PopoverPlacement;
  width?: number;
  id?: string;
  className?: string;
  testId?: string;
}

/**
 * Several view settings at once (spec A3.2): labelled segmented controls
 * stacked in a non-modal popover (role="dialog", aria-modal="false"), with
 * "Reset view" at the foot. A change applies at once and the popover stays
 * open; opening focuses the pressed button of the first control; Escape
 * closes it and returns focus to the trigger.
 */
export function ViewOptions({
  open,
  anchorRef,
  onClose,
  ariaLabel,
  controls,
  onReset,
  resetDisabled,
  resetLabel = "Reset view",
  resetTestId,
  hint,
  placement = "bottom-end",
  width = 248,
  id,
  className,
  testId,
}: ViewOptionsProps) {
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const focusFirstPressed = () => {
    const pressed = bodyRef.current?.querySelector<HTMLElement>('.cc-seg__btn[aria-pressed="true"]');
    pressed?.focus();
  };
  return (
    <Popover
      open={open}
      anchorRef={anchorRef}
      onClose={onClose}
      role="dialog"
      ariaLabel={ariaLabel}
      placement={placement}
      width={width}
      id={id}
      testId={testId}
      className={cx("cc-view-options", className)}
      initialFocus={(popover) => popover.querySelector<HTMLElement>('.cc-seg__btn[aria-pressed="true"]')}
    >
      <div className="cc-view-options__body" ref={bodyRef}>
        {controls.map((control) => (
          <div className="cc-view-options__control" key={control.label}>
            <div className="cc-view-options__label" aria-hidden="true">
              {control.label}
            </div>
            <SegmentedControl
              options={control.options}
              value={control.value}
              onChange={control.onChange}
              ariaLabel={control.ariaLabel || control.label}
              testId={control.testId}
              className="cc-view-options__seg"
            />
            {control.hint ? <div className="cc-view-options__hint">{control.hint}</div> : null}
          </div>
        ))}
        {hint ? <div className="cc-view-options__hint">{hint}</div> : null}
      </div>
      <div className="cc-view-options__foot">
        <button
          type="button"
          className="cc-view-options__reset"
          disabled={resetDisabled}
          onClick={() => {
            onReset();
            // The button disables itself once the view is the default: keep
            // focus inside the popover, on the first control.
            requestAnimationFrame(focusFirstPressed);
          }}
          data-testid={resetTestId}
        >
          {resetLabel}
        </button>
      </div>
    </Popover>
  );
}

/**
 * A view-options trigger and its popover (the sidebar's view menu, A5): an
 * ellipsis icon button with `aria-haspopup="dialog"` that carries a 5 px
 * accent dot while the view is not the default (and then stays visible).
 */
export function ViewOptionsMenu({
  label,
  ariaLabel,
  controls,
  onReset,
  isDefault,
  hint,
  triggerTestId,
  resetTestId,
  testId,
  size = "sm",
  reveal,
  placement = "bottom-end",
  width = 248,
  triggerClassName,
  className,
  onOpenChange,
}: {
  /** The trigger's accessible name ("Roster view options"). */
  label: string;
  /** The popover's name ("Roster view"). */
  ariaLabel: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  controls: ReadonlyArray<ViewOptionsControl<any>>;
  onReset: () => void;
  isDefault: boolean;
  hint?: ReactNode;
  triggerTestId?: string;
  resetTestId?: string;
  testId?: string;
  size?: "sm" | "md";
  /** Hover-revealed while the view is the default (A4.1). */
  reveal?: boolean;
  placement?: PopoverPlacement;
  width?: number;
  triggerClassName?: string;
  className?: string;
  onOpenChange?: (open: boolean) => void;
}) {
  const [open, setOpenState] = useState(false);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const popoverId = `${useId()}-view`;
  const setOpen = (next: boolean) => {
    setOpenState(next);
    onOpenChange?.(next);
  };
  return (
    <>
      <IconButton
        ref={triggerRef}
        label={label}
        glyph="ellipsis"
        size={size}
        quiet
        reveal={reveal && isDefault}
        badge={!isDefault}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popoverId : undefined}
        className={cx("cc-view-options-trigger", triggerClassName)}
        data-testid={triggerTestId}
        onClick={() => setOpen(!open)}
      />
      <ViewOptions
        open={open}
        anchorRef={triggerRef}
        onClose={() => setOpen(false)}
        ariaLabel={ariaLabel}
        controls={controls}
        onReset={onReset}
        resetDisabled={isDefault}
        resetTestId={resetTestId}
        hint={hint}
        placement={placement}
        width={width}
        id={popoverId}
        testId={testId}
        className={className}
      />
    </>
  );
}
