import { cleanup, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { IdentityInspectViewState } from "@console-core";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";
import { buildGraph } from "../../../packages/console-components/src/topology/data";
import { memberReferenceKeys } from "../../../packages/console-core/src/member-ref";
import { AgentDetailsPanel } from "../panels/AgentDetailsPanel";
import type { ConsoleAgent, ConsoleExperience } from "../types";
import { createAgentResolver } from "./activity-ledger";
import { buildDockTarget, buildPanelConversationKey, sidebarAgentPinId } from "./adapters";
import { agentDirectoryEntry, buildConsoleIdentityAliasMap, canonicalConsoleIdentityFromMap, normalizeAgents } from "./agents";
import { consoleComposerDraftKey, consoleSendStorageKey } from "./send-attempt-storage";
import { workGraphItemOwner } from "./workgraph-layout";

// Members with one name in several mobs (`reviewer`, `alpha/reviewer`,
// `beta/reviewer`): every key the console builds from an agent is its whole
// identity, and a reference that names a mob names that mob's member only.

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const agent = (id: string, label: string, mobId: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  agent_id: id,
  member_id: id,
  identity: id,
  label,
  kind: "mob_agent",
  role: "analyst",
  mob_id: mobId,
  ...extra,
});

const AGENTS: ConsoleAgent[] = [
  agent("planner", "Planner", "ops-team", { role: "coordinator", wired_to: ["alpha/reviewer", "alpha/archivist"] }),
  agent("reviewer", "Primary reviewer", "ops-team"),
  agent("archivist", "Primary archivist", "ops-team"),
  agent("alpha/reviewer", "Alpha reviewer", "alpha", { wired_to: ["planner"] }),
  agent("beta/reviewer", "Beta reviewer", "beta"),
];

describe("the keys of same-named members", () => {
  it("are distinct for panes, pins, drafts, queues and frames", () => {
    const targets = AGENTS.filter((entry) => entry.member_id.endsWith("reviewer")).map(buildDockTarget);
    expect(targets.map((target) => target.id)).toEqual(["reviewer", "alpha/reviewer", "beta/reviewer"]);
    expect(new Set(targets.map((target) => buildPanelConversationKey("pane-1", target))).size).toBe(3);
    expect(AGENTS.map(sidebarAgentPinId)).toEqual(["planner", "reviewer", "archivist", "alpha/reviewer", "beta/reviewer"]);
    // Storage keys carry the identity encoded, so its `/` never ends a key part.
    expect(consoleSendStorageKey("scope", "alpha/reviewer")).toBe("meta-send-attempts:v1:scope:alpha%2Freviewer");
    expect(consoleComposerDraftKey("scope", "alpha/reviewer", "composer-1")).toBe("meta-composer-draft:v2:scope:alpha%2Freviewer:composer-1");
    expect(consoleSendStorageKey("scope", "alpha/reviewer") === consoleSendStorageKey("scope", "reviewer")).toBe(false);
    const aliases = buildConsoleIdentityAliasMap(AGENTS);
    for (const identity of ["reviewer", "alpha/reviewer", "beta/reviewer"]) {
      expect(canonicalConsoleIdentityFromMap(identity, aliases)).toBe(identity);
    }
  });

  it("keeps a nested member's own identity and status when a label names another identity", () => {
    const agents = normalizeAgents({
      identity_status: {
        rows: [
          { identity: "reviewer", display_name: "Primary reviewer", state: "active", addressability: "addressable" },
          { identity: "alpha/reviewer", display_name: "Alpha reviewer", state: "retiring", addressability: "addressable" },
        ],
      },
      agent_sidebar: {
        live_snapshot: {
          agents: [
            { agent_id: "reviewer", member_id: "reviewer", identity: "reviewer", label: "Primary reviewer", mob_id: "ops-team" },
            {
              agent_id: "alpha/reviewer",
              member_id: "alpha/reviewer",
              identity: "alpha/reviewer",
              label: "Alpha reviewer",
              mob_id: "alpha",
              labels: { agent_identity: "reviewer" },
            },
          ],
        },
      },
    } as unknown as ConsoleExperience, []);
    expect(agents.map((entry) => [entry.identity, entry.member_id, entry.state, entry.label])).toEqual([
      ["reviewer", "reviewer", "active", "Primary reviewer"],
      ["alpha/reviewer", "alpha/reviewer", "retiring", "Alpha reviewer"],
    ]);
  });
});

describe("a reference that names a mob", () => {
  const nested = new Set(["alpha", "beta"]);

  it("is looked up as that mob's member only", () => {
    expect(memberReferenceKeys("alpha/reviewer", nested)).toEqual({ keys: ["alpha/reviewer"], nested: true });
    expect(memberReferenceKeys("alpha/analyst/reviewer", nested)).toEqual({ keys: ["alpha/analyst/reviewer", "alpha/reviewer"], nested: true });
    // Any other reference keeps its lookups: itself, then its last segment.
    expect(memberReferenceKeys("reviewer", nested)).toEqual({ keys: ["reviewer"], nested: false });
    expect(memberReferenceKeys("ops-team/analyst/reviewer", nested)).toEqual({ keys: ["ops-team/analyst/reviewer", "reviewer"], nested: false });
    expect(memberReferenceKeys("alpha/reviewer", null)).toEqual({ keys: ["alpha/reviewer", "reviewer"], nested: false });
  });

  it("resolves a peer name to the member of its mob in the activity ledger", () => {
    const resolve = createAgentResolver(AGENTS);
    expect(resolve("reviewer")).toBe("reviewer");
    expect(resolve("alpha/reviewer")).toBe("alpha/reviewer");
    expect(resolve("alpha/analyst/reviewer")).toBe("alpha/reviewer");
    expect(resolve("beta/analyst/reviewer")).toBe("beta/reviewer");
    expect(resolve("ops-team/analyst/reviewer")).toBe("reviewer");
    // A member of a listed mob the console does not list is never the
    // primary mob's member of that name.
    expect(resolve("alpha/archivist")).toBe("alpha/archivist");
    expect(resolve("alpha/analyst/archivist")).toBe("alpha/archivist");
  });

  it("gives each member its own topology node and never borrows another mob's", () => {
    const graph = buildGraph(
      AGENTS.map((entry) => ({ identity: entry.identity!, label: entry.label, role: entry.role, wired_to: entry.wired_to || [], labels: {}, mob_id: entry.mob_id })),
      AGENTS,
    );
    expect(graph.agents.map((node) => node.id)).toEqual(["planner", "reviewer", "archivist", "alpha/reviewer", "beta/reviewer"]);
    expect(graph.resolve("alpha/reviewer")).toBe("alpha/reviewer");
    expect(graph.resolve("beta/analyst/reviewer")).toBe("beta/reviewer");
    expect(graph.resolve("reviewer")).toBe("reviewer");
    expect(graph.resolve("alpha/archivist")).toBeNull();
    expect(graph.resolve("alpha/analyst/archivist")).toBeNull();
    // The planner is wired to alpha's reviewer; the unlisted alpha/archivist
    // draws no edge to the primary mob's archivist.
    const wired = graph.edges.filter((edge) => edge.relation === "wired").map((edge) => [edge.from, edge.to].sort().join(" - "));
    expect(wired).toEqual(["alpha/reviewer - planner"]);
  });

  it("names a work item's owner by the member of the owner key's mob", () => {
    const directory = createAgentDirectory(AGENTS.map(agentDirectoryEntry));
    const owner = (id: string) => workGraphItemOwner({ owner: { key: { kind: "agent", id } } }, directory)?.name;
    expect(owner("mob/alpha/agent/reviewer")).toBe("Alpha reviewer");
    expect(owner("mob/beta/agent/reviewer")).toBe("Beta reviewer");
    expect(owner("mob/ops-team/agent/reviewer")).toBe("Primary reviewer");
    expect(owner("mob/alpha/agent/archivist")).toBe("Archivist");
  });

  it("shows an unlisted peer of another mob by its own name in the details panel", () => {
    const directory = createAgentDirectory(AGENTS.map(agentDirectoryEntry));
    const inspect = {
      identity: "planner",
      display_name: "Planner",
      role: "coordinator",
      state: "active",
      addressability: "addressable",
      labels: {},
      continuity: { generation: 1 },
      topology_peers: ["alpha/reviewer", "alpha/archivist"],
    } as unknown as IdentityInspectViewState;
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel identity="planner" inspect={inspect} agent={AGENTS[0]} agents={AGENTS} stateLabel="active" actions={{}} />
      </AgentDirectoryProvider>,
    );
    const connections = within(view.getByTestId("inspect-panel:planner")).getByTestId("inspect-connections:planner");
    const text = connections.textContent || "";
    expect(text.includes("Alpha reviewer")).toBe(true);
    expect(text.includes("Archivist")).toBe(true);
    expect(text.includes("Primary archivist")).toBe(false);
    expect(text.includes("Primary reviewer")).toBe(false);
  });
});
