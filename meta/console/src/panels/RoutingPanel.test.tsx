/**
 * Routing in words: a route names its recipient; the channel, sink and
 * module keys and the delivery IDs stay in tooltips and technical details.
 */
import React from "react";
import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { RoutingSectionView } from "@console-core";
import { RoutingPanel } from "./RoutingPanel";
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

function renderRouting() {
  const directory = createAgentDirectory([
    { id: "identity:release-manager", label: "Release Manager" },
    { id: "identity:nightly-monitor", label: "Nightly Monitor" },
  ]);
  return render(
    <AgentDirectoryProvider directory={directory}>
      <RoutingPanel data={data} />
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

  it("keeps the channel, sink and module keys in the route's technical details", () => {
    const view = renderRouting();
    const detail = view.container.querySelector(".routing__detail");
    expect(detail === null).toBe(false);
    const visible = textOutsideDetails(detail as Element);
    expect(visible.includes("Release Manager")).toBe(true);
    expect(/Module|Channel|inbox|gating|via/.test(visible)).toBe(false);
    const technical = (detail as Element).querySelector("details")?.textContent ?? "";
    for (const expected of ["Recipient key", "identity:release-manager", "Channel", "identity", "Sink", "gating", "Module", "inbox"]) {
      expect(technical.includes(expected)).toBe(true);
    }
  });

  it("shows a delivery's ID only on hover", () => {
    const view = renderRouting();
    const delivery = view.getByTestId("routing-delivery:dlv_01JBX7S1UH3K9M5");
    const text = delivery.textContent ?? "";
    expect(text.includes("dlv_01JBX7S1UH3K9M5")).toBe(false);
    expect(text.includes("Release Manager")).toBe(true);
    expect(text.includes("Delivered")).toBe(true);
    expect(text.includes("2 attempts")).toBe(true);
    expect(titles(delivery).some((title) => title.includes("Delivery dlv_01JBX7S1UH3K9M5"))).toBe(true);
  });

  it("finds a route by its recipient's name", () => {
    const view = renderRouting();
    const search = view.getByPlaceholderText("Route or recipient");
    fireEvent.change(search, { target: { value: "nightly monitor" } });
    expect(view.queryByTestId("routing-route:nightly.digest") === null).toBe(false);
    expect(view.queryByTestId("routing-route:release.approvals") === null).toBe(true);
  });
});
