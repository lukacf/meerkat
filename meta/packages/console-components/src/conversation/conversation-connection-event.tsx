import type { CSSProperties } from "react";

import type { ConversationConnectionEvent } from "@console-core";

function peerColor(id: string): string {
  let hash = 0;
  for (const character of id) {
    hash = ((hash << 5) - hash + character.codePointAt(0)!) | 0;
  }
  return `hsl(${Math.abs(hash) % 330} 66% 64%)`;
}

function eventSummary(event: ConversationConnectionEvent): string {
  const count = event.peers.length;
  const one = count === 1;
  const noun = one ? event.peers[0]?.label || "one endpoint" : `${count} endpoints`;
  if (event.action === "connected") return `Connected to ${noun}.`;
  if (event.action === "reconnected") return `Reconnected to ${noun}.`;
  return `Disconnected from ${noun}.`;
}

export function ConversationConnectionEventView({ event }: { event: ConversationConnectionEvent }) {
  const disconnected = event.action === "disconnected";
  const crossScopeCount = event.peers.filter((peer) => peer.crossScope).length;
  return (
    <article
      aria-label={eventSummary(event)}
      className={`cc-connection-event is-${event.action}`}
      data-connection-action={event.action}
    >
      <p className="cc-connection-event__summary">
        {eventSummary(event)}
        {event.status && event.status !== "succeeded" ? (
          <span className="cc-connection-event__status"> · {event.status}</span>
        ) : null}
        {crossScopeCount > 0 ? (
          <span className="cc-connection-event__cross-count">
            {` · ${crossScopeCount} cross-scope`}
          </span>
        ) : null}
        {event.message ? <span className="cc-connection-event__message"> · {event.message}</span> : null}
      </p>
      <div className="cc-connection-event__peers">
        {event.peers.map((peer) => (
          <span
            className={`cc-connection-event__peer${peer.crossScope ? " is-cross-scope" : ""}`}
            key={peer.id}
            style={{ "--cc-connection-color": peerColor(peer.id) } as CSSProperties}
          >
            <span aria-hidden="true" className="cc-connection-event__dot" />
            <span className={disconnected ? "cc-connection-event__struck" : undefined}>{peer.label}</span>
            {peer.scopeLabel ? (
              <span className="cc-connection-event__scope">· {peer.scopeLabel}</span>
            ) : null}
            {!disconnected ? <span aria-hidden="true" className="cc-connection-event__check">✓</span> : null}
          </span>
        ))}
      </div>
    </article>
  );
}
