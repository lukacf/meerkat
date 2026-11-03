import { describe, expect, it } from "vitest";

import type { RoutingSectionView } from "@console-core";
import {
  buildRoutingModel,
  channelLabel,
  deliveryClock,
  deliverySummaries,
  deliveryWord,
  payloadSummary,
  recipientKind,
  retriedWords,
  runtimeOffsetWords,
} from "./routing-model";

type Delivery = RoutingSectionView["deliveries"][number];

const route = (key: string, recipient: string, extra: Partial<RoutingSectionView["routes"][number]> = {}) => ({
  route_key: key, recipient, channel: "email", sink: "email", target_module: "delivery", ...extra,
});
const delivery = (id: string, routeId: string, recipient: string, at: number, status = "sent", extra: Partial<Delivery> = {}): Delivery => ({
  delivery_id: id, route_id: routeId, recipient, sink: "email", target_module: "delivery", status,
  first_attempt_ms: at, final_attempt_ms: at, attempts: [{ attempt: 1, status, backoff_ms: 0 }], ...extra,
});

describe("the route and delivery join", () => {
  it("keeps an exact match, matches a resolution id by recipient, and keeps what matches nothing", () => {
    const now = Date.UTC(2026, 10, 1, 12);
    const model = buildRoutingModel({
      routes: [route("alerts", "alerts@example.com"), route("finance", "finance@example.com")],
      deliveries: [
        delivery("d1", "alerts", "alerts@example.com", now - 3000),
        delivery("d2", "route-000002", "finance@example.com", now - 2000, "failed", {
          attempts: [{ attempt: 1, status: "transient_failure", backoff_ms: 500 }, { attempt: 2, status: "failed", backoff_ms: 0 }],
        }),
        delivery("d3", "route-000003", "nobody@example.com", now - 1000),
      ],
    });
    const alerts = model.routes.find((view) => view.key === "alerts")!;
    const finance = model.routes.find((view) => view.key === "finance")!;
    expect(alerts.match).toBe("exact");
    expect(alerts.deliveries.map((view) => view.id)).toEqual(["d1"]);
    expect(finance.match).toBe("recipient");
    expect(finance.statusWords).toBe("1 of 1 failed");
    expect(finance.tone).toBe("crit");
    expect(finance.deliveries[0].retried).toBe("retried 1 time");
    expect(model.unmatched.map((view) => view.id)).toEqual(["d3"]);
    expect(model.approximate).toBe(true);
    expect(model.totals).toEqual({ routes: 2, deliveries: 3, delivered: 2, failed: 1 });
    // Newest first, whatever order the history read gave.
    expect(model.deliveries.map((view) => view.id)).toEqual(["d3", "d2", "d1"]);
  });

  it("lists a delivery under every route that shares its recipient, sink and module", () => {
    const model = buildRoutingModel({
      routes: [route("any-alerts", "ops@example.com", { channel: "*" }), route("email-alerts", "ops@example.com")],
      deliveries: [delivery("d1", "route-000001", "ops@example.com", Date.now())],
    });
    for (const view of model.routes) {
      expect(view.deliveries.length).toBe(1);
      expect(view.sharedWith.length).toBe(1);
    }
    expect(model.totals.deliveries).toBe(1);
  });

  it("folds a channel with more than five routes and orders any channel after the named ones", () => {
    const routes = [
      route("brief", "ops@example.com", { channel: "schedule" }),
      route("all", "alerts@example.com", { channel: "*" }),
      ...Array.from({ length: 6 }, (_, index) => route(`mob.member.a${index}`, `a${index}`, { channel: "notification", sink: "mob_member" })),
    ];
    const model = buildRoutingModel({ routes, deliveries: [] });
    expect(model.groups.map((group) => [group.label, group.foldable])).toEqual([
      ["Schedule", false],
      ["Any channel", false],
      ["Notification", true],
    ]);
    expect(model.routes.find((view) => view.key === "mob.member.a0")?.recipientKind).toBe("agent");
    expect(model.routes.find((view) => view.key === "brief")?.statusWords).toBe("No deliveries yet");
    expect(model.window).toBe(null);
  });
});

describe("routing words", () => {
  it("names channels, recipients and statuses for people", () => {
    expect(channelLabel("*")).toBe("Any channel");
    expect(channelLabel("webhook")).toBe("Webhook");
    expect(channelLabel(undefined)).toBe("Any channel");
    const isAgent = (key: string) => key === "billing";
    expect(recipientKind("billing", "email", isAgent)).toBe("agent");
    expect(recipientKind("ana@example.com", "email", isAgent)).toBe("email");
    expect(recipientKind("https://hooks.example.com/x", "webhook", isAgent)).toBe("url");
    expect(recipientKind("+44 20 7946 0000", "sms", isAgent)).toBe("phone");
    expect(recipientKind("queue:refunds", "queue", isAgent)).toBe("other");
    expect(deliveryWord("sent")).toBe("Delivered");
    expect(deliveryWord("dead_letter")).toBe("Gave up");
    expect(deliveryWord("custom_state")).toBe("Custom state");
    expect(retriedWords(1)).toBe("not retried");
    expect(retriedWords(4)).toBe("retried 3 times");
  });

  it("tells epoch times from times counted from the runtime's start", () => {
    expect(deliveryClock(Date.UTC(2026, 0, 1))).toBe("epoch");
    expect(deliveryClock(1_717_171_710)).toBe("runtime");
    expect(runtimeOffsetWords(45_000)).toBe("45 s after start");
    expect(runtimeOffsetWords(12 * 60_000)).toBe("12 min after start");
    expect(runtimeOffsetWords(2 * 3_600_000 + 5 * 60_000)).toBe("2 h 5 min after start");
  });

  it("reads what a delivery sent from the payload fields that name it", () => {
    expect(payloadSummary({ event: "incident.opened", summary: "Carrier tracking timing out" })).toBe("Carrier tracking timing out");
    expect(payloadSummary({ alert: "Queue above 30 new tickets", force_fail: true })).toBe("Queue above 30 new tickets");
    expect(payloadSummary({ count: 3 })).toBe(null);
    expect(payloadSummary("plain")).toBe(null);
    expect(payloadSummary({ summary: "x".repeat(200) })?.length).toBe(96);
    expect(deliverySummaries({ deliveries: [
      { delivery_id: "d1", payload: { subject: "Daily brief" } },
      { delivery_id: "d2", payload: {} },
      { payload: { subject: "no id" } },
    ] })).toEqual({ d1: "Daily brief" });
    expect(deliverySummaries(null)).toEqual({});
  });
});
