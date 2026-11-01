import type { ComponentPropsWithRef, ReactNode } from "react";

import { cx } from "./cx";
import { KitGlyph, type KitGlyphName } from "./glyphs";

export type ButtonVariant = "default" | "primary" | "positive" | "danger" | "ghost";
export type ButtonSize = "md" | "sm";

export type ButtonProps = ComponentPropsWithRef<"button"> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** A leading icon; decorative, the label or aria-label names the button. */
  icon?: ReactNode;
} & ButtonNaming;

/**
 * A button whose visible label can be missing names itself, and the type
 * says so: an icon-only button, or one whose label a narrow pane hides,
 * takes a required aria-label.
 */
type ButtonNaming =
  | {
      /** A square icon button. */
      iconOnly: true;
      collapsible?: boolean;
      "aria-label": string;
    }
  | {
      iconOnly?: boolean;
      /** Wrap the label so a narrow pane can hide it and keep the icon. */
      collapsible: true;
      "aria-label": string;
    }
  | { iconOnly?: false; collapsible?: false };

/**
 * The one console button: outline by default, accent-filled primary, danger
 * and ghost variants, 28 px (24 px small). The label stays a direct text
 * child, so tests and assistive tech read the button's own text. A pressed
 * (`aria-pressed`) or expanded (`aria-expanded`) button shows the soft fill.
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

export type IconButtonProps = Omit<ComponentPropsWithRef<"button">, "children"> & {
  /** The accessible name, also the hover tooltip. */
  label: string;
  /** A kit glyph (KitGlyph name). */
  glyph?: KitGlyphName;
  /** Any other icon (a sprite Icon from the app); `glyph` wins. */
  icon?: ReactNode;
  /** 28 px (`md`, headers) or 24 px (`sm`, rows and menus). */
  size?: ButtonSize;
  /** Meta ink until hovered: the quiet overflow trigger of a panel header. */
  quiet?: boolean;
  /** Hover-revealed (A4.1): transparent until its container is hovered or
   * focused, always in the tab order. Needs a `cc-reveal-host` container. */
  reveal?: boolean;
  /** A 5 px accent dot at the top right: a non-default state behind it. */
  badge?: boolean;
  /** Overrides the tooltip (defaults to `label`). */
  title?: string;
};

/**
 * An icon-only ghost button (spec A3.2): 28 x 28 (24 x 24 small), the glyph
 * at 16 px (14 px small), named by `label` (`aria-label` and `title`).
 * Pressed or expanded shows the soft fill.
 */
export function IconButton({
  label,
  glyph,
  icon,
  size = "md",
  quiet,
  reveal,
  badge,
  title,
  className,
  type = "button",
  ...rest
}: IconButtonProps) {
  return (
    <button
      type={type}
      aria-label={label}
      title={title ?? label}
      className={cx(
        "cc-btn",
        "cc-btn--icon",
        "cc-btn--ghost",
        size === "sm" && "cc-btn--sm",
        quiet && "cc-btn--quiet",
        reveal && "cc-reveal",
        badge && "has-badge",
        className,
      )}
      {...rest}
    >
      <span className="cc-btn__icon" aria-hidden="true">
        {glyph ? <KitGlyph name={glyph} /> : icon}
      </span>
      {badge ? <span className="cc-btn__badge" aria-hidden="true" /> : null}
    </button>
  );
}
