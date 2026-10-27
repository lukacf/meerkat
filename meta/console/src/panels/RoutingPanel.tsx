import React from "react";
import type { RoutingSectionView } from "@console-core";

interface RoutingPanelProps {
  data: RoutingSectionView;
}

export function RoutingPanel({ data }: RoutingPanelProps): React.JSX.Element {
  const routes = data.routes || [];
  const deliveries = data.deliveries || [];

  const [q, setQ] = React.useState("");
  const [sel, setSel] = React.useState<string>(routes[0]?.route_key || "");

  const rows = React.useMemo(() => {
    if (!q) return routes;
    const needle = q.toLowerCase();
    return routes.filter((r) =>
      r.route_key.toLowerCase().includes(needle) ||
      r.recipient.toLowerCase().includes(needle) ||
      r.sink.toLowerCase().includes(needle) ||
      r.target_module.toLowerCase().includes(needle),
    );
  }, [routes, q]);

  const active = rows.find((r) => r.route_key === sel) || rows[0];

  const recentDeliveries = deliveries.slice(0, 40);
  const trafficForRoute = (routeKey: string) =>
    deliveries.filter((d) => d.route_id === routeKey).length;

  return (
    <div className="view routing" data-testid="routing-panel">
      <div className="view__head">
        <h2>Routing</h2>
        <span className="view__sub">{rows.length} routes · {deliveries.length} deliveries (recent)</span>
        <span className="view__spacer" />
        <input
          className="view__search"
          placeholder="Filter route, recipient, sink…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
      </div>
      <div className="routing__body">
        <div className="routing__table">
          <div className="routing__row routing__row--head">
            <span>Route</span><span>Channel</span><span>Recipient</span><span>Sink</span><span>Module</span><span>24h</span>
          </div>
          {rows.map((r) => {
            const isSel = active && r.route_key === active.route_key;
            return (
              <div
                key={r.route_key}
                className={`routing__row ${isSel ? "is-selected" : ""}`}
                onClick={() => setSel(r.route_key)}
                data-testid={`routing-route:${r.route_key}`}
              >
                <span className="routing__intent mono">{r.route_key}</span>
                <span className="mono dim">{r.channel || "—"}</span>
                <span className="mono">{r.recipient}</span>
                <span className="dim">{r.sink}</span>
                <span className="mono dim">{r.target_module}</span>
                <span className="mono">{trafficForRoute(r.route_key)}</span>
              </div>
            );
          })}
          {rows.length === 0 && (
            <div style={{ padding: 24, color: "var(--ink-dim)", fontFamily: "var(--mono)", fontSize: 12 }}>
              No routes configured.
            </div>
          )}
        </div>
        <aside className="routing__flow">
          {active && (
            <>
              <div className="rf__title">Flow</div>
              <div className="rf__diagram">
                <div className="rf__node rf__node--intent">
                  <div className="rf__lbl">Route</div>
                  <div className="rf__val mono">{active.route_key}</div>
                </div>
                <svg className="rf__arrow" viewBox="0 0 40 12">
                  <path d="M0 6 H 34 M 28 2 L 34 6 L 28 10" stroke="currentColor" fill="none" strokeWidth="1" />
                </svg>
                <div className="rf__node rf__node--handler">
                  <div className="rf__lbl">via {active.sink}</div>
                  <div className="rf__val mono">{active.recipient}</div>
                </div>
                <svg className="rf__arrow rf__arrow--drop" viewBox="0 0 12 40">
                  <path d="M6 0 V 34 M 2 28 L 6 34 L 10 28" stroke="currentColor" fill="none" strokeWidth="1" />
                </svg>
                <div className="rf__node rf__node--gate">
                  <div className="rf__lbl">Module</div>
                  <div className="rf__val mono">{active.target_module}</div>
                </div>
              </div>
              <div className="rf__stats">
                <div><dt>Retry max</dt><dd>{active.retry_max ?? "—"}</dd></div>
                <div><dt>Backoff</dt><dd>{active.backoff_ms ? `${active.backoff_ms} ms` : "—"}</dd></div>
                <div><dt>Rate limit</dt><dd>{active.rate_limit_per_minute ? `${active.rate_limit_per_minute}/m` : "—"}</dd></div>
              </div>
              <div className="rf__title" style={{ marginTop: 12 }}>Recent deliveries</div>
              <div style={{ display: "flex", flexDirection: "column", gap: 4, fontFamily: "var(--mono)", fontSize: 11, color: "var(--ink-muted)" }}>
                {recentDeliveries.filter((d) => d.route_id === active.route_key).slice(0, 8).map((d) => (
                  <div key={d.delivery_id} data-testid={`routing-delivery:${d.delivery_id}`}>
                    <span style={{ color: d.status === "delivered" ? "var(--ok)" : d.status === "failed" ? "var(--crit)" : "var(--warn)" }}>
                      {d.status}
                    </span>{" "}
                    <span className="dim">· {d.delivery_id.slice(0, 8)}</span>{" "}
                    <span>→ {d.recipient}</span>
                  </div>
                ))}
                {recentDeliveries.filter((d) => d.route_id === active.route_key).length === 0 && (
                  <span className="dim">No recent deliveries.</span>
                )}
              </div>
            </>
          )}
        </aside>
      </div>
    </div>
  );
}
