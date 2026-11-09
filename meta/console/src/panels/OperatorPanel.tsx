import React from "react";
import { operatorPanelPath } from "../lib/adapters";
import type { ConsoleSidebarButtonConfig } from "../types";
import { EmptyState, IconButton, KitGlyph, PanelHeader, PanelShell } from "../../../packages/console-components/src/panel-kit";

/**
 * A docked operator panel configured in console.toml. It is trusted operator
 * code, not an isolated frame: a same-origin page runs with the viewing
 * operator's console session and can act with that authority. The path is
 * read from the current static configuration on every render, never from a
 * saved layout.
 *
 * The kit header names the page by its button's label and offers Reload and
 * Open in new tab (spec B20); a button without a usable page shows the
 * not-configured state.
 */
export function OperatorPanel({ buttonId, buttons }: {
  buttonId: string;
  buttons: readonly ConsoleSidebarButtonConfig[] | undefined;
}) {
  const button = (buttons || []).find((candidate) => candidate.id === buttonId);
  const path = operatorPanelPath(button?.panel);
  const title = button?.label?.trim() || "Operator panel";
  // A reload remounts the frame on the same path.
  const [generation, setGeneration] = React.useState(0);
  if (!button || !path) {
    return (
      <PanelShell className="console-operator-panel" testId={`operator-panel-missing:${buttonId}`}>
        <PanelHeader title={title} summary={[{ text: "Operator panel", announce: false }]} />
        <EmptyState
          kind="not-configured"
          title="This panel's page is not configured"
          body="No page is set for this button in the console configuration. Set a same-origin path for it, and the page opens here."
        />
      </PanelShell>
    );
  }
  return (
    <PanelShell className="console-operator-panel" testId={`operator-panel:${buttonId}`}>
      <PanelHeader
        title={title}
        summary={[
          { text: "Host page", announce: false },
          { text: "uses your console sign-in", announce: false },
        ]}
        actions={(
          <>
            <IconButton
              label={`Reload ${title}`}
              glyph="refresh"
              quiet
              onClick={() => setGeneration((value) => value + 1)}
              data-testid={`operator-panel-reload:${buttonId}`}
            />
            <a
              className="cc-btn cc-btn--icon cc-btn--ghost cc-btn--quiet"
              href={path}
              target="_blank"
              rel="noopener"
              aria-label={`Open ${title} in a new tab`}
              title={`Open ${title} in a new tab`}
              data-testid={`operator-panel-open:${buttonId}`}
            >
              <span className="cc-btn__icon" aria-hidden="true"><KitGlyph name="open" /></span>
            </a>
          </>
        )}
      />
      {/* No sandbox: this is trusted same-origin code, and a host's form posts
          must keep their real Origin. If a sandbox is ever added it must
          include allow-same-origin and allow-forms, or same-origin form
          admission breaks. The console sets no frame-src restriction, so
          same-origin panel paths stay framable. */}
      <iframe key={generation} className="console-operator-panel__frame" src={path} title={button.label} />
    </PanelShell>
  );
}
