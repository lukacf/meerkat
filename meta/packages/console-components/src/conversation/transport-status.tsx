import type { ConsoleTransportState } from "@console-core";

export function consoleTransportLabel(state: ConsoleTransportState): string {
  switch (state.phase) {
    case "connecting": return "Connecting";
    case "connected-replaying": return "Connected, checking history";
    case "live": return state.freshness === "current" ? "Live" : "Connected, freshness unknown";
    case "offline": return "Offline, showing saved history";
    case "retrying": return "Reconnecting, history may be out of date";
    case "authentication-required": return "Sign in to reconnect";
    case "forbidden": return "Timeline access denied";
    case "consumer-failed": return "Timeline update failed";
    case "stopped": return "Updates stopped, history may be out of date";
  }
}

/** Connection health is separate from the agent's authoritative run status. */
export function ConsoleTransportStatus({ state, onRetry }: {
  state: ConsoleTransportState;
  onRetry?: () => void;
}) {
  const retryable = state.phase === "retrying" || state.phase === "offline" || state.phase === "stopped";
  return (
    <div className="cc-transport-status" data-testid="console-transport-status" data-phase={state.phase} role="status">
      <span className="cc-transport-status__dot" aria-hidden="true" />
      <span className="cc-transport-status__label">{consoleTransportLabel(state)}</span>
      {retryable && onRetry ? <button type="button" onClick={onRetry}>Reconnect</button> : null}
    </div>
  );
}
