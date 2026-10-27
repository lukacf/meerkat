import React from "react";

/// One reserved line for a panel's refresh error. Stream events start
/// refreshes at any moment, and a failed refresh sets the error while the next
/// good one clears it. The row always renders, one line high across the
/// panel's width, so neither moves the controls above or below it under a
/// click in progress. A long error is ellipsized, with the full text in its
/// title.
export function PanelStatusRow({
  error,
  testId,
}: {
  error?: string | null;
  testId?: string;
}): React.JSX.Element {
  return (
    <div className="panel-status-row">
      {error ? (
        <span className="panel-status-row__error" role="alert" title={error} data-testid={testId}>
          {error}
        </span>
      ) : null}
    </div>
  );
}
