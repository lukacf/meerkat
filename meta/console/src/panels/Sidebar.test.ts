import assert from "node:assert/strict";
import test from "node:test";

import type { ConsoleAgent, ConsoleAgentListConfig, ConsoleFrame } from "../types";
import { __sidebarTest, readSidebarPins } from "./Sidebar";

test("sidebar treats broad-profile worker wired to commander as a child, not the host", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["full-tools-worker-2"],
  };
  const worker: ConsoleAgent = {
    agent_id: "full-tools-worker-2",
    member_id: "full-tools-worker-2",
    identity: "full-tools-worker-2",
    label: "full-tools-worker-2",
    kind: "mob_agent",
    role: "commander",
    group: "commander",
    wired_to: ["incident-commander"],
  };

  assert.equal(__sidebarTest.isCommanderLike(worker), false);
  assert.equal(__sidebarTest.isCommanderLike(commander), true);
  assert.equal(__sidebarTest.isSpawnedDelegateLike(worker, commander), true);
});

test("sidebar configured links reuse console href allowlist", () => {
  assert.equal(__sidebarTest.safeConsoleHref("https://example.test/docs"), "https://example.test/docs");
  assert.equal(__sidebarTest.safeConsoleHref("/console/docs"), "/console/docs");
  assert.equal(__sidebarTest.safeConsoleHref("javascript:alert(1)"), null);
  assert.equal(__sidebarTest.safeConsoleHref("data:text/plain,hello"), null);
});

test("sidebar marks a member degraded from its identity view state alone", () => {
  const member: ConsoleAgent = {
    agent_id: "worker-1",
    member_id: "worker-1",
    identity: "worker-1",
    label: "worker-1",
    kind: "mob_agent",
  };
  assert.equal(__sidebarTest.deriveStateAttr({ ...member, state: "broken" }), "degraded");
  assert.equal(__sidebarTest.deriveStateAttr({ ...member, state: "active" }), "active");
  assert.notEqual(__sidebarTest.deriveStateAttr({ ...member, state: "dormant" }), "degraded");
  // A 0.9.3 gateway still sends lease_healthy; the sidebar no longer reads it.
  const leaseLost = { ...member, state: "active", lease_healthy: false } as ConsoleAgent;
  assert.equal(__sidebarTest.deriveStateAttr(leaseLost), "active");
});

test("sidebar resolves worker-spawned workers under their worker host", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["tutti-profile-worker"],
  };
  const worker: ConsoleAgent = {
    agent_id: "tutti-profile-worker",
    member_id: "tutti-profile-worker",
    identity: "tutti-profile-worker",
    label: "tutti-profile-worker",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: ["incident-commander", "standby-worker"],
  };
  const subWorker: ConsoleAgent = {
    agent_id: "standby-worker",
    member_id: "standby-worker",
    identity: "standby-worker",
    label: "standby-worker",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: ["tutti-profile-worker"],
  };
  const agents = [commander, worker, subWorker];

  assert.equal(__sidebarTest.findSpawnHost(worker, agents, commander)?.member_id, "incident-commander");
  assert.equal(__sidebarTest.findSpawnHost(subWorker, agents, commander)?.member_id, "tutti-profile-worker");
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents(agents).get("Coordinators")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["incident-commander", 0],
      ["tutti-profile-worker", 1],
      ["standby-worker", 2],
    ],
  );
});

test("sidebar uses runtime-derived spawned_by when no wiring identifies the host", () => {
  const parent: ConsoleAgent = {
    agent_id: "ops-lead",
    member_id: "ops-lead",
    identity: "ops-lead",
    label: "Ops Lead",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: [],
  };
  const child: ConsoleAgent = {
    agent_id: "durable-investigator",
    member_id: "durable-investigator",
    identity: "durable-investigator",
    label: "Durable Investigator",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: [],
    labels: { spawned_by: "ops-lead", via_tool: "fork_off" },
  };

  assert.equal(__sidebarTest.findSpawnHost(child, [child, parent], parent)?.member_id, "ops-lead");
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents([child, parent]).get("Coordinators")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["ops-lead", 0],
      ["durable-investigator", 1],
    ],
  );
});

test("sidebar nests a same-role durable fork under its runtime-derived parent", () => {
  const parent: ConsoleAgent = {
    agent_id: "personal-agent",
    member_id: "personal-agent",
    identity: "personal-agent",
    label: "Personal Agent",
    kind: "mob_agent",
    role: "coordinator",
    group: "Coordinators",
    wired_to: [],
  };
  const child: ConsoleAgent = {
    agent_id: "personal-agent-voice-work",
    member_id: "personal-agent-voice-work",
    identity: "personal-agent-voice-work",
    label: "Personal Agent Voice Work",
    kind: "mob_agent",
    role: "coordinator",
    group: "Coordinators",
    wired_to: [],
    labels: { spawned_by: "personal-agent", via_tool: "fork_off" },
  };

  assert.equal(__sidebarTest.isCommanderLike(child), true);
  assert.equal(
    __sidebarTest.findSpawnHost(child, [parent, child], parent)?.member_id,
    "personal-agent",
  );
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents([parent, child]).get("Coordinators")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["personal-agent", 0],
      ["personal-agent-voice-work", 1],
    ],
  );
});

test("sidebar nests workers under wired non-worker hosts", () => {
  const investigator: ConsoleAgent = {
    agent_id: "rt:deep-investigator:singleton:0",
    member_id: "rt:deep-investigator:singleton:0",
    identity: "deep-investigator:singleton",
    label: "Deep Investigator",
    kind: "mob_agent",
    role: "deep-investigator",
    group: "Coordinators",
    wired_to: ["investigation-worker-daily-comet"],
  };
  const worker: ConsoleAgent = {
    agent_id: "investigation-worker-daily-comet",
    member_id: "investigation-worker-daily-comet",
    identity: "investigation-worker-daily-comet",
    label: "investigation-worker-daily-comet",
    kind: "mob_agent",
    role: "investigation-worker",
    group: "worker",
    wired_to: ["deep-investigator:singleton"],
  };
  const agents = [worker, investigator];

  assert.equal(__sidebarTest.findSpawnHost(worker, agents, null)?.member_id, "rt:deep-investigator:singleton:0");
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents(agents).get("Coordinators")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["rt:deep-investigator:singleton:0", 0],
      ["investigation-worker-daily-comet", 1],
    ],
  );
});

test("sidebar configured grouping keeps spawned investigation workers under their coordinator host", () => {
  const investigator: ConsoleAgent = {
    agent_id: "rt:deep-investigator:singleton:0",
    member_id: "rt:deep-investigator:singleton:0",
    identity: "deep-investigator:singleton",
    label: "Deep Investigator",
    kind: "mob_agent",
    role: "deep-investigator",
    group: "Coordinators",
    wired_to: ["investigation-worker-nested-spawn-1"],
  };
  const worker: ConsoleAgent = {
    agent_id: "investigation-worker-nested-spawn-1",
    member_id: "investigation-worker-nested-spawn-1",
    identity: "investigation-worker-nested-spawn-1",
    label: "investigation-worker-nested-spawn-1",
    kind: "mob_agent",
    role: "investigation-worker",
    group: "investigation-worker",
    wired_to: ["deep-investigator:singleton"],
  };
  const grouped = __sidebarTest.groupSidebarAgents([worker, investigator], {
    group_by: ["labels.console_group", "group", "role"],
    section_order: ["Coordinators", "investigation-worker"],
  });

  assert.equal(grouped.has("investigation-worker"), false);
  assert.deepEqual(
    grouped.get("Coordinators")?.map((row) => [row.agent.member_id, row.depth]),
    [
      ["rt:deep-investigator:singleton:0", 0],
      ["investigation-worker-nested-spawn-1", 1],
    ],
  );
});

test("sidebar configured subgroups inherit from parents so children stay under their parent subgroup", () => {
  const parent: ConsoleAgent = {
    agent_id: "initiative:parent",
    member_id: "initiative:parent",
    identity: "initiative:parent",
    label: "Parent Initiative",
    kind: "mob_agent",
    role: "initiative",
    group: "initiatives",
    labels: {
      group: "initiatives",
      scope_id: "parent-scope",
    },
    wired_to: ["initiative:child"],
  };
  const child: ConsoleAgent = {
    agent_id: "initiative:child",
    member_id: "initiative:child",
    identity: "initiative:child",
    label: "Child Worker",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    labels: {
      group: "workers",
      scope_id: "child-scope",
    },
    wired_to: ["initiative:parent"],
  };
  const grouped = __sidebarTest.groupSidebarAgents([child, parent], opsGrouping);

  assert.deepEqual(
    grouped.get("initiatives")?.map((row) => [row.agent.member_id, row.subgroup, row.depth]),
    [
      ["initiative:parent", "parent-scope", 0],
      ["initiative:child", "parent-scope", 1],
    ],
  );
});

test("sidebar resolves worker-spawned workers from implicit delegate parent refs", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["incident-worker-full-1"],
  };
  const worker: ConsoleAgent = {
    agent_id: "incident-worker-full-1",
    member_id: "incident-worker-full-1",
    identity: "incident-worker-full-1",
    label: "incident-worker-full-1",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: ["incident-commander", "implicit-019e186d-48e4-7da1-9559-4d17155ab30d/delegate/cardinalpay-support-worker-1"],
  };
  const subWorker: ConsoleAgent = {
    agent_id: "cardinalpay-support-worker-1",
    member_id: "cardinalpay-support-worker-1",
    identity: "cardinalpay-support-worker-1",
    label: "cardinalpay-support-worker-1",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: ["implicit-019e186d-48e4-7da1-9559-4d17155ab30d/delegate/incident-worker-full-1"],
  };
  const agents = [commander, subWorker, worker];

  assert.equal(__sidebarTest.findSpawnHost(worker, agents, commander)?.member_id, "incident-commander");
  assert.equal(__sidebarTest.findSpawnHost(subWorker, agents, commander)?.member_id, "incident-worker-full-1");
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents(agents).get("Coordinators")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["incident-commander", 0],
      ["incident-worker-full-1", 1],
      ["cardinalpay-support-worker-1", 2],
    ],
  );
});

test("sidebar nests delegate-created workers even when runtime groups them as coordinators", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["qa2-parent-worker"],
  };
  const parentWorker: ConsoleAgent = {
    agent_id: "qa2-parent-worker",
    member_id: "qa2-parent-worker",
    identity: "qa2-parent-worker",
    label: "qa2-parent-worker",
    kind: "mob_agent",
    role: "delegate",
    group: "Coordinators",
    labels: {
      delegate_host_identity: "incident-commander",
      source_mob_id: "implicit-019e22a9-4e67-7f62-a9e2-3d96c8d43439",
    },
    wired_to: [
      "incident-command-center/commander/incident-commander",
      "incident-commander",
    ],
  };
  const childWorker: ConsoleAgent = {
    agent_id: "qa2-child-worker",
    member_id: "qa2-child-worker",
    identity: "qa2-child-worker",
    label: "qa2-child-worker",
    kind: "mob_agent",
    role: "delegate",
    group: "Coordinators",
    labels: {
      delegate_host_identity: "qa2-parent-worker",
      source_mob_id: "implicit-019e22ab-64c8-7d43-a19a-2e12cab16f0f",
    },
    wired_to: [
      "implicit-019e22a9-4e67-7f62-a9e2-3d96c8d43439/delegate/qa2-parent-worker",
      "qa2-parent-worker",
    ],
  };
  const agents = [commander, childWorker, parentWorker];

  assert.equal(__sidebarTest.findSpawnHost(parentWorker, agents, commander)?.member_id, "incident-commander");
  assert.equal(__sidebarTest.findSpawnHost(childWorker, agents, commander)?.member_id, "qa2-parent-worker");
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents(agents).get("Coordinators")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["incident-commander", 0],
      ["qa2-parent-worker", 1],
      ["qa2-child-worker", 2],
    ],
  );
});

test("sidebar resolves encoded identity references while nesting workers", () => {
  const commander: ConsoleAgent = {
    agent_id: "identity:luka",
    member_id: "identity:luka",
    identity: "identity:luka",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["identity/worker"],
  };
  const worker: ConsoleAgent = {
    agent_id: "identity:worker",
    member_id: "identity:worker",
    identity: "identity:worker",
    label: "identity-worker",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: ["identity/luka"],
  };

  assert.equal(__sidebarTest.findSpawnHost(worker, [commander, worker], commander)?.member_id, "identity:luka");
});

test("sidebar can group agents by configured metadata selectors and subgroups", () => {
  const agents: ConsoleAgent[] = [
    {
      agent_id: "initiative:billing",
      member_id: "initiative:billing",
      identity: "initiative:billing",
      label: "Billing Initiative",
      kind: "mob_agent",
      role: "initiative",
      group: "Domains",
      labels: {
        console_group: "Initiatives",
        org: "Payments",
      },
    },
    {
      agent_id: "initiative:comms",
      member_id: "initiative:comms",
      identity: "initiative:comms",
      label: "Comms Initiative",
      kind: "mob_agent",
      role: "initiative",
      group: "Domains",
      labels: {
        console_group: "Initiatives",
        org: "Customer",
      },
    },
    {
      agent_id: "identity:luka",
      member_id: "identity:luka",
      identity: "identity:luka",
      label: "Example",
      kind: "mob_agent",
      role: "identity",
      group: "Personal",
      labels: {},
    },
  ];

  const grouped = __sidebarTest.groupSidebarAgents(agents, {
    group_by: ["labels.console_group", "group"],
    subgroup_by: ["labels.org"],
    section_order: ["Personal", "Initiatives"],
    badges: [
      { id: "org", label: "Org", field: "labels.org" },
    ],
  });

  assert.deepEqual(
    grouped.get("Initiatives")?.map((row) => [row.agent.member_id, row.subgroup]),
    [
      ["initiative:comms", "Customer"],
      ["initiative:billing", "Payments"],
    ],
  );
  assert.deepEqual(
    grouped.get("Personal")?.map((row) => row.agent.member_id),
    ["identity:luka"],
  );
  assert.deepEqual(
    __sidebarTest.configuredAgentBadges(agents[0], {
      badges: [
        { id: "org", label: "Org", field: "labels.org" },
      ],
    }),
    [{ id: "org", label: "Org", value: "Payments", tone: undefined }],
  );
});

class MemoryStorage {
  private readonly values = new Map<string, string>();

  get length(): number {
    return this.values.size;
  }

  key(index: number): string | null {
    return Array.from(this.values.keys())[index] ?? null;
  }

  getItem(key: string): string | null {
    return this.values.has(key) ? this.values.get(key)! : null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

const opsGrouping = {
  group_by: ["labels.group", "role"],
  subgroup_by: ["labels.scope_id"],
  section_order: ["coordinators", "initiatives", "workers", "recipients"],
  collapse_single_subgroup: true,
};

function opsAgent(args: {
  id: string;
  label: string;
  group: string;
  scope: string;
  role?: string;
  identity?: string;
  labelIdentity?: string;
}): ConsoleAgent {
  return {
    agent_id: `rt:${args.id}`,
    member_id: `member:${args.id}`,
    identity: args.identity,
    label: args.label,
    kind: "mob_agent",
    role: args.role || args.group,
    labels: {
      group: args.group,
      scope_id: args.scope,
      scope_ids: args.scope,
      ...(args.labelIdentity ? { agent_identity: args.labelIdentity } : {}),
    },
  };
}

function virtualRowsForAgents(
  agents: ConsoleAgent[],
  options: {
    collapsedSections?: Set<string>;
    collapsedSubgroups?: Set<string>;
    pinnedAgentIds?: Set<string>;
    searchActive?: boolean;
  } = {},
) {
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const sectionNames = __sidebarTest.orderedSectionNames(grouped, opsGrouping);
  return __sidebarTest.buildSidebarVirtualRows({
    sectionNames,
    grouped,
    grouping: opsGrouping,
    collapsedSections: options.collapsedSections || new Set(),
    collapsedSubgroups: options.collapsedSubgroups || new Set(),
    pinnedAgentIds: options.pinnedAgentIds,
    searchActive: options.searchActive,
  });
}

test("sidebar section collapse storage overrides config defaults after first load", () => {
  const storage = new MemoryStorage();
  const storageKey = __sidebarTest.sidebarStorageKey("sections", "runtime-a");
  const grouping = {
    sections: [
      { name: "initiatives", collapsed: true },
      { name: "workers", collapsed: false },
    ],
  };

  assert.deepEqual(
    Array.from(__sidebarTest.collapsedSectionsForStorage(grouping, storageKey, storage)),
    ["initiatives"],
  );

  __sidebarTest.writeSidebarStringSet(storage, storageKey, new Set(["workers"]));

  assert.deepEqual(
    Array.from(__sidebarTest.collapsedSectionsForStorage(grouping, storageKey, storage)),
    ["workers"],
  );
});

test("sidebar storage namespace separates configured defaults from persisted user preferences", () => {
  const storage = new MemoryStorage();
  const grouping = {
    sections: [
      { name: "initiatives", collapsed: true },
      { name: "workers", collapsed: false },
    ],
  };
  const runtimeAKey = __sidebarTest.sidebarStorageKey("sections", "runtime-a:hash");
  const runtimeBKey = __sidebarTest.sidebarStorageKey("sections", "runtime-b:hash");

  __sidebarTest.writeSidebarStringSet(storage, runtimeAKey, new Set(["workers"]));

  assert.deepEqual(
    Array.from(__sidebarTest.collapsedSectionsForStorage(grouping, runtimeAKey, storage)).sort(),
    ["workers"],
    "persisted user preference wins in its namespace",
  );
  assert.deepEqual(
    Array.from(__sidebarTest.collapsedSectionsForStorage(grouping, runtimeBKey, storage)).sort(),
    ["initiatives"],
    "unrelated namespace still uses configured defaults",
  );
});

test("sidebar subgroup headers render and can remove their agents when collapsed", () => {
  const agents = [
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "initiative-liveops", label: "LiveOps Initiative", group: "initiatives", scope: "liveops", role: "initiative" }),
  ];
  const expanded = virtualRowsForAgents(agents);

  assert.deepEqual(
    expanded.filter((row) => row.kind === "subgroup").map((row) => [row.bucket, row.label, row.collapsed]),
    [
      ["initiatives", "cto", false],
      ["initiatives", "liveops", false],
    ],
  );

  const collapsed = virtualRowsForAgents(agents, {
    collapsedSubgroups: new Set([__sidebarTest.sidebarSubgroupStorageId("initiatives", "cto")]),
  });

  assert.deepEqual(
    collapsed.filter((row) => row.kind === "agent").map((row) => row.row.agent.member_id),
    ["member:initiative-liveops"],
  );
});

test("sidebar subgroup collapse state persists through storage", () => {
  const storage = new MemoryStorage();
  const storageKey = __sidebarTest.sidebarStorageKey("subgroups", "runtime-a");
  const subgroupKey = __sidebarTest.sidebarSubgroupStorageId("initiatives", "liveops");
  __sidebarTest.writeSidebarStringSet(storage, storageKey, new Set([subgroupKey]));

  assert.deepEqual(
    Array.from(__sidebarTest.collapsedSubgroupsForStorage(storageKey, storage)),
    [subgroupKey],
  );
});

test("sidebar exact order preferences persist through storage without sorting", () => {
  const storage = new MemoryStorage();
  const sectionOrderKey = __sidebarTest.sidebarStorageKey(
    __sidebarTest.SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
    "runtime-a",
  );
  const subgroupOrderKey = __sidebarTest.sidebarStorageKey(
    __sidebarTest.SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
    "runtime-a",
  );
  const subgroupBeta = __sidebarTest.sidebarSubgroupStorageId("initiatives", "beta");
  const subgroupAlpha = __sidebarTest.sidebarSubgroupStorageId("initiatives", "alpha");

  __sidebarTest.writeSidebarStringList(storage, sectionOrderKey, ["recipients", "workers", "initiatives"]);
  __sidebarTest.writeSidebarStringList(storage, subgroupOrderKey, [subgroupBeta, subgroupAlpha]);

  assert.deepEqual(
    __sidebarTest.readSidebarStringList(storage, sectionOrderKey),
    ["recipients", "workers", "initiatives"],
  );
  assert.deepEqual(
    __sidebarTest.readSidebarStringList(storage, subgroupOrderKey),
    [subgroupBeta, subgroupAlpha],
  );
});

test("sidebar search expands section and subgroup matches without mutating saved collapse state", () => {
  const agents = [
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "initiative-liveops", label: "LiveOps Initiative", group: "initiatives", scope: "liveops", role: "initiative" }),
  ];
  const collapsedSections = new Set(["initiatives"]);
  const collapsedSubgroups = new Set([__sidebarTest.sidebarSubgroupStorageId("initiatives", "cto")]);
  const rows = virtualRowsForAgents(agents, {
    collapsedSections,
    collapsedSubgroups,
    searchActive: true,
  });

  assert.deepEqual(
    rows.filter((row) => row.kind === "agent").map((row) => row.row.agent.member_id),
    ["member:initiative-cto", "member:initiative-liveops"],
  );
  assert.equal(collapsedSections.has("initiatives"), true);
  assert.equal(collapsedSubgroups.has(__sidebarTest.sidebarSubgroupStorageId("initiatives", "cto")), true);
});

test("sidebar pinned agents render in a special Pinned section and leave their configured section", () => {
  const agents = [
    opsAgent({ id: "initiative-alpha", label: "Alpha", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:alpha" }),
    opsAgent({ id: "initiative-beta", label: "Beta", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:beta" }),
    opsAgent({ id: "initiative-gamma", label: "Gamma", group: "initiatives", scope: "liveops", role: "initiative", identity: "initiative:gamma" }),
  ];
  const rows = virtualRowsForAgents(agents, {
    pinnedAgentIds: new Set(["initiative:beta", "initiative:gamma"]),
  });

  assert.deepEqual(
    rows.filter((row) => row.kind === "section").map((row) => [row.bucket, row.count]),
    [
      ["Pinned", 2],
      ["initiatives", 1],
    ],
  );
  assert.deepEqual(
    rows.filter((row) => row.kind === "agent").map((row) => [row.bucket, row.row.subgroup, row.row.agent.identity]),
    [
      ["Pinned", "cto", "initiative:beta"],
      ["Pinned", "liveops", "initiative:gamma"],
      ["initiatives", "cto", "initiative:alpha"],
    ],
  );
});

test("sidebar keeps pinned child agents under their parent by moving the visible hierarchy to Pinned", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["qa-parent-worker"],
  };
  const parentWorker: ConsoleAgent = {
    agent_id: "qa-parent-worker",
    member_id: "qa-parent-worker",
    identity: "qa-parent-worker",
    label: "qa-parent-worker",
    kind: "mob_agent",
    role: "delegate",
    group: "worker",
    wired_to: ["incident-commander", "qa-child-worker"],
  };
  const childWorker: ConsoleAgent = {
    agent_id: "qa-child-worker",
    member_id: "qa-child-worker",
    identity: "qa-child-worker",
    label: "qa-child-worker",
    kind: "mob_agent",
    role: "delegate",
    group: "worker",
    wired_to: ["qa-parent-worker"],
  };
  const grouped = __sidebarTest.groupSidebarAgents([childWorker, commander, parentWorker]);
  const rows = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: __sidebarTest.orderedSectionNames(grouped),
    grouped,
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
    pinnedAgentIds: new Set(["qa-child-worker"]),
  });

  assert.deepEqual(
    rows.filter((row) => row.kind === "agent").map((row) => [row.bucket, row.row.agent.member_id, row.row.depth]),
    [
      ["Pinned", "incident-commander", 0],
      ["Pinned", "qa-parent-worker", 1],
      ["Pinned", "qa-child-worker", 2],
    ],
  );
});

test("sidebar pinned family ids let an ancestor clear descendant pins that keep it in Pinned", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["qa-parent-worker"],
  };
  const parentWorker: ConsoleAgent = {
    agent_id: "qa-parent-worker",
    member_id: "qa-parent-worker",
    identity: "qa-parent-worker",
    label: "qa-parent-worker",
    kind: "mob_agent",
    role: "delegate",
    group: "worker",
    wired_to: ["incident-commander", "qa-child-worker"],
  };
  const childWorker: ConsoleAgent = {
    agent_id: "qa-child-worker",
    member_id: "qa-child-worker",
    identity: "qa-child-worker",
    label: "qa-child-worker",
    kind: "mob_agent",
    role: "delegate",
    group: "worker",
    wired_to: ["qa-parent-worker"],
  };

  assert.deepEqual(
    Array.from(__sidebarTest.sidebarPinnedFamilyPinIds(commander, [childWorker, commander, parentWorker])).sort(),
    [
      "incident-commander",
      "qa-child-worker",
      "qa-parent-worker",
    ],
  );
});

test("sidebar Pinned section can be collapsed and search temporarily expands it", () => {
  const agents = [
    opsAgent({ id: "initiative-alpha", label: "Alpha", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:alpha" }),
    opsAgent({ id: "initiative-beta", label: "Beta", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:beta" }),
  ];
  const collapsed = virtualRowsForAgents(agents, {
    pinnedAgentIds: new Set(["initiative:beta"]),
    collapsedSections: new Set(["Pinned"]),
  });

  assert.deepEqual(
    collapsed.map((row) => row.kind === "section" ? [row.kind, row.bucket, row.collapsed] : [row.kind, row.bucket]),
    [
      ["section", "Pinned", true],
      ["section", "initiatives", false],
      ["agent", "initiatives"],
    ],
  );

  const searchExpanded = virtualRowsForAgents(agents, {
    pinnedAgentIds: new Set(["initiative:beta"]),
    collapsedSections: new Set(["Pinned"]),
    searchActive: true,
  });
  assert.deepEqual(
    searchExpanded.filter((row) => row.kind === "agent").map((row) => [row.bucket, row.row.agent.identity]),
    [
      ["Pinned", "initiative:beta"],
      ["initiatives", "initiative:alpha"],
    ],
  );
});

test("sidebar moves children with a pinned parent so subagents remain under the parent", () => {
  const commander: ConsoleAgent = {
    agent_id: "incident-commander",
    member_id: "incident-commander",
    identity: "incident-commander",
    label: "Incident Commander",
    kind: "mob_agent",
    role: "commander",
    group: "Coordinators",
    wired_to: ["qa-worker"],
  };
  const worker: ConsoleAgent = {
    agent_id: "qa-worker",
    member_id: "qa-worker",
    identity: "qa-worker",
    label: "qa-worker",
    kind: "mob_agent",
    role: "worker",
    group: "worker",
    wired_to: ["incident-commander"],
  };
  const grouped = __sidebarTest.groupSidebarAgents([worker, commander]);
  const rows = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: __sidebarTest.orderedSectionNames(grouped),
    grouped,
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
    pinnedAgentIds: new Set(["incident-commander"]),
  });

  assert.deepEqual(
    rows.filter((row) => row.kind === "agent").map((row) => [row.bucket, row.row.agent.member_id, row.row.depth]),
    [
      ["Pinned", "incident-commander", 0],
      ["Pinned", "qa-worker", 1],
    ],
  );
});

test("sidebar pin ids prefer durable identity and labels.agent_identity before member_id", () => {
  assert.equal(
    __sidebarTest.sidebarAgentPinId(opsAgent({
      id: "durable",
      label: "Durable",
      group: "workers",
      scope: "cto",
      identity: "agent:durable",
    })),
    "agent:durable",
  );
  assert.equal(
    __sidebarTest.sidebarAgentPinId(opsAgent({
      id: "label-identity",
      label: "Label Identity",
      group: "workers",
      scope: "cto",
      labelIdentity: "agent:from-label",
    })),
    "agent:from-label",
  );
  assert.equal(
    __sidebarTest.sidebarAgentPinId(opsAgent({
      id: "member-only",
      label: "Member Only",
      group: "workers",
      scope: "cto",
    })),
    "member:member-only",
  );
});

test("sidebar configured ops-style grouping yields configured sections and scope subgroups", () => {
  const agents = [
    opsAgent({ id: "coord", label: "Coordinator", group: "coordinators", scope: "cto", role: "coordinator" }),
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "initiative-liveops", label: "LiveOps Initiative", group: "initiatives", scope: "liveops", role: "initiative" }),
    opsAgent({ id: "initiative-game-production", label: "Game Production", group: "initiatives", scope: "game-production", role: "initiative" }),
    opsAgent({ id: "initiative-game-platform", label: "Game Platform", group: "initiatives", scope: "game-platform", role: "initiative" }),
    opsAgent({ id: "worker", label: "Worker", group: "workers", scope: "game-platform", role: "worker" }),
    opsAgent({ id: "recipient", label: "Recipient", group: "recipients", scope: "liveops", role: "recipient" }),
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const sections = __sidebarTest.orderedSectionNames(grouped, opsGrouping);
  const rows = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: sections,
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
  });

  assert.deepEqual(sections, ["coordinators", "initiatives", "workers", "recipients"]);
  assert.deepEqual(
    rows
      .filter((row) => row.kind === "subgroup" && row.bucket === "initiatives")
      .map((row) => row.label),
    ["cto", "game-platform", "game-production", "liveops"],
  );
  assert.deepEqual(
    grouped.get("initiatives")?.map((row) => [row.agent.member_id, row.subgroup]),
    [
      ["member:initiative-cto", "cto"],
      ["member:initiative-game-platform", "game-platform"],
      ["member:initiative-game-production", "game-production"],
      ["member:initiative-liveops", "liveops"],
    ],
  );
});

test("stock sidebar renderer flattens its ConsoleNavigationModel", () => {
  const agents = [
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:cto" }),
    opsAgent({ id: "initiative-liveops", label: "LiveOps Initiative", group: "initiatives", scope: "liveops", role: "initiative" }),
    opsAgent({ id: "worker-liveops", label: "LiveOps Worker", group: "workers", scope: "liveops", role: "worker" }),
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const input = {
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set<string>(),
    collapsedSubgroups: new Set<string>(),
    pinnedAgentIds: new Set(["initiative:cto"]),
  };
  const legacyRows = __sidebarTest.buildSidebarVirtualRows(input);
  const model = __sidebarTest.buildStockSidebarNavigationModel(input);
  const renderedRows = __sidebarTest.sidebarNavigationRows(model);

  assert.equal(model.orientation, "vertical");
  assert.deepEqual(
    model.nodes.map((node) => [node.type, node.id, node.label]),
    [
      ["group", "section:__meta_pinned", "Pinned"],
      ["group", "section:initiatives", "initiatives"],
      ["group", "section:workers", "workers"],
    ],
  );
  assert.deepEqual(
    model.nodes[1]?.type === "group"
      ? model.nodes[1].children.map((node) => [node.type, node.id, node.label])
      : [],
    [
      ["group", "subgroup:initiatives:liveops", "liveops"],
    ],
  );
  assert.deepEqual(
    renderedRows.map((row) => row.key),
    legacyRows.map((row) => row.key),
  );
});

test("stock sidebar navigation model keeps ungrouped agents at section level", () => {
  const agents = [
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative" }),
    {
      agent_id: "rt:initiative-unscoped",
      member_id: "member:initiative-unscoped",
      label: "Unscoped Initiative",
      kind: "mob_agent",
      role: "initiative",
      labels: { group: "initiatives" },
    } satisfies ConsoleAgent,
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const model = __sidebarTest.buildStockSidebarNavigationModel({
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set<string>(),
    collapsedSubgroups: new Set<string>(),
  });
  const initiatives = model.nodes.find((node) => node.id === "section:initiatives");
  assert.equal(initiatives?.type, "group");
  assert.deepEqual(
    initiatives?.type === "group"
      ? initiatives.children.map((node) => [node.type, node.id, node.label])
      : [],
    [
      ["group", "subgroup:initiatives:cto", "cto"],
      ["item", "agent:member:initiative-unscoped", "Unscoped Initiative"],
    ],
  );
  const cto = initiatives?.type === "group"
    ? initiatives.children.find((node) => node.id === "subgroup:initiatives:cto")
    : null;
  assert.deepEqual(
    cto?.type === "group" ? cto.children.map((node) => node.id) : [],
    ["agent:member:initiative-cto"],
  );
});

test("stock sidebar navigation model does not collide with a configured Pinned group", () => {
  const agents = [
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:cto" }),
    opsAgent({ id: "literal-pinned", label: "Literal Pinned Group Agent", group: "Pinned", scope: "ops", role: "worker" }),
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const model = __sidebarTest.buildStockSidebarNavigationModel({
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set<string>(),
    collapsedSubgroups: new Set<string>(),
    pinnedAgentIds: new Set(["initiative:cto"]),
  });

  assert.deepEqual(
    model.nodes
      .filter((node) => node.label === "Pinned")
      .map((node) => node.id),
    ["section:__meta_pinned", "section:Pinned"],
  );
  assert.deepEqual(
    __sidebarTest.sidebarNavigationRows(model)
      .filter((row) => row.kind === "agent")
      .map((row) => row.row.agent.member_id),
    ["member:initiative-cto", "member:literal-pinned"],
  );
});

test("sidebar applies persisted section and subgroup drag order without moving Pinned from the top", () => {
  const agents = [
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:cto" }),
    opsAgent({ id: "initiative-cto-two", label: "Second CTO Initiative", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "initiative-liveops", label: "LiveOps Initiative", group: "initiatives", scope: "liveops", role: "initiative" }),
    opsAgent({ id: "worker-liveops", label: "LiveOps Worker", group: "workers", scope: "liveops", role: "worker" }),
    opsAgent({ id: "recipient-cto", label: "CTO Recipient", group: "recipients", scope: "cto", role: "recipient" }),
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const rows = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
    pinnedAgentIds: new Set(["initiative:cto"]),
    sectionOrder: ["recipients", "workers", "initiatives"],
    subgroupOrder: [
      __sidebarTest.sidebarSubgroupStorageId("initiatives", "liveops"),
      __sidebarTest.sidebarSubgroupStorageId("initiatives", "cto"),
    ],
  });

  assert.deepEqual(
    rows.filter((row) => row.kind === "section").map((row) => row.bucket),
    ["Pinned", "recipients", "workers", "initiatives"],
  );
  assert.deepEqual(
    rows
      .filter((row) => row.kind === "subgroup" && row.bucket === "initiatives")
      .map((row) => row.label),
    ["liveops", "cto"],
  );
});

test("sidebar drag preview includes the dragged category or subgroup tree", () => {
  const agents = [
    opsAgent({ id: "initiative-alpha", label: "Alpha Initiative", group: "initiatives", scope: "alpha", role: "initiative" }),
    opsAgent({ id: "worker-alpha", label: "Alpha Worker", group: "workers", scope: "alpha", role: "worker" }),
    opsAgent({ id: "initiative-beta", label: "Beta Initiative", group: "initiatives", scope: "beta", role: "initiative" }),
    opsAgent({ id: "recipient-beta", label: "Beta Recipient", group: "recipients", scope: "beta", role: "recipient" }),
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const rows = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
  });

  assert.deepEqual(
    __sidebarTest.sidebarDragPreviewRows(rows, { kind: "section", id: "initiatives" })
      .map((row) => row.kind === "agent" ? row.row.agent.member_id : `${row.kind}:${row.kind === "subgroup" ? row.label : row.bucket}`),
    [
      "section:initiatives",
      "subgroup:alpha",
      "member:initiative-alpha",
      "subgroup:beta",
      "member:initiative-beta",
    ],
  );
  assert.deepEqual(
    __sidebarTest.sidebarDragPreviewRows(rows, {
      kind: "subgroup",
      id: __sidebarTest.sidebarSubgroupStorageId("initiatives", "alpha"),
      bucket: "initiatives",
    }).map((row) => row.kind === "agent" ? row.row.agent.member_id : `${row.kind}:${row.kind === "subgroup" ? row.label : row.bucket}`),
    ["subgroup:alpha", "member:initiative-alpha"],
  );
});

test("sidebar virtualization computes stable visible ranges with overscan", () => {
  const agents = Array.from({ length: 80 }, (_, index) => opsAgent({
    id: `initiative-${index}`,
    label: `Initiative ${index}`,
    group: "initiatives",
    scope: index % 2 === 0 ? "cto" : "liveops",
    role: "initiative",
  }));
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const rows = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
  });
  const offsets = __sidebarTest.sidebarVirtualOffsets(rows);
  const topRange = __sidebarTest.sidebarVisibleRange({
    rowCount: rows.length,
    offsets: offsets.offsets,
    total: offsets.total,
    scrollTop: 0,
    listHeight: 160,
  });
  const lowerRange = __sidebarTest.sidebarVisibleRange({
    rowCount: rows.length,
    offsets: offsets.offsets,
    total: offsets.total,
    scrollTop: 2400,
    listHeight: 160,
  });

  assert.equal(topRange.start, 0);
  assert.equal(topRange.end < rows.length, true);
  assert.equal(lowerRange.start > topRange.start, true);
  assert.equal(lowerRange.end <= rows.length, true);
  assert.equal(lowerRange.end > lowerRange.start, true);
});

test("sidebar reorder helper preserves unknown new categories at the end", () => {
  const current = __sidebarTest.applySidebarOrder(["alpha", "beta", "gamma"], ["gamma", "alpha"]);
  assert.deepEqual(current, ["gamma", "alpha", "beta"]);
  assert.deepEqual(
    __sidebarTest.reorderSidebarOrder(current, "beta", "gamma", "before"),
    ["beta", "gamma", "alpha"],
  );
});

test("sidebar treats an agent pinned by member_id as pinned even when it has a durable identity", () => {
  const agent = opsAgent({
    id: "ops-lead",
    label: "Ops Lead",
    group: "coordinators",
    scope: "cto",
    identity: "identity:ops-lead",
  });
  // A legacy/default pin stored under the volatile member_id must still match,
  // and unpinning must clear both forms (see ConsoleApp.togglePinnedAgent).
  assert.equal(__sidebarTest.isAgentPinned(agent, new Set(["member:ops-lead"])), true);
  assert.equal(__sidebarTest.isAgentPinned(agent, new Set(["identity:ops-lead"])), true);

  const pinId = __sidebarTest.sidebarAgentPinId(agent);
  const pinned = new Set(["member:ops-lead"]);
  pinned.delete(pinId);
  pinned.delete(agent.member_id);
  assert.equal(__sidebarTest.isAgentPinned(agent, pinned), false);
});

test("sidebar prunes stale-namespace keys for the active scope while preserving other scopes", () => {
  const storage = new MemoryStorage();
  const scope = "runtime-a";
  const activeNamespace = `${encodeURIComponent(scope)}:hash-new`;
  const staleNamespace = `${encodeURIComponent(scope)}:hash-old`;
  const otherScopeNamespace = `${encodeURIComponent("runtime-b")}:hash-new`;

  const activePinsKey = __sidebarTest.sidebarStorageKey(__sidebarTest.SIDEBAR_PINS_STORAGE_PREFIX, activeNamespace);
  const stalePinsKey = __sidebarTest.sidebarStorageKey(__sidebarTest.SIDEBAR_PINS_STORAGE_PREFIX, staleNamespace);
  const staleSectionsKey = __sidebarTest.sidebarStorageKey(__sidebarTest.SECTION_COLLAPSE_STORAGE_PREFIX, staleNamespace);
  const otherScopeKey = __sidebarTest.sidebarStorageKey(__sidebarTest.SUBGROUP_COLLAPSE_STORAGE_PREFIX, otherScopeNamespace);

  __sidebarTest.writeSidebarStringSet(storage, activePinsKey, new Set(["identity:keep"]));
  __sidebarTest.writeSidebarStringSet(storage, stalePinsKey, new Set(["identity:drop"]));
  __sidebarTest.writeSidebarStringSet(storage, staleSectionsKey, new Set(["initiatives"]));
  __sidebarTest.writeSidebarStringSet(storage, otherScopeKey, new Set(["[\"initiatives\",\"cto\"]"]));

  __sidebarTest.pruneStaleSidebarStorage(storage, scope, activeNamespace);

  assert.notEqual(storage.getItem(activePinsKey), null, "active key retained");
  assert.equal(storage.getItem(stalePinsKey), null, "stale pins pruned");
  assert.equal(storage.getItem(staleSectionsKey), null, "stale sections pruned");
  assert.notEqual(storage.getItem(otherScopeKey), null, "other scope preserved");
});

test("sidebar lists a side chat under the agent it branched from", () => {
  const source: ConsoleAgent = {
    agent_id: "alpha",
    member_id: "alpha",
    identity: "alpha",
    label: "Alpha",
    kind: "mob_agent",
    role: "analyst",
    group: "Analysts",
    wired_to: [],
  };
  const sideChat: ConsoleAgent = {
    agent_id: "alpha-side-ab12",
    member_id: "alpha-side-ab12",
    identity: "alpha-side-ab12",
    label: "Alpha (side chat)",
    kind: "mob_agent",
    role: "analyst",
    group: "Analysts",
    wired_to: [],
    side_chat_of: "alpha",
  };
  const peer: ConsoleAgent = {
    agent_id: "beta",
    member_id: "beta",
    identity: "beta",
    label: "Beta",
    kind: "mob_agent",
    role: "analyst",
    group: "Analysts",
    wired_to: [],
  };

  assert.equal(
    __sidebarTest.findSpawnHost(sideChat, [peer, sideChat, source], null)?.member_id,
    "alpha",
  );
  assert.deepEqual(
    __sidebarTest.groupSidebarAgents([sideChat, peer, source]).get("Domains")?.map((row) => [
      row.agent.member_id,
      row.depth,
    ]),
    [
      ["alpha", 0],
      ["alpha-side-ab12", 1],
      ["beta", 0],
    ],
  );
});

function teamAgent(id: string, label: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent {
  return {
    agent_id: id,
    member_id: id,
    identity: `agent:${id}`,
    label,
    kind: "mob_agent",
    role: "analyst",
    group: "Team",
    ...extra,
  };
}

const teamGrouping: ConsoleAgentListConfig = { group_by: ["group"] };

function sortedRows(
  agents: ConsoleAgent[],
  options: {
    grouping?: ConsoleAgentListConfig;
    flat?: boolean;
    sortBy?: "manual" | "name" | "activity";
    order?: string[];
    activity?: Record<string, number>;
    pinnedAgentIds?: Set<string>;
  } = {},
) {
  const order = {
    sortBy: options.sortBy ?? "manual",
    rank: new Map((options.order ?? []).map((key, index) => [key, index] as const)),
    lastActivity: (agent: ConsoleAgent) => options.activity?.[agent.member_id],
  };
  const grouping = options.grouping ?? teamGrouping;
  const grouped = __sidebarTest.groupSidebarAgents(agents, grouping, { flat: options.flat, order });
  const sectionNames = options.flat
    ? (grouped.has(__sidebarTest.FLAT_SECTION_NAME) ? [__sidebarTest.FLAT_SECTION_NAME] : [])
    : __sidebarTest.orderedSectionNames(grouped, grouping);
  return __sidebarTest.buildSidebarVirtualRows({
    sectionNames,
    grouped,
    grouping,
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
    pinnedAgentIds: options.pinnedAgentIds,
    flat: options.flat,
    order,
  });
}

function agentRowIds(rows: ReturnType<typeof sortedRows>): string[] {
  return rows.flatMap((row) => row.kind === "agent" ? [row.row.agent.member_id] : []);
}

test("sidebar manual order with nothing stored keeps today's default order exactly", () => {
  const agents = [
    opsAgent({ id: "coord", label: "Coordinator", group: "coordinators", scope: "cto", role: "coordinator", identity: "coord:main" }),
    opsAgent({ id: "initiative-liveops", label: "LiveOps Initiative", group: "initiatives", scope: "liveops", role: "initiative" }),
    opsAgent({ id: "initiative-cto", label: "CTO Initiative", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:cto" }),
    opsAgent({ id: "initiative-cto-2", label: "Agent 10", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "initiative-cto-3", label: "Agent 2", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "worker-liveops", label: "LiveOps Worker", group: "workers", scope: "liveops", role: "worker" }),
  ];
  const grouped = __sidebarTest.groupSidebarAgents(agents, opsGrouping);
  const legacy = __sidebarTest.buildSidebarVirtualRows({
    sectionNames: __sidebarTest.orderedSectionNames(grouped, opsGrouping),
    grouped,
    grouping: opsGrouping,
    collapsedSections: new Set(),
    collapsedSubgroups: new Set(),
    pinnedAgentIds: new Set(["initiative:cto"]),
  });
  const manual = sortedRows(agents, { grouping: opsGrouping, pinnedAgentIds: new Set(["initiative:cto"]) });

  assert.deepEqual(manual.map((row) => row.key), legacy.map((row) => row.key));
});

test("sidebar manual order puts stored agents first in stored order and new agents after them", () => {
  const agents = [
    teamAgent("a", "Alpha"),
    teamAgent("b", "Bravo"),
    teamAgent("c", "Charlie"),
    teamAgent("d", "Delta"),
  ];

  assert.deepEqual(agentRowIds(sortedRows(agents)), ["a", "b", "c", "d"]);
  assert.deepEqual(agentRowIds(sortedRows(agents, { order: ["agent:c", "agent:gone", "agent:a"] })), ["c", "a", "b", "d"]);
  assert.deepEqual(
    agentRowIds(sortedRows([...agents, teamAgent("e", "Aardvark")], { order: ["agent:d", "agent:c", "agent:b", "agent:a"] })),
    ["d", "c", "b", "a", "e"],
    "a new agent goes to the end of its container",
  );
});

test("sidebar manual order keys agents by durable identity, so a rename keeps its place", () => {
  const renamed = [teamAgent("a", "Zulu"), teamAgent("b", "Bravo")];
  assert.deepEqual(agentRowIds(sortedRows(renamed)), ["b", "a"], "by name Bravo comes first");
  assert.deepEqual(agentRowIds(sortedRows(renamed, { order: ["agent:a", "agent:b"] })), ["a", "b"]);

  const withoutIdentity = [teamAgent("a", "Alpha", { identity: undefined }), teamAgent("b", "Bravo")];
  assert.equal(__sidebarTest.sidebarAgentOrderKey(withoutIdentity[0]!), "a");
  assert.deepEqual(agentRowIds(sortedRows(withoutIdentity, { order: ["agent:b", "a"] })), ["b", "a"]);
  assert.deepEqual(
    agentRowIds(sortedRows([teamAgent("a", "Alpha"), teamAgent("b", "Bravo")], { order: ["agent:b", "a"] })),
    ["b", "a"],
    "a key stored before the agent had an identity still matches its member id",
  );
});

test("sidebar agent moves change only the moved siblings and list every shown agent first", () => {
  const present = ["agent:a", "agent:b", "agent:c", "agent:d", "agent:x", "agent:y"];
  const first = __sidebarTest.sidebarAgentOrderAfterMove({
    stored: [],
    agents: [],
    present,
    siblings: ["agent:a", "agent:b", "agent:c", "agent:d"],
    dragged: "agent:d",
    target: "agent:a",
    where: "before",
    inputSource: "pointer",
  });
  assert.deepEqual(first, ["agent:d", "agent:a", "agent:b", "agent:c", "agent:x", "agent:y"]);

  const second = __sidebarTest.sidebarAgentOrderAfterMove({
    stored: first!,
    agents: [],
    present,
    siblings: ["agent:x", "agent:y"],
    dragged: "agent:y",
    target: "agent:x",
    where: "before",
    inputSource: "keyboard",
  });
  assert.deepEqual(second, ["agent:d", "agent:a", "agent:b", "agent:c", "agent:y", "agent:x"]);

  const keepsAbsent = __sidebarTest.sidebarAgentOrderAfterMove({
    stored: ["agent:gone", "agent:b", "agent:a"],
    agents: [],
    present: ["agent:a", "agent:b"],
    siblings: ["agent:b", "agent:a"],
    dragged: "agent:a",
    target: "agent:b",
    where: "before",
    inputSource: "pointer",
  });
  assert.deepEqual(keepsAbsent, ["agent:gone", "agent:a", "agent:b"]);

  for (const unchanged of [
    { dragged: "agent:a", target: "agent:b", where: "before" as const },
    { dragged: "agent:a", target: "agent:a", where: "after" as const },
    { dragged: "agent:x", target: "agent:a", where: "before" as const },
  ]) {
    assert.equal(__sidebarTest.sidebarAgentOrderAfterMove({
      stored: [],
      agents: [],
      present,
      siblings: ["agent:a", "agent:b"],
      inputSource: "pointer",
      ...unchanged,
    }), null);
  }
});

test("sidebar agent rows share a container only with their siblings", () => {
  const planner = teamAgent("planner", "Planner");
  const agents = [
    planner,
    teamAgent("worker-one", "Worker One", { labels: { spawned_by: "agent:planner" } }),
    teamAgent("worker-two", "Worker Two", { labels: { spawned_by: "agent:planner" } }),
    teamAgent("queue", "Queue"),
  ];
  const rows = sortedRows(agents);
  const containerOf = (id: string) => {
    const row = rows.find((candidate) => candidate.kind === "agent" && candidate.row.agent.member_id === id);
    return row?.kind === "agent" ? row.container : null;
  };

  assert.deepEqual(agentRowIds(rows), ["planner", "worker-one", "worker-two", "queue"]);
  assert.equal(containerOf("planner"), containerOf("queue"));
  assert.equal(containerOf("worker-one"), containerOf("worker-two"));
  assert.notEqual(containerOf("planner"), containerOf("worker-one"));
  assert.equal(containerOf("worker-one"), __sidebarTest.sidebarAgentContainer("Team", null, "planner"));

  assert.deepEqual(
    agentRowIds(sortedRows(agents, { order: ["agent:queue", "agent:planner"] })),
    ["queue", "planner", "worker-one", "worker-two"],
    "a parent carries its subtree",
  );
  assert.deepEqual(
    agentRowIds(sortedRows(agents, { order: ["agent:worker-two", "agent:worker-one"] })),
    ["planner", "worker-two", "worker-one", "queue"],
    "children reorder under their parent",
  );
  const plannerIndex = rows.findIndex((row) => row.kind === "agent" && row.row.agent.member_id === "planner");
  const queueIndex = rows.findIndex((row) => row.kind === "agent" && row.row.agent.member_id === "queue");
  assert.equal(__sidebarTest.agentSubtreeEnd(rows, plannerIndex), queueIndex - 1);
  assert.equal(__sidebarTest.agentSubtreeEnd(rows, queueIndex), queueIndex);
});

test("sidebar keeps a child left in its category under its pinned parent's container", () => {
  // Pinning c1 pins its parent p too, but not its sibling c2. c2 stays in the
  // category, still sorted as p's child, so it trades places only with p's
  // other children there, never with the roots it is shown among.
  const agents = [
    teamAgent("p", "Parent"),
    teamAgent("c1", "Child one", { labels: { spawned_by: "agent:p" } }),
    teamAgent("c2", "Child two", { labels: { spawned_by: "agent:p" } }),
    teamAgent("c3", "Child three", { labels: { spawned_by: "agent:p" } }),
    teamAgent("r1", "Root one"),
    teamAgent("r2", "Root two"),
  ];
  const rows = sortedRows(agents, { pinnedAgentIds: new Set(["agent:c1"]) });
  const containerOf = (key: string) => {
    const row = rows.find((candidate) => candidate.kind === "agent" && candidate.key === key);
    return row?.kind === "agent" ? row.container : null;
  };

  assert.deepEqual(agentRowIds(rows), ["p", "c1", "c3", "c2", "r1", "r2"]);
  assert.equal(containerOf("agent:c2"), __sidebarTest.sidebarAgentContainer("Team", null, "p"));
  assert.equal(containerOf("agent:c3"), containerOf("agent:c2"));
  assert.equal(containerOf("agent:r1"), __sidebarTest.sidebarAgentContainer("Team", null, null));
  assert.equal(containerOf("agent:r2"), containerOf("agent:r1"));
  const roots = rows.flatMap((row) => row.kind === "agent" && row.container === containerOf("agent:r1")
    ? [__sidebarTest.sidebarAgentOrderKey(row.row.agent)]
    : []);
  assert.deepEqual(roots, ["agent:r1", "agent:r2"], "the roots' siblings are the roots alone");
});

// A small seeded generator, so a failure names a reproducible round.
function seededRandom(seed: number): () => number {
  let state = seed;
  return () => {
    state = (state * 1103515245 + 12345) & 0x7fffffff;
    return state / 0x7fffffff;
  };
}

function randomOrderRoster(rand: () => number, count: number): ConsoleAgent[] {
  const pick = <T,>(values: T[]): T => values[Math.floor(rand() * values.length)]!;
  const agents: ConsoleAgent[] = [];
  for (let index = 0; index < count; index += 1) {
    const labels: Record<string, string> = {};
    if (index > 0 && rand() < 0.4) labels.spawned_by = `agent:m${Math.floor(rand() * index)}`;
    if (rand() < 0.4) labels.org = pick(["x", "y"]);
    agents.push({
      agent_id: `m${index}`,
      member_id: `m${index}`,
      identity: `agent:m${index}`,
      label: `Agent ${Math.floor(rand() * 40)}`,
      kind: "mob_agent",
      role: pick(["coordinator", "worker", "router", "lead", "analyst", "helper"]),
      group: pick(["Team", "Ops", "Rest"]),
      labels,
    });
  }
  return agents;
}

// The keys a move lists first, as the component lists them: the category
// order, whichever view is shown.
function categoryOrderKeys(agents: ConsoleAgent[], grouping: ConsoleAgentListConfig, stored: string[] = []): string[] {
  const order = {
    sortBy: "manual" as const,
    rank: new Map(stored.map((key, index) => [key, index] as const)),
    lastActivity: () => undefined,
  };
  const grouped = __sidebarTest.groupSidebarAgents(agents, grouping, { order });
  return __sidebarTest.sidebarCategorySequence(grouped, grouping).map((row) => __sidebarTest.sidebarAgentOrderKey(row.agent));
}

function agentRowKeys(rows: ReturnType<typeof sortedRows>): string[] {
  return rows.flatMap((row) => row.kind === "agent" ? [__sidebarTest.sidebarAgentOrderKey(row.row.agent)] : []);
}

function agentKeysByContainer(rows: ReturnType<typeof sortedRows>): Map<string, string[]> {
  const byContainer = new Map<string, string[]>();
  for (const row of rows) {
    if (row.kind !== "agent") continue;
    if (!byContainer.has(row.container)) byContainer.set(row.container, []);
    byContainer.get(row.container)!.push(__sidebarTest.sidebarAgentOrderKey(row.row.agent));
  }
  return byContainer;
}

// Moves random siblings in random rosters (pins, subgroups, nesting, both
// groupings) and checks that every accepted move lands where it was asked to
// and leaves every other container as it was.
function checkRandomMoves(seed: number, inputSource: "keyboard" | "pointer"): { moves: number; failures: string[] } {
  const rand = seededRandom(seed);
  const pick = <T,>(values: T[]): T => values[Math.floor(rand() * values.length)]!;
  const failures: string[] = [];
  let moves = 0;
  for (let round = 0; round < 300; round += 1) {
    const agents = randomOrderRoster(rand, 4 + Math.floor(rand() * 14));
    const grouping: ConsoleAgentListConfig = rand() < 0.5
      ? { group_by: ["group"] }
      : { group_by: ["group"], subgroup_by: ["labels.org"], collapse_single_subgroup: false };
    const pinnedAgentIds = new Set(rand() < 0.5 ? agents.filter(() => rand() < 0.15).map((agent) => agent.identity!) : []);
    const flat = rand() < 0.3;
    let stored: string[] = [];
    for (let step = 0; step < 6; step += 1) {
      const options = { grouping, pinnedAgentIds, flat };
      const before = agentKeysByContainer(sortedRows(agents, { ...options, order: stored }));
      const present = categoryOrderKeys(agents, grouping, stored);
      const movable = [...before.entries()].filter(([, keys]) => keys.length > 1);
      if (movable.length === 0) break;
      const [container, siblings] = pick(movable);
      const index = Math.floor(rand() * siblings.length);
      const up = rand() < 0.5;
      const target = inputSource === "keyboard"
        ? (up ? siblings[index - 1] : siblings[index + 1])
        : pick(siblings.filter((_, other) => other !== index));
      if (!target) continue;
      const dragged = siblings[index]!;
      const expected = siblings.filter((key) => key !== dragged);
      expected.splice(expected.indexOf(target) + (up ? 0 : 1), 0, dragged);
      const next = __sidebarTest.sidebarAgentOrderAfterMove({
        stored,
        agents,
        present,
        siblings,
        dragged,
        target,
        where: up ? "before" : "after",
        inputSource,
      });
      if (expected.every((key, at) => key === siblings[at])) {
        if (next) failures.push(`round ${round}: a drop that moves nothing wrote an order`);
        continue;
      }
      if (!next) {
        failures.push(`round ${round}: refused a sibling move`);
        break;
      }
      moves += 1;
      const after = agentKeysByContainer(sortedRows(agents, { ...options, order: next }));
      const landed = after.get(container) || [];
      if (landed.join() !== expected.join()) {
        failures.push(`round ${round} step ${step}: ${container} expected ${expected.join()} got ${landed.join()} (pinned ${[...pinnedAgentIds].join()}, flat ${flat})`);
      }
      for (const [other, keys] of before) {
        if (other !== container && (after.get(other) || []).join() !== keys.join()) {
          failures.push(`round ${round} step ${step}: ${other} changed from ${keys.join()} to ${(after.get(other) || []).join()}`);
        }
      }
      stored = next;
    }
  }
  return { moves, failures };
}

test("sidebar keyboard moves in random rosters land as announced and leave other containers alone", () => {
  const { moves, failures } = checkRandomMoves(777, "keyboard");
  assert.ok(moves > 500, `the rosters gave enough moves to check (${moves})`);
  assert.deepEqual(failures.slice(0, 5), []);
});

test("sidebar pointer drops in random rosters land as announced and leave other containers alone", () => {
  const { moves, failures } = checkRandomMoves(4242, "pointer");
  assert.ok(moves > 500, `the rosters gave enough drops to check (${moves})`);
  assert.deepEqual(failures.slice(0, 5), []);
});

test("sidebar a first move in either view flips no pair it did not move in the other view (random rosters)", () => {
  const rand = seededRandom(2718);
  const pick = <T,>(values: T[]): T => values[Math.floor(rand() * values.length)]!;
  const failures: string[] = [];
  let checked = 0;
  for (let round = 0; round < 200; round += 1) {
    const agents = randomOrderRoster(rand, 4 + Math.floor(rand() * 14));
    const grouping: ConsoleAgentListConfig = rand() < 0.5
      ? { group_by: ["group"] }
      : { group_by: ["group"], subgroup_by: ["labels.org"], collapse_single_subgroup: false };
    const pinnedAgentIds = new Set(rand() < 0.5 ? agents.filter(() => rand() < 0.15).map((agent) => agent.identity!) : []);
    const present = categoryOrderKeys(agents, grouping);
    for (const flat of [false, true]) {
      const rows = sortedRows(agents, { grouping, pinnedAgentIds, flat });
      const containers = [...agentKeysByContainer(rows).entries()].filter(([, keys]) => keys.length > 1);
      if (containers.length === 0) continue;
      const [, siblings] = pick(containers);
      const index = Math.floor(rand() * siblings.length);
      const dragged = siblings[index]!;
      const next = __sidebarTest.sidebarAgentOrderAfterMove({
        stored: [],
        agents,
        present,
        siblings,
        dragged,
        target: pick(siblings.filter((_, other) => other !== index)),
        where: rand() < 0.5 ? "before" : "after",
        inputSource: "pointer",
      });
      if (!next) continue;
      // The moved agent and its nested agents move together; every other
      // pair must keep its order in the view not shown.
      const parentOf = new Map(rows.flatMap((row) => row.kind === "agent"
        ? [[row.row.agent.member_id, row.row.parentMemberId] as const]
        : []));
      const draggedId = agents.find((agent) => agent.identity === dragged)!.member_id;
      const movedWith = (id: string): boolean => {
        for (let current: string | null | undefined = id; current; current = parentOf.get(current)) {
          if (current === draggedId) return true;
        }
        return false;
      };
      const otherView = (order: string[]) => sortedRows(agents, { grouping, pinnedAgentIds, flat: !flat, order })
        .flatMap((row) => row.kind === "agent" && !movedWith(row.row.agent.member_id) ? [row.row.agent.member_id] : []);
      checked += 1;
      const before = otherView([]).join();
      const after = otherView(next).join();
      if (before !== after) failures.push(`round ${round}, a move in the ${flat ? "flat" : "category"} view: ${before} became ${after}`);
    }
  }
  assert.ok(checked > 150, `the rosters gave enough first moves to check (${checked})`);
  assert.deepEqual(failures.slice(0, 5), []);
});

test("sidebar Group None lists the category order flattened while nothing is stored", () => {
  const agents = [
    teamAgent("alpha", "Alpha"),
    teamAgent("bravo", "Bravo"),
    teamAgent("mike", "Mike", { group: "Ops" }),
    teamAgent("zulu", "Zulu", { group: "Ops" }),
  ];

  assert.deepEqual(agentRowIds(sortedRows(agents)), ["mike", "zulu", "alpha", "bravo"], "Ops comes before Team");
  assert.deepEqual(agentRowIds(sortedRows(agents, { flat: true })), ["mike", "zulu", "alpha", "bravo"]);
  assert.deepEqual(
    agentRowIds(sortedRows([...agents, teamAgent("echo", "Echo", { group: "Ops" })], { flat: true, order: ["agent:bravo", "agent:alpha"] })),
    ["bravo", "alpha", "echo", "mike", "zulu"],
    "agents not yet stored follow the stored ones in the category order",
  );
});

test("sidebar a first move in either view keeps the other view's order apart from the moved agent", () => {
  const agents = [
    teamAgent("alpha", "Alpha"),
    teamAgent("bravo", "Bravo"),
    teamAgent("mike", "Mike", { group: "Ops" }),
    teamAgent("zulu", "Zulu", { group: "Ops" }),
  ];
  const present = categoryOrderKeys(agents, teamGrouping);
  assert.deepEqual(present, ["agent:mike", "agent:zulu", "agent:alpha", "agent:bravo"]);

  const fromCategory = __sidebarTest.sidebarAgentOrderAfterMove({
    stored: [],
    agents,
    present,
    siblings: ["agent:alpha", "agent:bravo"],
    dragged: "agent:bravo",
    target: "agent:alpha",
    where: "before",
    inputSource: "keyboard",
  });
  assert.deepEqual(agentRowIds(sortedRows(agents, { flat: true, order: fromCategory! })), ["mike", "zulu", "bravo", "alpha"]);

  const fromFlat = __sidebarTest.sidebarAgentOrderAfterMove({
    stored: [],
    agents,
    present,
    siblings: agentRowKeys(sortedRows(agents, { flat: true })),
    dragged: "agent:zulu",
    target: "agent:mike",
    where: "before",
    inputSource: "keyboard",
  });
  assert.deepEqual(agentRowIds(sortedRows(agents, { order: fromFlat! })), ["zulu", "mike", "alpha", "bravo"]);
  assert.deepEqual(agentRowIds(sortedRows(agents, { flat: true, order: fromFlat! })), ["zulu", "mike", "alpha", "bravo"]);
});

test("sidebar an agent stored by member id keeps its place when another container moves", () => {
  // z was stored before it had an identity, so the list holds its member id.
  const agents = [
    teamAgent("b", "Bravo"),
    teamAgent("z", "Zulu"),
    teamAgent("x", "Xray", { group: "Other" }),
    teamAgent("y", "Yankee", { group: "Other" }),
  ];
  const stored = ["z", "agent:b"];
  const team = (order: string[]) => sortedRows(agents, { order }).flatMap((row) =>
    row.kind === "agent" && row.bucket === "Team" ? [row.row.agent.member_id] : []);
  assert.deepEqual(team(stored), ["z", "b"]);

  const next = __sidebarTest.sidebarAgentOrderAfterMove({
    stored,
    agents,
    present: ["agent:x", "agent:y", "agent:z", "agent:b"],
    siblings: ["agent:x", "agent:y"],
    dragged: "agent:y",
    target: "agent:x",
    where: "before",
    inputSource: "keyboard",
  });
  assert.deepEqual(team(next!), ["z", "b"]);
  assert.deepEqual(next, ["agent:z", "agent:b", "agent:y", "agent:x"], "the member id key became the identity key in place");
});

test("sidebar subgroups and pins keep separate containers", () => {
  const agents = [
    opsAgent({ id: "cto-a", label: "CTO A", group: "initiatives", scope: "cto", role: "initiative", identity: "initiative:cto-a" }),
    opsAgent({ id: "cto-b", label: "CTO B", group: "initiatives", scope: "cto", role: "initiative" }),
    opsAgent({ id: "ops-a", label: "Ops A", group: "initiatives", scope: "liveops", role: "initiative" }),
  ];
  const rows = sortedRows(agents, {
    grouping: { ...opsGrouping, collapse_single_subgroup: false },
    pinnedAgentIds: new Set(["initiative:cto-a"]),
  });
  const containers = new Map(rows.flatMap((row) => row.kind === "agent" ? [[row.key, row.container] as const] : []));

  assert.equal(containers.get("agent:Pinned:member:cto-a"), __sidebarTest.sidebarAgentContainer("Pinned", null, null));
  assert.equal(containers.get("agent:member:cto-b"), __sidebarTest.sidebarAgentContainer("initiatives", "cto", null));
  assert.equal(containers.get("agent:member:ops-a"), __sidebarTest.sidebarAgentContainer("initiatives", "liveops", null));
});

test("sidebar Pinned rows follow the manual order across categories", () => {
  const agents = [
    teamAgent("a", "Alpha", { group: "First" }),
    teamAgent("b", "Bravo", { group: "Second" }),
  ];
  const pinnedAgentIds = new Set(["agent:a", "agent:b"]);
  const pinnedIds = (rows: ReturnType<typeof sortedRows>) => rows.flatMap((row) =>
    row.kind === "agent" && row.bucket === "Pinned" ? [row.row.agent.member_id] : []);

  assert.deepEqual(pinnedIds(sortedRows(agents, { pinnedAgentIds })), ["a", "b"]);
  assert.deepEqual(pinnedIds(sortedRows(agents, { pinnedAgentIds, order: ["agent:b", "agent:a"] })), ["b", "a"]);
});

test("sidebar Group None lists every agent in one list, keeping Pinned on top and the nesting", () => {
  const agents = [
    teamAgent("a1", "Alpha", { group: "First" }),
    teamAgent("b1", "Bravo", { group: "Second" }),
    teamAgent("b2", "Bravo helper", { group: "Second", labels: { spawned_by: "agent:b1" } }),
    teamAgent("p", "Pinned one", { group: "First" }),
  ];
  const grouping: ConsoleAgentListConfig = { group_by: ["group"], subgroup_by: ["role"], collapse_single_subgroup: false };

  const flat = sortedRows(agents, { grouping, flat: true });
  assert.deepEqual(flat.filter((row) => row.kind !== "agent").map((row) => row.kind), []);
  assert.deepEqual(agentRowIds(flat), ["a1", "p", "b1", "b2"], "the category order flattened");
  const child = flat.find((row) => row.kind === "agent" && row.row.agent.member_id === "b2");
  assert.equal(child?.kind === "agent" && child.row.depth, 1);

  assert.deepEqual(agentRowIds(sortedRows(agents, { grouping, flat: true, order: ["agent:b1", "agent:a1"] })), ["b1", "b2", "a1", "p"]);

  const withPins = sortedRows(agents, { grouping, flat: true, pinnedAgentIds: new Set(["agent:p"]) });
  assert.deepEqual(
    withPins.filter((row) => row.kind === "section").map((row) => row.kind === "section" ? [row.bucket, row.reorderable] : []),
    [["Pinned", false], [__sidebarTest.FLAT_SECTION_NAME, false]],
  );
  assert.deepEqual(agentRowIds(withPins), ["p", "a1", "b1", "b2"]);
  assert.equal(withPins.some((row) => row.kind === "subgroup"), false);
});

test("sidebar name sort is numeric and case-insensitive and ignores the host-first rule", () => {
  const agents = [
    teamAgent("ten", "Agent 10"),
    teamAgent("two", "agent 2"),
    teamAgent("one", "Agent 1"),
    teamAgent("commander", "Zed commander"),
    teamAgent("same-b", "Same"),
    teamAgent("same-a", "same"),
  ];

  assert.equal(agentRowIds(sortedRows(agents))[0], "commander", "the default order puts the host first");
  assert.deepEqual(agentRowIds(sortedRows(agents, { sortBy: "name" })), ["one", "two", "ten", "same-a", "same-b", "commander"]);
});

test("sidebar last activity sort puts the newest first and unknown last", () => {
  const agents = [
    teamAgent("a", "Alpha"),
    teamAgent("b", "Bravo"),
    teamAgent("c", "Charlie"),
    teamAgent("d", "Delta"),
  ];

  assert.deepEqual(
    agentRowIds(sortedRows(agents, { sortBy: "activity", activity: { a: 100, b: 300, d: 300 } })),
    ["b", "d", "a", "c"],
  );
});

test("sidebar last activity merges the roster's time with newer frames this console saw", () => {
  const frames = [
    { id: "f1", event: "text_delta", identity: "agent:a", timestampMs: 250 },
    { id: "f2", event: "text_delta", identity: "agent:a", timestampMs: 150 },
    { id: "f3", event: "text_delta", identity: "agent:b", timestampMs: 90 },
    { id: "f4", event: "text_delta", timestampMs: 999 },
  ] satisfies ConsoleFrame[];
  const times = __sidebarTest.newestFrameTimes(frames);

  assert.deepEqual(Array.from(times.entries()).sort(), [["agent:a", 250], ["agent:b", 90]]);
  assert.equal(__sidebarTest.agentLastActivity(teamAgent("a", "Alpha", { last_activity_ms: 100 }), times), 250);
  assert.equal(__sidebarTest.agentLastActivity(teamAgent("b", "Bravo", { last_activity_ms: 500 }), times), 500);
  assert.equal(__sidebarTest.agentLastActivity(teamAgent("c", "Charlie", { last_activity_ms: 40 }), times), 40);
  assert.equal(__sidebarTest.agentLastActivity(teamAgent("d", "Delta"), times), undefined);
});

test("sidebar last activity counts only frames of the agent acting, as the server does", () => {
  // History rebuilt from a session and synthesized frames are stamped when
  // the console observes them; lifecycle records, interaction failures and
  // memory events record something done to the agent. None of them is the
  // agent acting, so an idle agent stays as idle as its roster says.
  const frames = [
    { id: "h", event: "runtime_notice_snapshot", sourceKind: "session_history", identity: "agent:a", timestampMs: 1_000_000 },
    { id: "s", event: "text_delta", sourceKind: "synthetic", identity: "agent:a", timestampMs: 1_000_001 },
    { id: "r", event: "identity_retired", sourceKind: "console_event", identity: "agent:a", timestampMs: 1_000_002 },
    { id: "x", event: "interaction_failed", sourceKind: "console_event", identity: "agent:a", timestampMs: 1_000_003 },
    { id: "m", event: "memory.record.promoted", sourceKind: "console_event", identity: "agent:a", timestampMs: 1_000_004 },
    { id: "live", event: "text_delta", sourceKind: "console_event", identity: "agent:b", timestampMs: 300 },
    { id: "send", event: "user_message", sourceKind: "send", identity: "agent:c", timestampMs: 400 },
    { id: "bare", event: "text_delta", identity: "agent:d", timestampMs: 500 },
  ] satisfies ConsoleFrame[];
  const times = __sidebarTest.newestFrameTimes(frames);

  assert.deepEqual(Array.from(times.entries()).sort(), [["agent:b", 300], ["agent:c", 400], ["agent:d", 500]]);
  assert.equal(__sidebarTest.agentLastActivity(teamAgent("a", "Alpha", { last_activity_ms: 10 }), times), 10);
  assert.equal(__sidebarTest.agentLastActivity(teamAgent("b", "Bravo", { last_activity_ms: 10 }), times), 300);
});

test("sidebar row detail is a human line: role or group, then status in words or last activity", () => {
  const now = 10 * 60 * 60 * 1000;
  const minutes = (count: number) => count * 60 * 1000;
  const detail = (agent: ConsoleAgent, lastActivity?: number) => __sidebarTest.agentDetailText(agent, lastActivity, now);

  assert.equal(detail(teamAgent("w", "Worker", { role: "investigation-worker", response_phase: "generating" })), "Investigation worker · working");
  assert.equal(detail(teamAgent("r", "Router", { role: "router", response_phase: null }), now - minutes(3)), "Router · active 3 min ago");
  assert.equal(detail(teamAgent("r", "Router", { role: "router", response_phase: null })), "Router · idle");
  assert.equal(detail(teamAgent("t", "Tool", { role: "router", response_phase: "tool-executing" })), "Router · using a tool");
  assert.equal(detail(teamAgent("x", "Gone", { role: undefined, group: "Delivery", state: "retired" })), "Delivery · retired");
  assert.equal(detail(teamAgent("q", "Quiet", { role: undefined, group: undefined })), "");
  assert.equal(detail(teamAgent("u", "Unknown", { role: undefined, group: undefined }), now - minutes(125)), "Active 2 h ago");
  assert.equal(detail(teamAgent("n", "Raw", { identity: "agent:raw-identity" })).includes("agent:raw-identity"), false);

  assert.equal(__sidebarTest.relativeActivityText(0), "just now");
  assert.equal(__sidebarTest.relativeActivityText(59_000), "just now");
  assert.equal(__sidebarTest.relativeActivityText(60_000), "1 min ago");
  assert.equal(__sidebarTest.relativeActivityText(minutes(60 * 3)), "3 h ago");
  assert.equal(__sidebarTest.relativeActivityText(minutes(60 * 49)), "2 d ago");
  assert.equal(__sidebarTest.relativeActivityText(-5_000), "just now");
});

test("sidebar pins move to the per-user key and read the configuration key only while it is absent", () => {
  const storage = new MemoryStorage();
  const userKey = "meta-console-sidebar-agent-pins:principal-a";
  const configurationKey = "meta-console-sidebar-pins:runtime-a:hash-1";
  storage.setItem(configurationKey, JSON.stringify(["agent:kept"]));

  assert.deepEqual([...readSidebarPins(storage, userKey, configurationKey, ["agent:default"])], ["agent:kept"]);
  assert.deepEqual(JSON.parse(storage.getItem(userKey) || "null"), ["agent:kept"]);

  storage.setItem(configurationKey, JSON.stringify(["agent:later"]));
  storage.setItem(userKey, JSON.stringify([]));
  assert.deepEqual([...readSidebarPins(storage, userKey, configurationKey, ["agent:default"])], [], "an empty per-user set wins");

  const fresh = new MemoryStorage();
  assert.deepEqual([...readSidebarPins(fresh, userKey, configurationKey, ["agent:default"])], ["agent:default"]);
  assert.equal(fresh.getItem(userKey), null, "defaults are not written");
});
