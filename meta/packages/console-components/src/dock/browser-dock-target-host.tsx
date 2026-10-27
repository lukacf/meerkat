import React, { type ComponentPropsWithoutRef, type ReactNode } from "react";

import type { BrowserDockTarget } from "@console-core";

export type BrowserDockTargetHostProps = {
  target: BrowserDockTarget;
  children: ReactNode;
  className?: string;
} & Omit<ComponentPropsWithoutRef<"div">, "children" | "className">;

/**
 * Placement-neutral boundary for browser content owned by the consuming host.
 *
 * This component deliberately owns no page, lifecycle, session, lease, or
 * runtime state. The host supplies the trusted chrome and viewport marker as
 * children and remains responsible for all browser behavior.
 */
export function BrowserDockTargetHost({
  target,
  children,
  className,
  ...hostProps
}: BrowserDockTargetHostProps) {
  return (
    <div
      {...hostProps}
      className={className}
      data-browser-dock-target-id={target.id}
      data-browser-panel-id={target.browserPanelId}
    >
      {children}
    </div>
  );
}
