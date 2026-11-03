import React from "react";

import { Button, StatusMark, cx } from "../../../packages/console-components/src/panel-kit";

/** The lifecycle actions that ask before they act. */
export type ConfirmedLifecycleAction = "retire" | "reset";

/** What the confirmation says: the question names the agent, the
 * consequence says plainly what happens. */
export function lifecycleConfirmWords(action: ConfirmedLifecycleAction, name: string): { question: string; consequence: string } {
  if (action === "retire") {
    return {
      question: `Retire ${name}?`,
      consequence: "It stops and leaves the team. Its open work stops with it.",
    };
  }
  return {
    question: `Reset ${name}?`,
    consequence: "It starts over on a fresh session. Its current conversation ends and it keeps its place in the team.",
  };
}

const FOCUSABLE = "button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])";

/**
 * The confirmation a destructive lifecycle action asks for (Retire, Reset):
 * an inline alert dialog that names the agent in words and says what will
 * happen, with the action and Cancel. It takes focus on Cancel (the safe
 * choice), keeps Tab and Shift+Tab inside it, cancels on Escape or a click
 * outside, and gives focus back to the control that opened it.
 */
export function LifecycleConfirm({
  action,
  name,
  actionLabel,
  onConfirm,
  onCancel,
  busy,
  className,
  testId,
  confirmTestId,
  cancelTestId,
}: {
  action: ConfirmedLifecycleAction;
  /** The agent's display name. */
  name: string;
  /** The configured label of the action ("Retire"). */
  actionLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
  busy?: boolean;
  className?: string;
  testId?: string;
  confirmTestId?: string;
  cancelTestId?: string;
}): React.JSX.Element {
  const words = lifecycleConfirmWords(action, name);
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const cancelRef = React.useRef<HTMLButtonElement | null>(null);
  const opener = React.useRef<HTMLElement | null>(null);
  const questionId = React.useId();
  const consequenceId = React.useId();
  const cancelRefLatest = React.useRef(onCancel);
  cancelRefLatest.current = onCancel;

  React.useLayoutEffect(() => {
    const doc = rootRef.current?.ownerDocument;
    const active = doc?.activeElement as HTMLElement | null;
    opener.current = active && active !== doc?.body && !rootRef.current?.contains(active) ? active : null;
    cancelRef.current?.focus({ preventScroll: false });
    return () => {
      // Back to the control that asked, when it is still there.
      const back = opener.current;
      if (back?.isConnected) back.focus({ preventScroll: true });
    };
  }, []);

  React.useEffect(() => {
    const doc = rootRef.current?.ownerDocument;
    if (!doc) return undefined;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && rootRef.current?.contains(target)) return;
      cancelRefLatest.current();
    };
    doc.addEventListener("pointerdown", onPointerDown, true);
    return () => doc.removeEventListener("pointerdown", onPointerDown, true);
  }, []);

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    const items = Array.from(rootRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = rootRef.current?.ownerDocument.activeElement;
    if (event.shiftKey && (active === first || !rootRef.current?.contains(active ?? null))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !rootRef.current?.contains(active ?? null))) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      ref={rootRef}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={questionId}
      aria-describedby={consequenceId}
      className={cx("lifecycle-confirm", className)}
      data-action={action}
      data-testid={testId}
      onKeyDown={onKeyDown}
    >
      <p className="lifecycle-confirm__question" id={questionId}>
        <StatusMark tone="crit" />
        {words.question}
      </p>
      <p className="lifecycle-confirm__consequence" id={consequenceId}>
        {words.consequence}
      </p>
      <div className="cc-btn-row lifecycle-confirm__actions">
        <Button
          variant="danger"
          size="sm"
          disabled={busy}
          data-testid={confirmTestId}
          onClick={onConfirm}
        >
          {actionLabel || (action === "retire" ? "Retire" : "Reset")}
        </Button>
        <Button ref={cancelRef} size="sm" data-testid={cancelTestId} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
