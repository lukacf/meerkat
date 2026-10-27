import { describe, expect, test } from "vitest";

import {
  denseGraphConnectEdge,
  denseGraphDisconnectEdge,
  denseGraphEdgeAtPoint,
  denseGraphEdgeFingerprint,
  denseGraphEdgeIsTorn,
  denseGraphLayoutFingerprint,
  denseGraphNodeAtPoint,
  denseGraphPendingNodeIds,
} from "./dense-graph-map";
import { buildGraph, edgeKey } from "./data";

describe("DenseGraphMap geometry and edit semantics", () => {
  test("selects the nearest node inside a zoom-aware hit target", () => {
    const nodes = [
      { id: "alpha", x: 10, y: 10 },
      { id: "beta", x: 25, y: 10 },
    ];

    expect(denseGraphNodeAtPoint(nodes, { x: 23, y: 10 }, 1)).toBe("beta");
    expect(denseGraphNodeAtPoint(nodes, { x: 80, y: 80 }, 1)).toBeNull();
    expect(denseGraphNodeAtPoint(nodes, { x: 43, y: 10 }, 0.5)).toBe("beta");
  });

  test("hit-tests sampled curves and only tears after the gesture threshold", () => {
    const curves = [{
      key: "alpha|beta",
      pointAt: (t: number) => ({ x: t * 100, y: t * 20 }),
    }];

    expect(denseGraphEdgeAtPoint(curves, { x: 50, y: 10 }, 1)).toBe("alpha|beta");
    expect(denseGraphEdgeAtPoint(curves, { x: 50, y: 40 }, 1)).toBeNull();
    expect(denseGraphEdgeIsTorn({ x: 0, y: 0 }, { x: 24, y: 0 })).toBe(false);
    expect(denseGraphEdgeIsTorn({ x: 0, y: 0 }, { x: 27, y: 0 })).toBe(true);
  });

  test("invalidates same-count edge replacements", () => {
    const first = denseGraphEdgeFingerprint([
      { from: "alpha", to: "beta" },
      { from: "beta", to: "gamma" },
    ]);
    const replacement = denseGraphEdgeFingerprint([
      { from: "alpha", to: "gamma" },
      { from: "beta", to: "gamma" },
    ]);

    expect(first).not.toBe(replacement);
    expect(denseGraphEdgeFingerprint([
      { from: "beta", to: "alpha" },
      { from: "gamma", to: "beta" },
    ])).toBe(first);
  });

  test("keeps the node layout fingerprint stable across edge edits", () => {
    const disconnected = buildGraph([
      { identity: "alpha", label: "Alpha", role: "planner", group: "Project", wired_to: [] },
      { identity: "beta", label: "Beta", role: "builder", group: "Project", wired_to: [] },
    ], []);
    const connected = buildGraph([
      { identity: "alpha", label: "Alpha", role: "planner", group: "Project", wired_to: ["beta"] },
      { identity: "beta", label: "Beta", role: "builder", group: "Project", wired_to: ["alpha"] },
    ], []);

    expect(denseGraphEdgeFingerprint(disconnected.edges)).not.toBe(
      denseGraphEdgeFingerprint(connected.edges),
    );
    expect(denseGraphLayoutFingerprint(connected)).toBe(
      denseGraphLayoutFingerprint(disconnected),
    );
    expect(denseGraphLayoutFingerprint({
      ...connected,
      agents: [...connected.agents].reverse(),
    })).toBe(denseGraphLayoutFingerprint(connected));
    expect(denseGraphLayoutFingerprint({
      ...connected,
      groups: ["alpha|beta"],
    })).not.toBe(denseGraphLayoutFingerprint({
      ...connected,
      groups: ["alpha", "beta"],
    }));
  });

  test("produces endpoint pairs for connect drops and edge tears", () => {
    const existing = new Set([edgeKey("alpha", "beta")]);
    expect(denseGraphConnectEdge("alpha", "gamma", existing)).toEqual({ from: "alpha", to: "gamma" });
    expect(denseGraphConnectEdge("alpha", "beta", existing)).toBeNull();
    expect(denseGraphConnectEdge("alpha", "alpha", existing)).toBeNull();
    expect(denseGraphDisconnectEdge({ from: "alpha", to: "beta" }, false)).toBeNull();
    expect(denseGraphDisconnectEdge({ from: "alpha", to: "beta" }, true)).toEqual({ from: "alpha", to: "beta" });
  });

  test("blocks every graph edit involving a pending edge or node", () => {
    const pendingEdges = new Set([edgeKey("alpha", "beta")]);
    const pendingNodes = denseGraphPendingNodeIds(
      ["alpha", "beta", "gamma", "delta"],
      pendingEdges,
    );

    expect(Array.from(pendingNodes).sort()).toEqual(["alpha", "beta"]);
    expect(denseGraphConnectEdge(
      "alpha",
      "gamma",
      new Set(),
      pendingEdges,
      pendingNodes,
    )).toBeNull();
    expect(denseGraphDisconnectEdge(
      { from: "alpha", to: "gamma" },
      true,
      pendingEdges,
      pendingNodes,
    )).toBeNull();
    expect(denseGraphConnectEdge(
      "gamma",
      "delta",
      new Set(),
      pendingEdges,
      pendingNodes,
    )).toEqual({ from: "gamma", to: "delta" });
  });

  test("resolves pending nodes within a 1k-node / 10k-edge budget", () => {
    const agentIds = Array.from({ length: 1_000 }, (_value, index) => `agent-${index}`);
    const pendingEdges = new Set<string>();
    for (let source = 0; source < agentIds.length; source += 1) {
      for (let offset = 1; offset <= 10; offset += 1) {
        pendingEdges.add(edgeKey(
          agentIds[source],
          agentIds[(source + offset * 17) % agentIds.length],
        ));
      }
    }
    expect(pendingEdges.size).toBe(10_000);
    const startedAt = performance.now();
    const pendingNodes = denseGraphPendingNodeIds(agentIds, pendingEdges);
    const elapsedMs = performance.now() - startedAt;

    expect(pendingNodes.size).toBeGreaterThan(0);
    expect(elapsedMs).toBeLessThan(250);
  });
});
