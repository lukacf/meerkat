import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";

import { cx } from "./cx";

export type PopoverCloseReason = "escape" | "outside" | "focus" | "select";
export type PopoverPlacement = "bottom-end" | "bottom-start" | "top-end" | "top-start";

export interface PopoverProps {
  open: boolean;
  /** The trigger the popover hangs from; focus returns to it on Escape. */
  anchorRef: RefObject<HTMLElement | null>;
  onClose: (reason: PopoverCloseReason) => void;
  /** `dialog` for view options, `menu` for a menu of commands. */
  role?: "dialog" | "menu";
  ariaLabel?: string;
  ariaLabelledBy?: string;
  placement?: PopoverPlacement;
  /** Gap to the trigger in px. */
  offset?: number;
  /** A fixed width in px; otherwise the content's, between min and max. */
  width?: number;
  minWidth?: number;
  maxWidth?: number;
  /** The element to focus when the popover opens; the first control
   * otherwise. Return null to leave focus where it is. */
  initialFocus?: (popover: HTMLElement) => HTMLElement | null | undefined;
  /** Keys the popover does not handle itself (menus navigate with them). */
  onKeyDown?: (event: KeyboardEvent<HTMLDivElement>) => void;
  id?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}

const MARGIN = 8;
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** The browser's top layer (the Popover API), where present. */
function supportsTopLayer(): boolean {
  return typeof HTMLElement !== "undefined" && typeof HTMLElement.prototype.showPopover === "function";
}

/** Whether the trigger has scrolled wholly out of the viewport or out of a
 * clipping ancestor (a pane body, a scrolling list). A trigger without a
 * box (not laid out, or no layout engine) counts as in sight. */
function anchorOutOfSight(anchor: HTMLElement): boolean {
  const rect = anchor.getBoundingClientRect();
  if (rect.width === 0 && rect.height === 0) return false;
  const view = anchor.ownerDocument.defaultView;
  const outside = (box: { top: number; left: number; bottom: number; right: number }) =>
    rect.bottom <= box.top || rect.top >= box.bottom || rect.right <= box.left || rect.left >= box.right;
  if (view && outside({ top: 0, left: 0, bottom: view.innerHeight, right: view.innerWidth })) return true;
  for (let node = anchor.parentElement; node && node !== anchor.ownerDocument.body; node = node.parentElement) {
    const style = view?.getComputedStyle(node);
    if (style && /(auto|scroll|hidden|clip)/.test(`${style.overflowX} ${style.overflowY}`) && outside(node.getBoundingClientRect())) {
      return true;
    }
  }
  return false;
}

function prefersReducedMotion(element: Element): boolean {
  if (element.closest(".reduced-motion")) return true;
  const view = element.ownerDocument.defaultView;
  return Boolean(view?.matchMedia?.("(prefers-reduced-motion: reduce)").matches);
}

/**
 * A non-modal popover anchored to its trigger (spec A3.2): bottom end by
 * default, flipping above and shifting to stay inside the pane and then the
 * viewport, 4 px from the trigger. It renders inline right after its trigger,
 * so it stays inside the console's theme scope (tokens and variants apply)
 * and keeps the document's tab order, and lifts itself into the browser's
 * top layer (the Popover API) so no pane overflow, container or z-index can
 * clip it; without the API it is a fixed-position layer in place. Escape
 * closes it and returns focus to the trigger; a pointer press outside it,
 * focus moving outside it, or a scroll that takes the trigger out of sight
 * closes it too.
 */
export function Popover({
  open,
  anchorRef,
  onClose,
  role = "dialog",
  ariaLabel,
  ariaLabelledBy,
  placement = "bottom-end",
  offset = 4,
  width,
  minWidth = 200,
  maxWidth = 320,
  initialFocus,
  onKeyDown,
  id,
  className,
  testId,
  children,
}: PopoverProps) {
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const initialFocusRef = useRef(initialFocus);
  initialFocusRef.current = initialFocus;
  const [position, setPosition] = useState<{ top: number; left: number; side: "top" | "bottom" } | null>(null);
  // A popover that closes fades out over the exit duration before it
  // unmounts, where the browser animates and motion is not reduced (never in
  // jsdom, so tests see it gone at once).
  const [shownOpen, setShownOpen] = useState(open);
  const [closing, setClosing] = useState(false);
  if (shownOpen !== open) {
    setShownOpen(open);
    const element = popoverRef.current;
    setClosing(!open && Boolean(element && typeof element.animate === "function" && !prefersReducedMotion(element)));
  }
  const topLayer = supportsTopLayer();

  const place = useCallback(() => {
    const anchor = anchorRef.current;
    const popover = popoverRef.current;
    if (!anchor || !popover) return;
    const view = anchor.ownerDocument.defaultView;
    const vw = view?.innerWidth || 0;
    const vh = view?.innerHeight || 0;
    const trigger = anchor.getBoundingClientRect();
    const box = popover.getBoundingClientRect();
    const viewport = { left: MARGIN, top: MARGIN, right: (vw || Infinity) - MARGIN, bottom: (vh || Infinity) - MARGIN };
    const paneRect = anchor.closest(".pane__body")?.getBoundingClientRect();
    const pane = paneRect
      ? {
          left: Math.max(paneRect.left + MARGIN, viewport.left),
          top: Math.max(paneRect.top + MARGIN, viewport.top),
          right: Math.min(paneRect.right - MARGIN, viewport.right),
          bottom: Math.min(paneRect.bottom - MARGIN, viewport.bottom),
        }
      : viewport;
    // Shift inside the pane when the popover fits there, else the viewport.
    const horizontal = pane.right - pane.left >= box.width ? pane : viewport;
    const vertical = pane.bottom - pane.top >= box.height ? pane : viewport;
    const preferredLeft = placement.endsWith("end") ? trigger.right - box.width : trigger.left;
    const left = Math.min(Math.max(preferredLeft, horizontal.left), Math.max(horizontal.left, horizontal.right - box.width));
    const below = trigger.bottom + offset;
    const above = trigger.top - offset - box.height;
    const fitsBelow = below + box.height <= vertical.bottom;
    const fitsAbove = above >= vertical.top;
    const wantsTop = placement.startsWith("top");
    let side: "top" | "bottom" = wantsTop ? (fitsAbove || !fitsBelow ? "top" : "bottom") : (fitsBelow || !fitsAbove ? "bottom" : "top");
    if (!fitsBelow && !fitsAbove) {
      side = vertical.bottom - trigger.bottom >= trigger.top - vertical.top ? "bottom" : "top";
    }
    const rawTop = side === "bottom" ? below : above;
    const top = Math.min(Math.max(rawTop, viewport.top), Math.max(viewport.top, viewport.bottom - box.height));
    setPosition((previous) => (previous && previous.top === top && previous.left === left && previous.side === side ? previous : { top, left, side }));
  }, [anchorRef, offset, placement]);

  useEffect(() => {
    if (!closing) return undefined;
    const timer = setTimeout(() => setClosing(false), 140);
    return () => clearTimeout(timer);
  }, [closing]);

  useLayoutEffect(() => {
    if (!open) return undefined;
    const popover = popoverRef.current;
    if (popover && topLayer && !popover.matches(":popover-open")) {
      try {
        popover.showPopover();
      } catch {
        // Not connected yet or already shown: the fixed layer still places it.
      }
    }
    place();
    if (popover) {
      const chosen = initialFocusRef.current ? initialFocusRef.current(popover) : popover.querySelector<HTMLElement>(FOCUSABLE);
      if (chosen !== null) (chosen ?? popover).focus({ preventScroll: true });
    }
    const view = anchorRef.current?.ownerDocument.defaultView;
    // A scroll that takes the trigger out of sight closes the popover rather
    // than leaving it pinned to the edge of the view, away from its trigger.
    const onScroll = (event: Event) => {
      const target = event.target as Node | null;
      if (target && "nodeType" in target && popoverRef.current?.contains(target)) return;
      const anchor = anchorRef.current;
      if (anchor && anchorOutOfSight(anchor)) {
        const hadFocus = Boolean(popoverRef.current?.contains(anchor.ownerDocument.activeElement));
        onCloseRef.current("outside");
        if (hadFocus) anchor.focus({ preventScroll: true });
        return;
      }
      place();
    };
    view?.addEventListener("resize", place);
    view?.addEventListener("scroll", onScroll, true);
    const observer = typeof ResizeObserver !== "undefined" && popover ? new ResizeObserver(() => place()) : null;
    if (observer && popover) observer.observe(popover);
    return () => {
      view?.removeEventListener("resize", place);
      view?.removeEventListener("scroll", onScroll, true);
      observer?.disconnect();
    };
    // Focus moves in once per opening.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, place]);

  // A press outside closes it (the trigger toggles it itself).
  useEffect(() => {
    if (!open) return undefined;
    const doc = anchorRef.current?.ownerDocument;
    if (!doc) return undefined;
    const onPointerDown = (event: Event) => {
      const target = event.target as Node | null;
      if (!target || popoverRef.current?.contains(target) || anchorRef.current?.contains(target)) return;
      onCloseRef.current("outside");
    };
    doc.addEventListener("pointerdown", onPointerDown, true);
    return () => doc.removeEventListener("pointerdown", onPointerDown, true);
  }, [open, anchorRef]);

  if (!open && !closing) return null;
  const style: CSSProperties = {
    top: position?.top ?? 0,
    left: position?.left ?? 0,
    width,
    minWidth: width ?? minWidth,
    maxWidth: width ?? maxWidth,
    // Unplaced for one layout pass only: transparent, never `visibility:
    // hidden`, which would make its controls unfocusable while focus moves in.
    opacity: position ? undefined : 0,
  };
  return (
    <div
      ref={popoverRef}
      id={id}
      role={role}
      aria-modal={role === "dialog" ? false : undefined}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
      aria-hidden={closing ? true : undefined}
      tabIndex={-1}
      popover={topLayer ? "manual" : undefined}
      className={cx("cc-popover", className)}
      data-side={position?.side}
      data-state={closing ? "closing" : "open"}
      data-testid={testId}
      style={style}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onCloseRef.current("escape");
          anchorRef.current?.focus();
          return;
        }
        onKeyDown?.(event);
      }}
      onBlur={(event) => {
        // Close when focus moves to something outside; focus that goes
        // nowhere (a control that became disabled) keeps it open.
        const next = event.relatedTarget as Node | null;
        if (!open || !next || popoverRef.current?.contains(next) || anchorRef.current?.contains(next)) return;
        onCloseRef.current("focus");
      }}
    >
      {children}
    </div>
  );
}
