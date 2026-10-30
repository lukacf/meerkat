import React from "react";
import type { RoutingSectionView } from "@console-core";
import {
  AgentName,
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  ListRow,
  LoadingState,
  PanelHeader,
  PanelSection,
  PanelShell,
  RelativeTime,
  RowList,
  SearchField,
  StatusDot,
  StatusRow,
  TechnicalDetails,
  TechnicalId,
  agentDisplayName,
  humanizeEnum,
  joinMeta,
  plural,
  statusTone,
  useAgentDirectory,
} from "../../../packages/console-components/src/panel-kit";

type Route = RoutingSectionView["routes"][number];
type Delivery = RoutingSectionView["deliveries"][number];

interface RoutingPanelProps {
  data: RoutingSectionView;
  /** True while the first read is in flight; the list shows a skeleton. */
  loading?: boolean;
  /** A host that reports refresh errors passes this (null when healthy) and
   * gets the reserved status row, so an error never moves the list. */
  error?: string | null;
}

/** Deliveries the panel considers: the most recent ones the history returned. */
const RECENT_DELIVERIES = 40;
/** Deliveries listed for the selected route. */
const ROUTE_DELIVERIES_SHOWN = 8;

/** When a delivery last happened: its final attempt, else its first. */
function deliveryTime(delivery: Delivery): number {
  return delivery.final_attempt_ms || delivery.first_attempt_ms || 0;
}

/** "500 ms" below a second, "2 s", "1 min" above. */
function formatBackoff(ms: number | undefined): string {
  if (!ms || !Number.isFinite(ms) || ms <= 0) return "";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${Math.round(ms / 100) / 10} s`;
  return `${Math.round(ms / 60_000)} min`;
}

/** Status counts for a route's recent deliveries, worst first, as chips. */
function deliveryCounts(deliveries: Delivery[]): Array<{ status: string; count: number }> {
  const counts = new Map<string, number>();
  for (const delivery of deliveries) {
    const status = delivery.status || "unknown";
    counts.set(status, (counts.get(status) ?? 0) + 1);
  }
  const rank = (status: string) => {
    const tone = statusTone(status);
    return tone === "crit" ? 0 : tone === "warn" ? 1 : tone === "info" ? 2 : tone === "ok" ? 4 : 3;
  };
  return [...counts.entries()]
    .map(([status, count]) => ({ status, count }))
    .sort((a, b) => rank(a.status) - rank(b.status) || b.count - a.count);
}

/** "3 delivered", "1 failed", "2 dead letter". */
function countLabel(status: string, count: number): string {
  return `${count} ${humanizeEnum(status).toLowerCase() || "unknown"}`;
}

/** "Email to Support Triage via console": who receives a route's messages. */
function RouteCaption({ route }: { route: Route }): React.JSX.Element {
  const directory = useAgentDirectory();
  const channel = humanizeEnum(route.channel) || "Messages";
  const name = agentDisplayName(route.recipient, directory) || route.recipient;
  return (
    <>
      {channel} to <span title={route.recipient}>{name}</span>
      {route.sink ? ` via ${route.sink}` : null}
    </>
  );
}

export function RoutingPanel({ data, loading = false, error }: RoutingPanelProps): React.JSX.Element {
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

  const recentDeliveries = React.useMemo(() => deliveries.slice(0, RECENT_DELIVERIES), [deliveries]);
  const deliveriesByRoute = React.useMemo(() => {
    const byRoute = new Map<string, Delivery[]>();
    for (const delivery of recentDeliveries) {
      const list = byRoute.get(delivery.route_id);
      if (list) list.push(delivery);
      else byRoute.set(delivery.route_id, [delivery]);
    }
    return byRoute;
  }, [recentDeliveries]);
  const activeDeliveries = active ? deliveriesByRoute.get(active.route_key) ?? [] : [];

  const subtitle = q
    ? joinMeta(`${rows.length} of ${plural(routes.length, "route")}`, plural(recentDeliveries.length, "recent delivery", "recent deliveries"))
    : joinMeta(plural(routes.length, "route"), plural(recentDeliveries.length, "recent delivery", "recent deliveries"));

  const list = loading && routes.length === 0 ? (
    <LoadingState label="Loading routes" rows={4} />
  ) : routes.length === 0 ? (
    <EmptyState
      title="No routes configured"
      body="Routes carry messages from a channel, such as email or a schedule, to an agent. They appear here once a module defines them."
    />
  ) : rows.length === 0 ? (
    <EmptyState
      kind="filtered"
      title={`No routes match "${q}"`}
      action={<Button size="sm" onClick={() => setQ("")}>Clear filter</Button>}
    />
  ) : (
    <RowList>
      {rows.map((route) => {
        const routeDeliveries = deliveriesByRoute.get(route.route_key) ?? [];
        const last = routeDeliveries.reduce((latest, delivery) => Math.max(latest, deliveryTime(delivery)), 0);
        return (
          <ListRow
            key={route.route_key}
            testId={`routing-route:${route.route_key}`}
            selected={Boolean(active && route.route_key === active.route_key)}
            onActivate={() => setSel(route.route_key)}
            title={route.route_key}
            titleAttr={route.route_key}
            subtitle={<RouteCaption route={route} />}
            meta={
              <>
                {deliveryCounts(routeDeliveries).map(({ status, count }) => (
                  <Chip key={status} tone={statusTone(status)}>{countLabel(status, count)}</Chip>
                ))}
                {last ? <RelativeTime value={last} /> : <span className="routing__quiet">No recent deliveries</span>}
              </>
            }
          />
        );
      })}
    </RowList>
  );

  const detail = active ? (
    <>
      <PanelSection label="Flow" padded>
        <ol className="routing__steps">
          <li className="routing__step">
            <span className="routing__step-label">Route</span>
            <span className="routing__step-value" title={active.route_key}>{active.route_key}</span>
          </li>
          <li className="routing__step">
            <span className="routing__step-label">Recipient</span>
            <span className="routing__step-value">
              <AgentName id={active.recipient} avatar={18} />
              {active.sink ? <span className="routing__step-note">via {active.sink}</span> : null}
            </span>
          </li>
          <li className="routing__step">
            <span className="routing__step-label">Module</span>
            <span className="routing__step-value" title={active.target_module}>{active.target_module}</span>
          </li>
        </ol>
      </PanelSection>
      <PanelSection label="Policy" padded>
        <KeyValueList
          items={[
            { label: "Channel", value: humanizeEnum(active.channel) },
            { label: "Retries", value: active.retry_max != null ? String(active.retry_max) : "" },
            { label: "Backoff", value: formatBackoff(active.backoff_ms) },
            { label: "Rate limit", value: active.rate_limit_per_minute ? `${active.rate_limit_per_minute} per minute` : "" },
          ]}
        />
        {active.retry_max == null && !active.backoff_ms && !active.rate_limit_per_minute ? (
          <p className="routing__quiet">Default delivery policy.</p>
        ) : null}
        <TechnicalDetails items={[{ label: "Recipient key", value: active.recipient }]} />
      </PanelSection>
      <PanelSection label="Recent deliveries" count={activeDeliveries.length || null}>
        {activeDeliveries.length === 0 ? (
          <EmptyState inline title="No recent deliveries for this route" />
        ) : (
          <RowList>
            {activeDeliveries.slice(0, ROUTE_DELIVERIES_SHOWN).map((delivery) => {
              const tone = statusTone(delivery.status);
              const attempts = delivery.attempts?.length ?? 0;
              return (
                <ListRow
                  key={delivery.delivery_id}
                  testId={`routing-delivery:${delivery.delivery_id}`}
                  dense
                  lead={<StatusDot tone={tone} />}
                  title={<AgentName id={delivery.recipient} />}
                  subtitle={
                    <span className="routing__delivery-sub">
                      <TechnicalId value={delivery.delivery_id} label="Delivery ID" />
                      {attempts > 1 ? <span className="routing__delivery-attempts">{plural(attempts, "attempt")}</span> : null}
                    </span>
                  }
                  meta={
                    <>
                      <Chip tone={tone}>{humanizeEnum(delivery.status) || "Unknown"}</Chip>
                      <RelativeTime value={deliveryTime(delivery)} />
                    </>
                  }
                />
              );
            })}
          </RowList>
        )}
      </PanelSection>
    </>
  ) : null;

  return (
    <PanelShell className="routing" testId="routing-panel">
      <PanelHeader
        title="Routing"
        subtitle={subtitle}
        actions={
          routes.length > 0 ? (
            <SearchField
              label="Filter routes"
              value={q}
              onChange={(value) => setQ(value)}
              placeholder="Route, recipient, sink or module"
            />
          ) : null
        }
      />
      {error !== undefined ? <StatusRow error={error} /> : null}
      <div className={active ? "routing__body" : "routing__body routing__body--list"}>
        <div className="routing__list">{list}</div>
        {active ? <aside className="routing__detail" aria-label={`Route ${active.route_key}`}>{detail}</aside> : null}
      </div>
    </PanelShell>
  );
}
