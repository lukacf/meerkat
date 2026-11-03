import { describe, expect, it } from "vitest";

import { topologyLineage } from "./agent-lineage";

describe("topologyLineage", () => {
  const answer = {
    nodes: [
      { endpoint: { authority: "ops", identity: "carrier-watch" }, labels: { display_name: "Carrier Watch", spawned_by: "shipping" } },
      { endpoint: { authority: "ops", identity: "shipping" }, labels: { display_name: "Shipping" } },
      { identity: "returns-surge", labels: { spawned_by: " dispatch " } },
      { endpoint: { identity: "loop" }, labels: { spawned_by: "loop" } },
      null,
      "not a node",
    ],
  };

  it("maps each helper to the agent that started it, from the node labels", () => {
    expect(topologyLineage(answer)).toEqual({ "carrier-watch": "shipping", "returns-surge": "dispatch" });
  });

  it("returns the same map for the same answer, and an empty one without an answer", () => {
    expect(topologyLineage(answer) === topologyLineage(answer)).toBe(true);
    expect(topologyLineage(null)).toEqual({});
    expect(topologyLineage({ nodes: "none" })).toEqual({});
  });
});
