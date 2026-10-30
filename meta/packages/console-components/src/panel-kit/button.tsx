import type { ComponentPropsWithRef, ReactNode } from "react";

import { cx } from "./cx";

export type ButtonVariant = "default" | "primary" | "danger" | "ghost";
export type ButtonSize = "md" | "sm";

export type ButtonProps = ComponentPropsWithRef<"button"> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** A leading icon; decorative, the label or aria-label names the button. */
  icon?: ReactNode;
  /** A square icon button; requires an aria-label. */
  iconOnly?: boolean;
  /** Wrap the label so a narrow pane can hide it and keep the icon. */
  collapsible?: boolean;
};

/**
 * The one console button: outline by default, accent-filled primary, danger
 * and ghost variants, 28 px (24 px small). The label stays a direct text
 * child, so tests and assistive tech read the button's own text.
 */
export function Button({
  variant = "default",
  size = "md",
  icon,
  iconOnly,
  collapsible,
  className,
  children,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cx(
        "cc-btn",
        variant !== "default" && `cc-btn--${variant}`,
        size === "sm" && "cc-btn--sm",
        iconOnly && "cc-btn--icon",
        className,
      )}
      {...rest}
    >
      {icon ? (
        <span className="cc-btn__icon" aria-hidden="true">
          {icon}
        </span>
      ) : null}
      {collapsible && children != null ? <span className="cc-btn__label">{children}</span> : children}
    </button>
  );
}
