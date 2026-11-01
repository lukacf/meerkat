import React from "react";
import type { RoutingSectionView } from "@console-core";
import {
  AgentAvatar,
  AgentName,
  Button,
  DetailDrawer,
  DotStrip,
  DrawerLayout,
  DrawerSection,
  EmptyState,
  FilterBar,
  IconButton,
  KeyValueList,
  KitGlyph,
  LoadingState,
  PanelHeader,
  PanelShell,
  RelativeTime,
  StatusMark,
  StatusPill,
  StatusRow,
  agentDisplayName,
  cx,
  formatAbsoluteTime,
  joinMeta,
  plural,
  standardPanelMenu,
  useAgentDirectory,
  usePanelDensity,
  useShowTechnicalIds,
  type DotStripItem,
  type SummaryFact,
} from "../../../packages/console-components/src/panel-kit";
import { useNavigationFocus } from "../lib/console-navigation";
import {
  STRIP_DELIVERIES,
  buildRoutingModel,
  failureSentence,
  runtimeOffsetWords,
  type ChannelGroup,
  type DeliveryView,
  type RecipientKind,
  type RouteView,
  type RoutingModel,
} from "./routing-model";

export { deliverySummaries } from "./routing-model";

interface RoutingPanelProps {
  data: RoutingSectionView;
  /** True while a read is in flight; the first one shows a skeleton. */
  loading?: boolean;
  /** Why the last read failed; null or absent when it did not. */
  error?: string | null;
  /** The reads were refused: the viewer lacks routing access. */
  denied?: boolean;
  /** This runtime offers no routing reads. */
  unavailable?: boolean;
  /** When the shown data was read. */
  updatedAtMs?: number | null;
  /** What each delivery sent, in words, by delivery id. */
  deliverySummaries?: Readonly<Record<string, string>>;
  onRefresh?: () => void;
}

/** "500 ms" below a second, "2 s", "1 min" above. */
function formatBackoff(ms: number | undefined): string {
  if (ms === undefined || ms === null || !Number.isFinite(ms) || ms < 0) return "";
  if (ms === 0) return "None";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${Math.round(ms / 100) / 10} s`;
  return `${Math.round(ms / 60_000)} min`;
}

function retriesWords(retries: number | undefined): string {
  if (retries === undefined || retries === null || !Number.isFinite(retries)) return "";
  if (retries <= 0) return "None";
  return plural(retries, "retry", "retries");
}

/** A route's channel, sink and module keys for a tooltip: how it delivers,
 * in the router's own terms. */
function routeKeysTitle(route: Pick<RouteView["route"], "channel" | "sink" | "target_module">): string {
  return joinMeta(
    route.channel ? `Channel: ${route.channel}` : null,
    `Sink: ${route.sink}`,
    `Module: ${route.target_module}`,
  );
}

/** When a delivery happened, in the words its clock allows: "4m ago" for a
 * wall-clock time, "12 min after start" for a runtime-relative one. */
function DeliveryTime({ view, title }: { view: DeliveryView; title?: string }): React.JSX.Element | null {
  if (!view.at) return null;
  if (view.clock === "epoch") return <RelativeTime value={view.at} title={title} className="routing-flow__time" />;
  return (
    <span className="cc-time routing-flow__time" title={title ?? "Time since the runtime started"}>
      {runtimeOffsetWords(view.at)}
    </span>
  );
}

function deliveryTimeWords(view: DeliveryView): string {
  if (!view.at) return "";
  return view.clock === "epoch" ? formatAbsoluteTime(view.at) : runtimeOffsetWords(view.at);
}

/** The source mark of the route grammar: a hollow square, lit while its
 * channel delivered in the last two minutes. */
function ChannelMark({ active }: { active?: boolean }): React.JSX.Element {
  return (
    <svg className={cx("routing-flow__channel-mark", active && "is-active")} viewBox="0 0 12 12" width="12" height="12" aria-hidden="true" focusable="false">
      <rect x="1.6" y="1.6" width="8.8" height="8.8" rx="1" />
    </svg>
  );
}

/** A recipient that is not an agent: a person (ring and dot) for an address,
 * another system (dashed ring) for a web address, a plain dot otherwise. */
function RecipientMark({ kind }: { kind: RecipientKind }): React.JSX.Element {
  if (kind === "email" || kind === "phone") {
    return (
      <svg className="routing-flow__recipient-mark" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false">
        <circle cx="8" cy="8" r="5.5" className="routing-flow__mark-ring" />
        <circle cx="8" cy="8" r="2" className="routing-flow__mark-dot" />
      </svg>
    );
  }
  if (kind === "url") {
    return (
      <svg className="routing-flow__recipient-mark" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false">
        <circle cx="8" cy="8" r="6.5" className="routing-flow__mark-ring routing-flow__mark-ring--dashed" />
        <circle cx="8" cy="8" r="3.5" className="routing-flow__mark-dot" />
      </svg>
    );
  }
  return (
    <svg className="routing-flow__recipient-mark" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false">
      <circle cx="8" cy="8" r="4" className="routing-flow__mark-dot" />
    </svg>
  );
}

/** The recipient by name: an agent as its name (a link to it), anyone else
 * as the address itself, never re-cased into words. `prefix` is read by
 * assistive tech before the name ("Delivers to "); the edge says it on
 * screen. */
function RecipientName({ view, link, prefix }: { view: RouteView; link?: boolean; prefix?: string }): React.JSX.Element {
  const nameOf = useRecipientName();
  const recipient = view.route.recipient;
  const hidden = prefix ? <span className="cc-visually-hidden">{prefix}</span> : null;
  if (view.recipientKind === "agent") {
    return (
      <span className="routing-flow__agent-wrap">
        <AgentAvatar name={nameOf(view)} size={22} />
        {hidden}
        <AgentName id={recipient} link={link} className="routing-flow__agent" />
      </span>
    );
  }
  return (
    <span className="routing-flow__address" title={recipient}>
      <RecipientMark kind={view.recipientKind} />
      {hidden}
      <span className="routing-flow__address-text">{recipient}</span>
    </span>
  );
}

function useRecipientName(): (view: RouteView) => string {
  const directory = useAgentDirectory();
  return React.useCallback(
    (view: RouteView) => (view.recipientKind === "agent" ? agentDisplayName(view.route.recipient, directory) || view.route.recipient : view.route.recipient),
    [directory],
  );
}

/** A dashed route edge (4 3) with its chevron at the recipient, and the
 * crit diamond at its middle while its deliveries fail. */
function Connector({ failing, chevron, className }: { failing?: boolean; chevron?: boolean; className?: string }): React.JSX.Element {
  return (
    <span className={cx("routing-flow__edge", failing && "is-failing", className)} aria-hidden="true">
      <span className="routing-flow__edge-line" />
      {failing ? <StatusMark tone="crit" className="routing-flow__edge-mark" /> : null}
      {chevron ? (
        <svg className="routing-flow__edge-head" viewBox="0 0 8 10" width="8" height="10" focusable="false">
          <path d="M1.5 1.5 6 5 1.5 8.5" />
        </svg>
      ) : null}
    </span>
  );
}

/** A route's newest deliveries as marks, one slot each, right-aligned so
 * the newest delivery of every route sits at the same edge. */
function stripItems(deliveries: readonly DeliveryView[], recipientName: string): DotStripItem[] {
  const shown = deliveries.slice(0, STRIP_DELIVERIES);
  return shown.map((delivery, index) => {
    const what = delivery.summary ? `${delivery.summary}: ` : "";
    const when = delivery.at ? `, ${deliveryTimeWords(delivery)}` : "";
    const label = delivery.outcome === "failed"
      ? `${what}${failureSentence(delivery, recipientName).replace(/\.$/, "")}${when}`
      : `${what}${delivery.word} to ${recipientName}${when}`;
    return {
      id: delivery.id,
      // Slot 15 is the newest delivery, slot 14 the one before it, and so on.
      t: STRIP_DELIVERIES - 1 - index,
      shape: delivery.outcome === "failed" ? "diamond" : delivery.outcome === "delivered" ? "dot" : "hollow",
      tone: delivery.tone === "live" ? undefined : delivery.tone,
      live: delivery.tone === "live",
      label,
    };
  });
}

const STRIP_WIDTH = 128;

function DeliveryStrip({
  deliveries,
  recipientName,
  testId,
  testIdPrefix,
  onSelect,
  matched,
}: {
  deliveries: readonly DeliveryView[];
  recipientName: string;
  testId: string;
  testIdPrefix?: string;
  onSelect?: () => void;
  /** The deliveries were matched by recipient, not by the route's key. */
  matched?: boolean;
}): React.JSX.Element {
  if (!deliveries.length) {
    return <span className="routing-flow__dots routing-flow__dots--empty" data-testid={testId}>No deliveries yet</span>;
  }
  const shown = Math.min(deliveries.length, STRIP_DELIVERIES);
  const delivered = deliveries.slice(0, shown).filter((delivery) => delivery.outcome === "delivered").length;
  const failed = deliveries.slice(0, shown).filter((delivery) => delivery.outcome === "failed").length;
  const label = joinMeta(
    `The ${plural(shown, "newest delivery", "newest deliveries")} to ${recipientName}, oldest first`,
    delivered ? `${delivered} delivered` : null,
    failed ? `${failed} failed` : null,
    matched ? "matched by recipient" : null,
  );
  return (
    <DotStrip
      className="routing-flow__dots"
      testId={testId}
      testIdPrefix={testIdPrefix}
      items={stripItems(deliveries, recipientName)}
      start={-0.5}
      end={STRIP_DELIVERIES - 0.5}
      width={STRIP_WIDTH}
      height={16}
      markSize={6}
      label={label}
      onSelect={onSelect ? () => onSelect() : undefined}
    />
  );
}

function RouteStatus({ view }: { view: RouteView }): React.JSX.Element {
  return (
    <span className="routing-flow__status" data-tone={view.tone}>
      <span className="routing-flow__status-words">
        {view.total ? <StatusMark tone={view.tone} /> : null}
        {view.statusWords}
      </span>
      {view.last ? <DeliveryTime view={view.last} /> : null}
    </span>
  );
}

function RouteRow({
  view,
  selected,
  onSelect,
  continued,
}: {
  view: RouteView;
  selected: boolean;
  onSelect: (key: string) => void;
  /** Not the first route of its channel: the channel is drawn once. */
  continued?: boolean;
}): React.JSX.Element {
  const nameOf = useRecipientName();
  const recipientName = nameOf(view);
  const failing = view.failed > 0;
  return (
    <div
      role="listitem"
      className={cx("routing-flow__row", "cc-reveal-host", selected && "is-selected", failing && "is-failing", continued && "is-continued")}
      data-testid={`routing-route:${view.key}`}
      data-route-key={view.key}
      data-state={failing ? "failing" : view.total ? "delivering" : "quiet"}
      onClick={(event) => {
        const target = event.target as Element | null;
        if (target?.closest?.("a, button, input, select, textarea, summary, [role='button']")) return;
        onSelect(view.key);
      }}
    >
      <Connector className="routing-flow__edge--in" failing={false} />
      <span className="routing-flow__route">
        <span className="cc-visually-hidden">{`From ${view.channelLabel}: `}</span>
        <button
          type="button"
          className="routing-flow__route-button"
          aria-current={selected ? "true" : undefined}
          aria-expanded={selected}
          title={routeKeysTitle(view.route)}
          onClick={() => onSelect(view.key)}
        >
          {view.key}
        </button>
      </span>
      <Connector className="routing-flow__edge--out" failing={failing} chevron />
      <span className="routing-flow__recipient">
        <RecipientName view={view} link prefix="Delivers to " />
      </span>
      <DeliveryStrip
        deliveries={view.deliveries}
        recipientName={recipientName}
        testId={`routing-dots:${view.key}`}
        testIdPrefix="routing-delivery"
        matched={view.match === "recipient"}
        onSelect={() => onSelect(view.key)}
      />
      <RouteStatus view={view} />
    </div>
  );
}

/** One channel's routes. A channel with many routes (one per agent) folds
 * into one row: its routes and recipients counted, the deliveries summed. */
function ChannelBlock({
  group,
  selectedKey,
  onSelect,
  expanded,
  canFold,
  onToggle,
  live,
}: {
  group: ChannelGroup;
  selectedKey: string | null;
  onSelect: (key: string) => void;
  expanded: boolean;
  /** The fold control shows (not while a filter opens the channel). */
  canFold: boolean;
  onToggle: () => void;
  live: boolean;
}): React.JSX.Element {
  const nameOf = useRecipientName();
  const folded = group.foldable && !expanded;
  const agents = group.routes.filter((view) => view.recipientKind === "agent");
  // A delivery matched by recipient can belong to two routes of a channel:
  // the folded row counts it once.
  const seen = new Set<string>();
  const deliveries: DeliveryView[] = [];
  for (const view of group.routes) {
    for (const delivery of view.deliveries) {
      if (seen.has(delivery.id)) continue;
      seen.add(delivery.id);
      deliveries.push(delivery);
    }
  }
  deliveries.sort((a, b) => b.at - a.at);
  const failed = deliveries.filter((delivery) => delivery.outcome === "failed").length;
  const total = deliveries.length;
  const recipientsWords = agents.length === group.routes.length
    ? `${plural(agents.length, "agent")}, one route each`
    : plural(group.routes.length, "recipient");
  const foldedView: RouteView | null = folded ? {
    ...group.routes[0],
    key: `fold:${group.channel}`,
    deliveries,
    total,
    failed,
    delivered: deliveries.filter((delivery) => delivery.outcome === "delivered").length,
    last: deliveries[0] ?? null,
  } : null;
  return (
    <div
      className={cx("routing-flow__group", folded && "is-folded", group.routes.length > 1 && !folded && "is-fan")}
      data-testid={`routing-channel:${group.channel}`}
    >
      <div className="routing-flow__channel" aria-hidden="true">
        <ChannelMark active={live} />
        <span className="routing-flow__channel-label">{group.label}</span>
        <span className="routing-flow__channel-edge" />
      </div>
      <div className="routing-flow__routes" role="list" aria-label={`${group.label}: ${plural(group.routes.length, "route")}`}>
        {foldedView ? (
          <div role="listitem" className={cx("routing-flow__row", "routing-flow__row--folded", failed > 0 && "is-failing")} data-testid={`routing-fold:${group.channel}`}>
            <Connector className="routing-flow__edge--in" />
            <span className="routing-flow__route">
              <span className="cc-visually-hidden">{`From ${group.label}: `}</span>
              <button
                type="button"
                className="routing-flow__route-button routing-flow__fold-button"
                aria-expanded={false}
                aria-label={`Show the ${plural(group.routes.length, "route")} of ${group.label}`}
                onClick={onToggle}
              >
                <KitGlyph name="expand" className="routing-flow__fold-glyph" />
                {plural(group.routes.length, "route")}
              </button>
            </span>
            <Connector className="routing-flow__edge--out" failing={failed > 0} chevron />
            <span className="routing-flow__recipient routing-flow__recipient--many">
              <span className="cc-visually-hidden">Delivers to </span>
              <span className="routing-flow__avatars" aria-hidden="true">
                {agents.slice(0, 4).map((view) => (
                  <AgentAvatar key={view.key} name={nameOf(view)} size={18} />
                ))}
              </span>
              <span className="routing-flow__many-words">{recipientsWords}</span>
            </span>
            <DeliveryStrip
              deliveries={deliveries}
              recipientName={recipientsWords}
              testId={`routing-dots:${foldedView.key}`}
            />
            <RouteStatus view={{ ...foldedView, tone: failed ? "crit" : total ? "ok" : "neutral", statusWords: failed ? `${failed} of ${total} failed` : total ? `${total} delivered` : "No deliveries yet" }} />
          </div>
        ) : (
          group.routes.map((view, index) => (
            <RouteRow
              key={view.key}
              view={view}
              selected={view.key === selectedKey}
              onSelect={onSelect}
              continued={index > 0}
            />
          ))
        )}
      </div>
      {group.foldable && expanded && canFold ? (
        <div className="routing-flow__fold-row">
          <Button size="sm" variant="ghost" aria-expanded onClick={onToggle} icon={<KitGlyph name="collapse" />}>
            {`Fold the ${plural(group.routes.length, "route")} of ${group.label}`}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function RouteDrawer({
  view,
  model,
  onClose,
}: {
  view: RouteView;
  model: RoutingModel;
  onClose: () => void;
}): React.JSX.Element {
  const nameOf = useRecipientName();
  const recipientName = nameOf(view);
  const route = view.route;
  const policy = [
    { key: "retries", label: "Retries", value: retriesWords(route.retry_max) },
    { key: "backoff", label: "Backoff", value: formatBackoff(route.backoff_ms) },
    { key: "rate", label: "Rate limit", value: route.rate_limit_per_minute ? `${route.rate_limit_per_minute} per minute` : "" },
  ].filter((item) => item.value);
  return (
    <DetailDrawer
      className="routing__detail"
      testId="routing-drawer"
      closeTestId="routing-drawer-close"
      title={view.key}
      mark={<KitGlyph name="route" className="routing-drawer__mark" />}
      status={<StatusPill tone={view.tone}>{view.statusWords}</StatusPill>}
      subtitle={`${view.channelLabel} to ${recipientName}`}
      ariaLabel={`Route ${view.key}`}
      onClose={onClose}
      technical={[
        { label: "Route key", value: route.route_key },
        { label: "Recipient key", value: route.recipient },
        { label: "Channel", value: route.channel ?? "" },
        { label: "Sink", value: route.sink },
        { label: "Module", value: route.target_module },
      ]}
    >
      <DrawerSection label="Flow">
        <ol className="routing__steps">
          <li className="routing__step">
            <span className="routing__step-label">From</span>
            <span className="routing__step-value">
              <ChannelMark />
              {view.channelLabel}
            </span>
          </li>
          <li className="routing__step">
            <span className="routing__step-label">Route</span>
            <span className="routing__step-value" title={route.route_key}>{route.route_key}</span>
          </li>
          <li className="routing__step">
            <span className="routing__step-label">To</span>
            <span className="routing__step-value">
              <RecipientName view={view} link />
            </span>
          </li>
        </ol>
      </DrawerSection>
      <DrawerSection label="Policy">
        {policy.length ? <KeyValueList items={policy} /> : null}
        {!policy.length ? <p className="routing__quiet">Default delivery policy.</p> : null}
      </DrawerSection>
      <DrawerSection label="Recent deliveries" count={view.total || null}>
        {view.match === "recipient" ? (
          <p className="routing__note" data-testid="routing-drawer-matched">
            Matched by recipient: deliveries don't name their route yet, so this lists every recent delivery to {recipientName}
            {view.sharedWith.length ? `, which ${plural(view.sharedWith.length, "other route")} also reaches` : ""}.
          </p>
        ) : null}
        {view.total === 0 ? (
          <EmptyState inline title="No recent deliveries for this route" />
        ) : (
          <ul className="routing__deliveries" aria-label={`Recent deliveries to ${recipientName}`}>
            {view.deliveries.slice(0, 12).map((delivery) => (
              <DeliveryRow key={delivery.id} delivery={delivery} recipientName={recipientName} />
            ))}
          </ul>
        )}
        {view.total > 12 ? <p className="routing__quiet">{`Showing the newest 12 of ${view.total}.`}</p> : null}
      </DrawerSection>
      {model.window && model.window.clock === "runtime" ? (
        <p className="routing__quiet routing__clock-note">Delivery times count from when the runtime started.</p>
      ) : null}
    </DetailDrawer>
  );
}

function DeliveryRow({ delivery, recipientName }: { delivery: DeliveryView; recipientName: string }): React.JSX.Element {
  const failed = delivery.outcome === "failed";
  const hover = joinMeta(deliveryTimeWords(delivery), `Delivery ${delivery.id}`);
  return (
    <li
      className={cx("routing__delivery", failed && "is-failed")}
      data-testid={`routing-drawer-delivery:${delivery.id}`}
      data-outcome={delivery.outcome}
    >
      <StatusMark tone={delivery.tone} className="routing__delivery-mark" />
      <span className="routing__delivery-main">
        <span className="routing__delivery-title">{delivery.summary || `To ${recipientName}`}</span>
        {failed ? <span className="routing__delivery-why">{failureSentence(delivery, recipientName)}</span> : null}
      </span>
      <span className="routing__delivery-meta">
        <StatusPill tone={delivery.tone} variant="quiet" title={`Delivery ${delivery.id}`}>{delivery.word}</StatusPill>
        <DeliveryTime view={delivery} title={hover} />
      </span>
    </li>
  );
}

function UnmatchedBlock({ deliveries }: { deliveries: DeliveryView[] }): React.JSX.Element | null {
  if (!deliveries.length) return null;
  const failed = deliveries.filter((delivery) => delivery.outcome === "failed").length;
  return (
    <div className="routing-flow__unmatched" data-testid="routing-unmatched">
      <span className="routing-flow__unmatched-words">
        {failed ? <StatusMark tone="crit" /> : null}
        {`${plural(deliveries.length, "recent delivery", "recent deliveries")} match no route here`}
        {failed ? ` (${failed} failed)` : ""}
      </span>
    </div>
  );
}

export function RoutingPanel({
  data,
  loading = false,
  error,
  denied = false,
  unavailable = false,
  updatedAtMs,
  deliverySummaries,
  onRefresh,
}: RoutingPanelProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const [density, setDensity] = usePanelDensity("routing");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("routing");
  const [query, setQuery] = React.useState("");
  const [failingOnly, setFailingOnly] = React.useState(false);
  const [selectedKey, setSelectedKey] = React.useState<string | null>(null);
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(() => new Set());
  const flowRef = React.useRef<HTMLDivElement | null>(null);

  const model = React.useMemo(
    () => buildRoutingModel(data, { isAgent: (key) => Boolean(directory.lookup(key)), summaries: deliverySummaries }),
    [data, directory, deliverySummaries],
  );
  const nameOf = React.useCallback(
    (view: RouteView) => (view.recipientKind === "agent" ? agentDisplayName(view.route.recipient, directory) || view.route.recipient : view.route.recipient),
    [directory],
  );

  const needle = query.trim().toLowerCase();
  const matches = React.useCallback((view: RouteView) => {
    if (failingOnly && view.failed === 0) return false;
    if (!needle) return true;
    return [view.key, view.route.recipient, nameOf(view), view.channelLabel, view.route.sink, view.route.target_module]
      .some((value) => value.toLowerCase().includes(needle));
  }, [failingOnly, needle, nameOf]);
  const groups = React.useMemo(
    () => model.groups
      .map((group) => ({ ...group, routes: group.routes.filter(matches) }))
      .filter((group) => group.routes.length > 0),
    [model.groups, matches],
  );
  const shownRoutes = groups.reduce((sum, group) => sum + group.routes.length, 0);
  const filtering = Boolean(needle) || failingOnly;
  const failingRoutes = model.routes.filter((view) => view.failed > 0).length;

  const active = selectedKey ? model.routes.find((view) => view.key === selectedKey) ?? null : null;
  const select = React.useCallback((key: string | null) => {
    setSelectedKey(key);
    if (!key) return;
    const group = model.groups.find((candidate) => candidate.routes.some((view) => view.key === key));
    if (group?.foldable) setExpanded((current) => (current.has(group.channel) ? current : new Set([...current, group.channel])));
  }, [model.groups]);

  // Another panel asked to show a route (a name linked to it).
  useNavigationFocus("route", (key) => {
    select(key);
    requestAnimationFrame(() => {
      Array.from(flowRef.current?.querySelectorAll<HTMLElement>("[data-route-key]") ?? [])
        .find((row) => row.dataset.routeKey === key)
        ?.scrollIntoView?.({ block: "nearest" });
    });
  });

  const hasData = model.routes.length > 0 || model.deliveries.length > 0;
  const newest = model.deliveries[0];
  const liveChannels = React.useMemo(() => {
    const live = new Set<string>();
    if (!newest || newest.clock !== "epoch") return live;
    const now = Date.now();
    for (const view of model.routes) {
      if (view.last && view.last.clock === "epoch" && now - view.last.at < 120_000) live.add(view.channel || "*");
    }
    return live;
  }, [model.routes, newest]);

  let summary: SummaryFact[];
  if (denied) summary = [{ key: "denied", text: "No access to routing", tone: "locked" }];
  else if (unavailable) summary = [{ key: "unavailable", text: "Not available on this runtime", tone: "locked" }];
  else if (loading && !hasData) summary = [{ key: "loading", text: "Loading routes", announce: false }];
  else {
    summary = [
      {
        key: "routes",
        text: filtering ? `${shownRoutes} of ${plural(model.totals.routes, "route")}` : plural(model.totals.routes, "route"),
      },
      { key: "deliveries", text: plural(model.totals.deliveries, "recent delivery", "recent deliveries") },
    ];
    if (model.totals.failed > 0) summary.push({ key: "failed", text: `${model.totals.failed} failed`, tone: "crit", testId: "routing-summary-failed" });
  }

  let body: React.ReactNode;
  if (denied) {
    body = (
      <EmptyState
        kind="no-access"
        illustration="no-access"
        role="status"
        testId="routing-no-access"
        title="Routing needs access to manage routing and wiring"
        body="Routes and deliveries are shown to people who may manage routing and wiring. An admin can grant it in Console access."
        details={[{ label: "Missing permission", value: "runtime.admin" }]}
      />
    );
  } else if (unavailable) {
    body = (
      <EmptyState
        kind="not-configured"
        illustration="not-configured"
        role="status"
        testId="routing-unavailable"
        title="Routing isn't available on this runtime"
        body="This runtime offers no routes or delivery history, so there is nothing to show. Routes appear once a router and a delivery module are set up."
      />
    );
  } else if (!hasData && loading) {
    body = <LoadingState label="Loading routes" rows={4} />;
  } else if (!hasData && error) {
    body = (
      <EmptyState
        kind="error"
        illustration="error"
        role="status"
        title="Couldn't load routes"
        body="The routes or the delivery history could not be read. Your access may have changed, or the runtime is busy."
        action={onRefresh ? <Button size="sm" onClick={onRefresh}>Retry</Button> : undefined}
        details={[{ label: "Error", value: error, plain: true }]}
      />
    );
  } else if (!hasData) {
    body = (
      <EmptyState
        illustration="routes"
        title="No routes yet"
        body="Routes carry messages from a channel, such as email, a webhook or a schedule, to an agent or an address. They appear here once a module defines them."
      />
    );
  } else {
    const main = (
      <div className="routing-flow" ref={flowRef} data-testid="routing-flow">
        <div className="routing-flow__head" aria-hidden="true">
          <span className="routing-flow__head-channel">Channel</span>
          <span className="routing-flow__head-route">Route</span>
          <span className="routing-flow__head-recipient">Recipient</span>
          <span className="routing-flow__head-dots">Recent deliveries</span>
          <span className="routing-flow__head-status">Status</span>
        </div>
        {groups.length === 0 ? (
          <EmptyState
            kind="filtered"
            illustration="search"
            title={needle ? `No routes match "${query.trim()}"${failingOnly ? " and Failing" : ""}` : "No route is failing"}
            action={<Button size="sm" onClick={() => { setQuery(""); setFailingOnly(false); }}>Clear filters</Button>}
          />
        ) : (
          groups.map((group) => (
            <ChannelBlock
              key={group.channel}
              group={group}
              selectedKey={selectedKey}
              onSelect={(key) => select(key === selectedKey ? null : key)}
              expanded={expanded.has(group.channel) || (filtering && group.routes.length <= 12)}
              canFold={!filtering}
              onToggle={() => setExpanded((current) => {
                const next = new Set(current);
                if (next.has(group.channel)) next.delete(group.channel);
                else next.add(group.channel);
                return next;
              })}
              live={liveChannels.has(group.channel)}
            />
          ))
        )}
        {!filtering ? <UnmatchedBlock deliveries={model.unmatched} /> : null}
        {model.approximate || model.window?.clock === "runtime" ? (
          <p className="routing-flow__foot" data-testid="routing-flow-note">
            {joinMeta(
              model.approximate ? "Deliveries are matched by recipient: delivery records don't name their route yet" : null,
              model.window?.clock === "runtime" ? "Delivery times count from when the runtime started" : null,
            )}
          </p>
        ) : null}
      </div>
    );
    body = (
      <DrawerLayout
        main={main}
        open={Boolean(active)}
        onClose={() => setSelectedKey(null)}
        persistKey="routing"
        backLabel="All routes"
        backTestId="routing-drawer-back"
        drawer={active ? <RouteDrawer key={active.key} view={active} model={model} onClose={() => setSelectedKey(null)} /> : null}
      />
    );
  }

  return (
    <PanelShell className="routing" testId="routing-panel" density={density} showTechnicalIds={showTechnicalIds}>
      <PanelHeader
        title="Routing"
        summary={summary}
        summaryTestId="routing-summary"
        freshness={updatedAtMs && !denied ? { at: updatedAtMs, updating: loading && hasData } : undefined}
        refreshing={loading && hasData}
        tools={onRefresh ? <IconButton label="Refresh routing" glyph="refresh" data-testid="routing-refresh" onClick={onRefresh} /> : undefined}
        menu={{
          label: "More actions for Routing",
          triggerTestId: "routing-menu",
          items: standardPanelMenu({
            density,
            onDensityChange: setDensity,
            showTechnicalIds,
            onShowTechnicalIdsChange: setShowTechnicalIds,
            getCopyData: () => ({ routes: data.routes, deliveries: data.deliveries }),
            onRefresh,
            testIdPrefix: "routing",
          }),
        }}
      />
      {hasData && !denied && !unavailable ? (
        <FilterBar
          className="routing__filters"
          testId="routing-filters"
          ariaLabel="Filter routes"
          search={{ label: "Filter routes", value: query, onChange: setQuery, placeholder: "Route or recipient", testId: "routing-search" }}
          chips={failingRoutes > 0 || failingOnly ? [{
            label: "Failing",
            count: failingRoutes,
            tone: "crit",
            pressed: failingOnly,
            onToggle: setFailingOnly,
            testId: "routing-filter:failing",
          }] : []}
          onClear={() => { setQuery(""); setFailingOnly(false); }}
        />
      ) : null}
      <StatusRow mode="toast" error={hasData && error ? `Couldn't refresh routing (${error}).` : null} errorTestId="routing-error" updatedAt={updatedAtMs ?? undefined} onRetry={onRefresh} />
      <div className="routing__body">{body}</div>
    </PanelShell>
  );
}
