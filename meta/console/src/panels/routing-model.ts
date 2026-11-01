// The Routing panel's model (spec B12): which channels feed which routes and
// recipients, and how each route's recent deliveries went. Pure functions
// over the two reads (`meta/routing/routes/list`, `meta/delivery/history`),
// so the join, the order and the words are tested without React.
//
// Deliveries name the routing resolution they were sent under (`route_id`,
// "route-000017"), not the configured route's key, until the server adds
// `route_key` to delivery records (server change S1). A delivery whose
// `route_id` equals a route's key still matches it exactly; any other is
// matched to the routes with the same recipient, sink and module, and the
// panel says so ("matched by recipient").

import type { RoutingSectionView } from "@console-core";

export type Route = RoutingSectionView["routes"][number];
export type Delivery = RoutingSectionView["deliveries"][number];

/** How a route's deliveries were found: by its key, or by its target. */
export type DeliveryMatch = "exact" | "recipient";

/** What a recipient is, for its mark and its words. */
export type RecipientKind = "agent" | "email" | "url" | "phone" | "other";

export type DeliveryOutcome = "delivered" | "failed" | "pending" | "other";

export type ModelTone = "neutral" | "info" | "ok" | "warn" | "crit" | "live";

/** One delivery as the panel shows it. */
export interface DeliveryView {
  delivery: Delivery;
  id: string;
  /** When it last happened: its final attempt, else its first. */
  at: number;
  /** Epoch milliseconds, or milliseconds since the runtime started. */
  clock: "epoch" | "runtime";
  outcome: DeliveryOutcome;
  tone: ModelTone;
  /** The status in words: "Delivered", "Failed", "Queued". */
  word: string;
  /** Attempts recorded for it. */
  attempts: number;
  /** "retried 1 time", only for a failed delivery (attempts are synthetic). */
  retried: string | null;
  /** What was sent, in words, when the payload names it. */
  summary: string | null;
}

/** One configured route with the deliveries matched to it. */
export interface RouteView {
  route: Route;
  key: string;
  channel: string;
  /** "Email", "Any channel". */
  channelLabel: string;
  recipientKind: RecipientKind;
  /** Newest first. */
  deliveries: DeliveryView[];
  match: DeliveryMatch | null;
  /** Other routes that share this route's recipient, sink and module, so a
   * delivery matched by recipient may belong to either. */
  sharedWith: string[];
  delivered: number;
  failed: number;
  total: number;
  last: DeliveryView | null;
  tone: ModelTone;
  /** "4 of 12 failed", "12 delivered", "No deliveries yet". */
  statusWords: string;
}

/** The routes of one channel. A channel with many routes (one per agent)
 * folds into one row until it is opened. */
export interface ChannelGroup {
  channel: string;
  label: string;
  routes: RouteView[];
  foldable: boolean;
}

export interface RoutingModel {
  routes: RouteView[];
  groups: ChannelGroup[];
  /** Every delivery, newest first. */
  deliveries: DeliveryView[];
  /** Deliveries no configured route matches. */
  unmatched: DeliveryView[];
  totals: { routes: number; deliveries: number; delivered: number; failed: number };
  /** The time span the delivery strips share. */
  window: { start: number; end: number; clock: "epoch" | "runtime" } | null;
  /** Some route's deliveries were matched by recipient. */
  approximate: boolean;
}

/** A channel with more routes than this folds into one row. */
export const FOLD_ROUTES_OVER = 5;
/** Deliveries a route's strip shows, oldest at the left, newest at the
 * right edge, one slot each: the strip reads as a run of outcomes. */
export const STRIP_DELIVERIES = 16;

/** Times before 2001 are milliseconds since the runtime started, not epoch
 * times: the delivery clock counts from the runtime's start until a merged
 * event carries a wall-clock time. */
const EPOCH_FLOOR_MS = Date.UTC(2001, 0, 1);

const SUMMARY_KEYS = ["summary", "subject", "title", "alert", "message", "text", "event"] as const;
const SUMMARY_MAX = 96;

export function deliveryTime(delivery: Pick<Delivery, "final_attempt_ms" | "first_attempt_ms">): number {
  return delivery.final_attempt_ms || delivery.first_attempt_ms || 0;
}

export function deliveryClock(ms: number): "epoch" | "runtime" {
  return ms >= EPOCH_FLOOR_MS ? "epoch" : "runtime";
}

/** "Email", "Webhook", "Any channel" (`*`). */
export function channelLabel(channel: string | undefined): string {
  const value = (channel ?? "").trim();
  if (!value || value === "*") return "Any channel";
  return value.charAt(0).toUpperCase() + value.slice(1).replace(/[_-]+/g, " ");
}

/** What a recipient key names: an agent (in the directory, or a team
 * member's route), an email address, a web address, a phone, or other. */
export function recipientKind(recipient: string, sink: string, isAgent: (key: string) => boolean): RecipientKind {
  const value = recipient.trim();
  if (isAgent(value) || sink === "mob_member") return "agent";
  if (/^https?:\/\//i.test(value)) return "url";
  if (/^\+?[0-9][0-9 ()-]{5,}$/.test(value)) return "phone";
  if (/^[^\s@]+@[^\s@]+$/.test(value)) return "email";
  return "other";
}

const OUTCOME_BY_STATUS: Record<string, DeliveryOutcome> = {
  sent: "delivered",
  delivered: "delivered",
  succeeded: "delivered",
  success: "delivered",
  ok: "delivered",
  failed: "failed",
  failure: "failed",
  delivery_failed: "failed",
  dead_letter: "failed",
  rejected: "failed",
  error: "failed",
  queued: "pending",
  pending: "pending",
  retrying: "pending",
  scheduled: "pending",
  delivering: "pending",
  sending: "pending",
};

const WORD_BY_STATUS: Record<string, string> = {
  sent: "Delivered",
  delivered: "Delivered",
  succeeded: "Delivered",
  success: "Delivered",
  ok: "Delivered",
  failed: "Failed",
  failure: "Failed",
  delivery_failed: "Failed",
  dead_letter: "Gave up",
  rejected: "Refused",
  error: "Failed",
  queued: "Queued",
  pending: "Waiting",
  retrying: "Retrying",
  scheduled: "Scheduled",
  delivering: "Delivering",
  sending: "Delivering",
};

function normalizeStatus(status: string): string {
  return status.trim().toLowerCase().replace(/[\s-]+/g, "_");
}

function sentenceCase(value: string): string {
  const words = value.trim().replace(/[_-]+/g, " ").replace(/\s+/g, " ").toLowerCase();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "";
}

export function deliveryOutcome(status: string): DeliveryOutcome {
  return OUTCOME_BY_STATUS[normalizeStatus(status)] ?? "other";
}

/** A delivery's status in words: "Delivered" for sent, "Failed", "Queued". */
export function deliveryWord(status: string): string {
  return WORD_BY_STATUS[normalizeStatus(status)] ?? (sentenceCase(status) || "Unknown");
}

function outcomeTone(outcome: DeliveryOutcome, status: string): ModelTone {
  if (outcome === "delivered") return "ok";
  if (outcome === "failed") return "crit";
  if (outcome === "pending") return normalizeStatus(status) === "delivering" || normalizeStatus(status) === "sending" ? "live" : "info";
  return "neutral";
}

/** "retried 1 time", "retried 3 times": only a failed delivery says how
 * often it was retried (the attempt list is synthetic). */
export function retriedWords(attempts: number): string | null {
  const retries = Math.max(0, attempts - 1);
  if (retries === 0) return "not retried";
  return `retried ${retries} ${retries === 1 ? "time" : "times"}`;
}

/** What a delivery sent, in words, from the first text field of its payload
 * that names it ("Queue above 30 new tickets"). */
export function payloadSummary(payload: unknown): string | null {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) return null;
  const record = payload as Record<string, unknown>;
  for (const key of SUMMARY_KEYS) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) {
      const text = value.trim().replace(/\s+/g, " ");
      return text.length > SUMMARY_MAX ? `${text.slice(0, SUMMARY_MAX - 1)}…` : text;
    }
  }
  return null;
}

/** The payload summaries of a delivery history read, by delivery id. The
 * normalized delivery records leave payloads out; the host reads them here. */
export function deliverySummaries(historyResponse: unknown): Record<string, string> {
  const record = historyResponse && typeof historyResponse === "object" ? historyResponse as { deliveries?: unknown } : null;
  const out: Record<string, string> = {};
  if (!record || !Array.isArray(record.deliveries)) return out;
  for (const entry of record.deliveries) {
    if (!entry || typeof entry !== "object") continue;
    const delivery = entry as { delivery_id?: unknown; payload?: unknown };
    if (typeof delivery.delivery_id !== "string" || !delivery.delivery_id.trim()) continue;
    const summary = payloadSummary(delivery.payload);
    if (summary) out[delivery.delivery_id.trim()] = summary;
  }
  return out;
}

export function deliveryView(delivery: Delivery, summaries?: Readonly<Record<string, string>>): DeliveryView {
  const at = deliveryTime(delivery);
  const outcome = deliveryOutcome(delivery.status);
  const attempts = Math.max(delivery.attempts?.length ?? 0, 1);
  return {
    delivery,
    id: delivery.delivery_id,
    at,
    clock: deliveryClock(at),
    outcome,
    tone: outcomeTone(outcome, delivery.status),
    word: deliveryWord(delivery.status),
    attempts,
    retried: outcome === "failed" ? retriedWords(attempts) : null,
    summary: summaries?.[delivery.delivery_id] ?? null,
  };
}

function sameTarget(route: Route, delivery: Delivery): boolean {
  return route.recipient === delivery.recipient
    && route.sink === delivery.sink
    && route.target_module === delivery.target_module;
}

function routeStatus(delivered: number, failed: number, total: number, pending: number): { tone: ModelTone; words: string } {
  if (total === 0) return { tone: "neutral", words: "No deliveries yet" };
  if (failed > 0) return { tone: "crit", words: `${failed} of ${total} failed` };
  if (pending > 0 && delivered === 0) return { tone: "info", words: `${pending} waiting` };
  return { tone: "ok", words: `${delivered} delivered` };
}

/**
 * The panel's model: every route with the deliveries matched to it (newest
 * first), the channel groups, the unmatched deliveries and the totals.
 */
export function buildRoutingModel(
  data: RoutingSectionView,
  options: {
    isAgent?: (key: string) => boolean;
    summaries?: Readonly<Record<string, string>>;
  } = {},
): RoutingModel {
  const isAgent = options.isAgent ?? (() => false);
  const routes = data.routes ?? [];
  // Both reads return oldest first; the panel reads newest first.
  const deliveries = (data.deliveries ?? [])
    .map((delivery) => deliveryView(delivery, options.summaries))
    .sort((a, b) => b.at - a.at);

  const byKey = new Map(routes.map((route) => [route.route_key, route] as const));
  const matched = new Map<string, { deliveries: DeliveryView[]; exact: number; recipient: number }>();
  for (const route of routes) matched.set(route.route_key, { deliveries: [], exact: 0, recipient: 0 });
  const unmatched: DeliveryView[] = [];
  for (const view of deliveries) {
    const exact = byKey.get(view.delivery.route_id);
    if (exact) {
      const slot = matched.get(exact.route_key)!;
      slot.deliveries.push(view);
      slot.exact += 1;
      continue;
    }
    const candidates = routes.filter((route) => sameTarget(route, view.delivery));
    if (!candidates.length) {
      unmatched.push(view);
      continue;
    }
    for (const route of candidates) {
      const slot = matched.get(route.route_key)!;
      slot.deliveries.push(view);
      slot.recipient += 1;
    }
  }

  const routeViews: RouteView[] = routes.map((route) => {
    const slot = matched.get(route.route_key)!;
    const delivered = slot.deliveries.filter((view) => view.outcome === "delivered").length;
    const failed = slot.deliveries.filter((view) => view.outcome === "failed").length;
    const pending = slot.deliveries.filter((view) => view.outcome === "pending").length;
    const total = slot.deliveries.length;
    const status = routeStatus(delivered, failed, total, pending);
    return {
      route,
      key: route.route_key,
      channel: route.channel ?? "",
      channelLabel: channelLabel(route.channel),
      recipientKind: recipientKind(route.recipient, route.sink, isAgent),
      deliveries: slot.deliveries,
      match: slot.recipient > 0 ? "recipient" : slot.exact > 0 ? "exact" : null,
      sharedWith: routes
        .filter((other) => other.route_key !== route.route_key && other.recipient === route.recipient
          && other.sink === route.sink && other.target_module === route.target_module)
        .map((other) => other.route_key),
      delivered,
      failed,
      total,
      last: slot.deliveries[0] ?? null,
      tone: status.tone,
      statusWords: status.words,
    };
  });

  // Channels in a stable order: named channels by name, any channel next,
  // then the channels folded into one row (one route per agent).
  const byChannel = new Map<string, RouteView[]>();
  for (const view of routeViews) {
    const key = view.channel || "*";
    const list = byChannel.get(key);
    if (list) list.push(view);
    else byChannel.set(key, [view]);
  }
  const groups: ChannelGroup[] = [...byChannel.entries()].map(([channel, list]) => ({
    channel,
    label: channelLabel(channel),
    routes: [...list].sort((a, b) => a.key.localeCompare(b.key)),
    foldable: list.length > FOLD_ROUTES_OVER,
  }));
  const rank = (group: ChannelGroup) => (group.foldable ? 2 : group.channel === "*" ? 1 : 0);
  groups.sort((a, b) => rank(a) - rank(b) || a.label.localeCompare(b.label));

  const times = deliveries.map((view) => view.at).filter((at) => at > 0);
  let window: RoutingModel["window"] = null;
  if (times.length) {
    const start = Math.min(...times);
    const end = Math.max(...times);
    // A quiet span still draws its marks apart: pad by 2 % (at least 1 s).
    const pad = Math.max(1000, (end - start) * 0.02);
    window = { start: start - pad, end: end + pad, clock: deliveryClock(end) };
  }

  return {
    routes: routeViews,
    groups,
    deliveries,
    unmatched,
    totals: {
      routes: routes.length,
      deliveries: deliveries.length,
      delivered: deliveries.filter((view) => view.outcome === "delivered").length,
      failed: deliveries.filter((view) => view.outcome === "failed").length,
    },
    window,
    approximate: routeViews.some((view) => view.match === "recipient"),
  };
}

/** A delivery's time in words: "4m ago" for an epoch time; "12 min after
 * start" for a time the runtime counts from its own start. */
export function runtimeOffsetWords(ms: number): string {
  if (ms < 60_000) return `${Math.max(0, Math.floor(ms / 1000))} s after start`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)} min after start`;
  const hours = Math.floor(ms / 3_600_000);
  const minutes = Math.round((ms % 3_600_000) / 60_000);
  return minutes ? `${hours} h ${minutes} min after start` : `${hours} h after start`;
}

/** A route in one sentence for its row's accessible name and the drawer:
 * "Email to finance-lead@example.com: 12 delivered". */
export function routeSentence(view: RouteView, recipientName: string): string {
  return `${view.channelLabel} through ${view.key} to ${recipientName}: ${view.statusWords}`;
}

/** A failed delivery as a sentence: "Couldn't deliver to alerts@example.com,
 * retried 1 time." */
export function failureSentence(view: DeliveryView, recipientName: string): string {
  return `Couldn't deliver to ${recipientName}, ${view.retried ?? "not retried"}.`;
}
