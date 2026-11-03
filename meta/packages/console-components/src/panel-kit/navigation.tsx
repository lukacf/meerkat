import { createContext, useContext, type MouseEvent, type ReactNode } from "react";

import { cx } from "./cx";

/**
 * Cross-panel navigation (spec A4.7): the host (ConsoleApp) provides these,
 * and every entity name becomes a link to its entity. Every member is
 * optional; a part renders plain text for an entity the host cannot open.
 */
export interface ConsoleNavigation {
  /** "details" opens the agent's details beside the current pane; "chat"
   * opens its chat (a Ctrl or Cmd click). */
  openAgent?(identity: string, how?: "details" | "chat"): void;
  openWorkItem?(id: string): void;
  openMemoryRecord?(realm: string, id: string): void;
  openApproval?(id: string): void;
  openRoute?(key: string): void;
  openPanel?(kind: string): void;
}

const ConsoleNavigationContext = createContext<ConsoleNavigation | null>(null);

export function ConsoleNavigationProvider({ value, children }: { value: ConsoleNavigation | null; children?: ReactNode }) {
  return <ConsoleNavigationContext.Provider value={value}>{children}</ConsoleNavigationContext.Provider>;
}

/** The host's navigation, or null without a provider (tests, host shells). */
export function useConsoleNavigation(): ConsoleNavigation | null {
  return useContext(ConsoleNavigationContext);
}

export type EntityLinkKind = "work-item" | "memory-record" | "approval" | "route" | "panel";

/** Whether a click asks for the other way of opening (Ctrl or Cmd). */
export function wantsAlternate(event: MouseEvent): boolean {
  return event.ctrlKey || event.metaKey;
}

/**
 * The name of a work item, memory record, approval, route or panel as a link
 * to it (A4.7): a quiet text button when the host can open that kind, the
 * plain text otherwise. Put it where a control may stand (not inside
 * another button).
 */
export function EntityLink({
  kind,
  id,
  realm,
  title,
  className,
  testId,
  children,
}: {
  kind: EntityLinkKind;
  id: string;
  /** The memory realm (memory records). */
  realm?: string;
  title?: string;
  className?: string;
  testId?: string;
  children?: ReactNode;
}) {
  const navigation = useConsoleNavigation();
  const open: (() => void) | null = !navigation
    ? null
    : kind === "work-item" && navigation.openWorkItem
      ? () => navigation.openWorkItem?.(id)
      : kind === "memory-record" && navigation.openMemoryRecord && realm
        ? () => navigation.openMemoryRecord?.(realm, id)
        : kind === "approval" && navigation.openApproval
          ? () => navigation.openApproval?.(id)
          : kind === "route" && navigation.openRoute
            ? () => navigation.openRoute?.(id)
            : kind === "panel" && navigation.openPanel
              ? () => navigation.openPanel?.(id)
              : null;
  if (!open) {
    return (
      <span className={cx("cc-link", className)} title={title} data-testid={testId}>
        {children}
      </span>
    );
  }
  return (
    <button type="button" className={cx("cc-link", "cc-link--button", className)} title={title} data-testid={testId} onClick={open}>
      {children}
    </button>
  );
}
