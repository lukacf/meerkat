import React from "react";
import { operatorPanelPath } from "../lib/adapters";
import type { ConsoleSidebarButtonConfig } from "../types";

/**
 * A docked operator panel configured in console.toml. It is trusted operator
 * code, not an isolated frame: a same-origin page runs with the viewing
 * operator's console session and can act with that authority. The path is
 * read from the current static configuration on every render, never from a
 * saved layout.
 */
export function OperatorPanel({ buttonId, buttons }: {
  buttonId: string;
  buttons: readonly ConsoleSidebarButtonConfig[] | undefined;
}) {
  const button = (buttons || []).find((candidate) => candidate.id === buttonId);
  const path = operatorPanelPath(button?.panel);
  if (!button || !path) {
    return <div className="console-panel" data-testid={`operator-panel-missing:${buttonId}`}>This operator panel is not configured.</div>;
  }
  return (
    <div className="console-panel console-operator-panel" data-testid={`operator-panel:${buttonId}`}>
      {/* No sandbox: this is trusted same-origin code, and a host's form posts
          must keep their real Origin. If a sandbox is ever added it must
          include allow-same-origin and allow-forms, or same-origin form
          admission breaks. The console sets no frame-src restriction, so
          same-origin panel paths stay framable. */}
      <iframe className="console-operator-panel__frame" src={path} title={button.label} />
    </div>
  );
}
