/**
 * Routing in words: a route names its recipient; the channel, sink and
 * module keys and the delivery IDs stay in tooltips and technical details.
 * Real deliveries name their routing resolution ("route-000017"), not the
 * route's key, and are matched to it by recipient, sink and module.
 */
import React from "react";
import { act, fireEvent, render, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { RoutingSectionView } from "@console-core";
import { RoutingPanel } from "./RoutingPanel";
import { NavigationFocusProvider } from "../lib/console-navigation";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

const NOW = Date.now();

const data: RoutingSectionView = {
  routes: [
    {
      route_key: "release.approvals",
      recipient: "identity:release-manager",
      channel: "identity",
      sink: "gating",
      target_module: "inbox",
      retry_max: 3,
      backoff_ms: 500,
      rate_limit_per_minute: 120,
    },
    {
      route_key: "nightly.digest",
      recipient: "identity:nightly-monitor",
      channel: "schedule",
      sink: "digest",
      target_module: "delivery",
    },
  ],
  deliveries: [
    {
      delivery_id: "dlv_01JBX7S1UH3K9M5",
      route_id: "release.approvals",
      recipient: "identity:release-manager",
      sink: "gating",
      target_module: "inbox",
      status: "delivered",
      first_attempt_ms: NOW - 60_000,
      final_attempt_ms: NOW - 50_000,
      attempts: [
        { attempt: 1, status: "failed", backoff_ms: 500 },
        { attempt: 2, status: "delivered", backoff_ms: 0 },
      ],
    },
  ],
};

function directory() {
  return createAgentDirectory([
    { id: "identity:release-manager", label: "Release Manager" },
    { id: "identity:nightly-monitor", label: "Nightly Monitor" },
    { id: "billing", label: "Billing" },
  ]);
}

function renderRouting(props: Partial<React.ComponentProps<typeof RoutingPanel>> = {}) {
  return render(
    <AgentDirectoryProvider directory={directory()}>
      <RoutingPanel data={data} updatedAtMs={NOW} {...props} />
    </AgentDirectoryProvider>,
  );
}

function titles(element: Element): string[] {
  return Array.from(element.querySelectorAll("[title]"), (node) => node.getAttribute("title") ?? "");
}

/** An element's text without its collapsed technical details. */
function textOutsideDetails(element: Element): string {
  const copy = element.cloneNode(true) as Element;
  copy.querySelectorAll("details").forEach((details) => details.remove());
  return copy.textContent ?? "";
}

function openRoute(view: ReturnType<typeof renderRouting>, key: string) {
  const row = view.getByTestId(`routing-route:${key}`);
  fireEvent.click(within(row).getByRole("button", { name: key }));
}

/** The seeded preview's shape: delivery records name their routing
 * resolution, never the route key; a fifth of the alerts fail. */
function seededData(): RoutingSectionView {
  const routes: RoutingSectionView["routes"] = [
    { route_key: "all-alerts", channel: "*", recipient: "alerts@example.com", sink: "email", target_module: "delivery", retry_max: 1, backoff_ms: 1000, rate_limit_per_minute: 120 },
    { route_key: "billing-escalations", channel: "email", recipient: "finance-lead@example.com", sink: "email", target_module: "delivery", retry_max: 2, backoff_ms: 500, rate_limit_per_minute: 120 },
    { route_key: "daily-brief", channel: "schedule", recipient: "ops-lead@example.com", sink: "email", target_module: "delivery", retry_max: 0, backoff_ms: 0, rate_limit_per_minute: 120 },
    { route_key: "incident-webhook", channel: "webhook", recipient: "https://hooks.example.com/incidents", sink: "webhook", target_module: "delivery", retry_max: 4, backoff_ms: 250, rate_limit_per_minute: 120 },
    ...["accounts", "billing", "dispatch", "incident", "intake", "orders"].map((identity) => ({
      route_key: `mob.member.${identity}`, channel: "notification", recipient: identity, sink: "mob_member", target_module: "delivery",
    })),
  ];
  const recipients = [
    { recipient: "https://hooks.example.com/incidents", sink: "webhook" },
    { recipient: "ops-lead@example.com", sink: "email" },
    { recipient: "alerts@example.com", sink: "email" },
    { recipient: "finance-lead@example.com", sink: "email" },
  ];
  const deliveries: RoutingSectionView["deliveries"] = Array.from({ length: 20 }, (_, index) => {
    const target = recipients[index % 4];
    const failed = target.recipient === "alerts@example.com" && index % 8 === 2;
    const at = NOW - (20 - index) * 60_000;
    return {
      delivery_id: `delivery-${String(index + 1).padStart(6, "0")}`,
      route_id: `route-${String(index + 1).padStart(6, "0")}`,
      recipient: target.recipient,
      sink: target.sink,
      target_module: "delivery",
      status: failed ? "failed" : "sent",
      first_attempt_ms: at - 1000,
      final_attempt_ms: at,
      attempts: failed
        ? [{ attempt: 1, status: "transient_failure", backoff_ms: 1000 }, { attempt: 2, status: "failed", backoff_ms: 0 }]
        : [{ attempt: 1, status: "sent", backoff_ms: 0 }],
    };
  });
  return { routes, deliveries };
}

describe("RoutingPanel", () => {
  it("names a route's recipient in words, with the keys in the tooltips", () => {
    const view = renderRouting();
    const row = view.getByTestId("routing-route:release.approvals");
    const text = row.textContent ?? "";
    expect(text.includes("Delivers to Release Manager")).toBe(true);
    expect(/Identity to|via|gating|inbox|identity:release-manager/.test(text)).toBe(false);
    expect(titles(row).includes("Channel: identity · Sink: gating · Module: inbox")).toBe(true);
    expect(titles(row).includes("identity:release-manager")).toBe(true);
  });

  it("keeps the sink and module keys in the route's technical details and names the channel", () => {
    const view = renderRouting();
    // No route is open until one is chosen: the flow takes the whole pane.
    expect(view.container.querySelector(".routing__detail") === null).toBe(true);
    openRoute(view, "release.approvals");
    const detail = view.container.querySelector(".routing__detail");
    expect(detail === null).toBe(false);
    const visible = textOutsideDetails(detail as Element);
    expect(visible.includes("Release Manager")).toBe(true);
    expect(visible.includes("Identity")).toBe(true);
    expect(/Module|inbox|gating|via/.test(visible)).toBe(false);
    const technical = (detail as Element).querySelector("details")?.textContent ?? "";
    for (const expected of ["Recipient key", "identity:release-manager", "Channel", "identity", "Sink", "gating", "Module", "inbox"]) {
      expect(technical.includes(expected)).toBe(true);
    }
  });

  it("shows a delivery's ID only on hover, and its retries only when it failed", () => {
    const view = renderRouting();
    openRoute(view, "release.approvals");
    const delivery = view.getByTestId("routing-drawer-delivery:dlv_01JBX7S1UH3K9M5");
    const text = delivery.textContent ?? "";
    expect(text.includes("dlv_01JBX7S1UH3K9M5")).toBe(false);
    expect(text.includes("Release Manager")).toBe(true);
    expect(text.includes("Delivered")).toBe(true);
    // Attempts are synthetic: a delivered message never counts them.
    expect(/attempt|retried/.test(text)).toBe(false);
    expect(titles(delivery).some((title) => title.includes("Delivery dlv_01JBX7S1UH3K9M5"))).toBe(true);
    // The flow marks the same delivery on the route's strip.
    expect(view.getByTestId("routing-delivery:dlv_01JBX7S1UH3K9M5").getAttribute("data-tone")).toBe("ok");
  });

  it("finds a route by its recipient's name", () => {
    const view = renderRouting();
    const search = view.getByPlaceholderText("Route or recipient");
    fireEvent.change(search, { target: { value: "nightly monitor" } });
    expect(view.queryByTestId("routing-route:nightly.digest") === null).toBe(false);
    expect(view.queryByTestId("routing-route:release.approvals") === null).toBe(true);
  });

  it("keeps an exact route key match when a delivery names its route", () => {
    const view = renderRouting();
    const strip = view.getByTestId("routing-dots:release.approvals");
    expect(strip.querySelectorAll("[data-testid^='routing-delivery:']").length).toBe(1);
    expect(view.getByTestId("routing-route:release.approvals").textContent?.includes("1 delivered")).toBe(true);
    // An exact match needs no caveat.
    expect(view.getByTestId("routing-flow").textContent?.includes("matched by recipient")).toBe(false);
  });
});

describe("RoutingPanel with real delivery records", () => {
  it("matches deliveries to their routes by recipient, sink and module, and says so", () => {
    const view = renderRouting({ data: seededData() });
    const alerts = view.getByTestId("routing-route:all-alerts");
    // Five alerts were sent, three of them failed.
    expect(alerts.textContent?.includes("3 of 5 failed")).toBe(true);
    expect(alerts.getAttribute("data-state")).toBe("failing");
    expect(view.getByTestId("routing-route:billing-escalations").textContent?.includes("5 delivered")).toBe(true);
    expect(view.getByTestId("routing-route:incident-webhook").textContent?.includes("No deliveries yet")).toBe(false);
    expect(view.queryAllByText("No recent deliveries").length).toBe(0);
    expect(view.getByTestId("routing-dots:all-alerts").querySelectorAll("[data-shape='diamond']").length).toBe(3);
    expect(view.getByTestId("routing-flow").textContent?.includes("matched by recipient")).toBe(true);
    const summary = view.getByTestId("routing-summary").textContent ?? "";
    expect(summary.includes("10 routes")).toBe(true);
    expect(summary.includes("20 recent deliveries")).toBe(true);
    expect(view.getByTestId("routing-summary-failed").textContent).toBe("3 failed");
    expect(view.getByTestId("routing-summary-failed").getAttribute("data-tone")).toBe("crit");
  });

  it("says why a delivery failed in a sentence and lists the newest first", () => {
    const view = renderRouting({ data: seededData() });
    openRoute(view, "all-alerts");
    const drawer = view.getByTestId("routing-drawer");
    expect(within(drawer).getByTestId("routing-drawer-matched").textContent?.includes("Matched by recipient")).toBe(true);
    const rows = Array.from(drawer.querySelectorAll<HTMLElement>("[data-testid^='routing-drawer-delivery:']"));
    expect(rows.length).toBe(5);
    const ids = rows.map((row) => row.getAttribute("data-testid"));
    expect(ids[0]).toBe("routing-drawer-delivery:delivery-000019");
    expect(ids[ids.length - 1]).toBe("routing-drawer-delivery:delivery-000003");
    const failed = rows.filter((row) => row.getAttribute("data-outcome") === "failed");
    expect(failed.length).toBe(3);
    expect(failed[0].textContent?.includes("Couldn't deliver to alerts@example.com, retried 1 time.")).toBe(true);
  });

  it("folds a channel of many agent routes into one row until it is opened", () => {
    const view = renderRouting({ data: seededData() });
    const fold = view.getByTestId("routing-fold:notification");
    expect(fold.textContent?.includes("6 routes")).toBe(true);
    expect(fold.textContent?.includes("6 agents, one route each")).toBe(true);
    expect(view.queryByTestId("routing-route:mob.member.billing") === null).toBe(true);
    fireEvent.click(within(fold).getByRole("button", { name: "Show the 6 routes of Notification" }));
    expect(view.queryByTestId("routing-route:mob.member.billing") === null).toBe(false);
    expect(view.getByTestId("routing-route:mob.member.billing").textContent?.includes("Delivers to Billing")).toBe(true);
  });

  it("reads runtime-relative delivery times as time after start, never as 1970 dates", () => {
    const relative: RoutingSectionView = {
      routes: data.routes,
      deliveries: [{ ...data.deliveries[0], first_attempt_ms: 12 * 60_000, final_attempt_ms: 12 * 60_000 }],
    };
    const view = renderRouting({ data: relative });
    expect(view.getByTestId("routing-route:release.approvals").textContent?.includes("12 min after start")).toBe(true);
    expect(view.container.textContent?.includes("1970")).toBe(false);
  });
});

describe("RoutingPanel states", () => {
  it("shows no access, not an empty router, when the reads are refused", () => {
    const view = renderRouting({ data: { routes: [], deliveries: [] }, denied: true });
    expect(view.getByTestId("routing-no-access").getAttribute("data-kind")).toBe("no-access");
    expect(view.queryByText("No routes yet") === null).toBe(true);
    expect(view.queryByText("No routes configured") === null).toBe(true);
    expect(view.getByTestId("routing-summary").textContent?.includes("No access to routing")).toBe(true);
  });

  it("keeps the routes on screen and says a refresh failed", () => {
    const onRefresh = vi.fn();
    const view = renderRouting({ error: "gateway busy", onRefresh });
    expect(view.getByTestId("routing-error").textContent).toBe("Couldn't refresh routing (gateway busy).");
    expect(view.getByTestId("routing-route:release.approvals")).toBeTruthy();
    fireEvent.click(view.getByRole("button", { name: "Retry" }));
    expect(onRefresh).toHaveBeenCalledTimes(1);
  });

  it("teaches what routes are when none are configured", () => {
    const view = renderRouting({ data: { routes: [], deliveries: [] } });
    expect(view.getByText("No routes yet")).toBeTruthy();
  });
});

describe("RoutingPanel navigation", () => {
  it("opens the route another panel links to, selected and in view", () => {
    const done = vi.fn();
    const scroll = vi.fn();
    const original = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = scroll;
    const raf = vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => { callback(0); return 0; });
    try {
      const view = render(
        <AgentDirectoryProvider directory={directory()}>
          <NavigationFocusProvider value={{ request: { kind: "route", id: "nightly.digest", serial: 7 }, done }}>
            <RoutingPanel data={data} updatedAtMs={NOW} />
          </NavigationFocusProvider>
        </AgentDirectoryProvider>,
      );
      act(() => {});
      expect(done).toHaveBeenCalledWith(7);
      expect(view.getByTestId("routing-drawer").getAttribute("aria-label")).toBe("Route nightly.digest");
      expect(view.getByTestId("routing-route:nightly.digest").classList.contains("is-selected")).toBe(true);
      expect(scroll.mock.calls.length > 0).toBe(true);
    } finally {
      raf.mockRestore();
      Element.prototype.scrollIntoView = original;
    }
  });

  it("closes the drawer and keeps the flow when Close details is pressed", () => {
    const view = renderRouting();
    openRoute(view, "release.approvals");
    fireEvent.click(view.getByTestId("routing-drawer-close"));
    expect(view.queryByTestId("routing-drawer") === null).toBe(true);
    expect(view.getByTestId("routing-flow")).toBeTruthy();
  });
});
