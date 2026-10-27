import { test } from "node:test";
import assert from "node:assert/strict";
import {
  guidePlan,
  guideStagedDesign,
  guideDesignBrief,
  setupFacts,
  connectionDetails,
  classCountRequest,
  planGraph,
  toolSummary,
} from "./evolving-plan-state.mjs";

function plan() {
  return {
    project: { name: "Research team" },
    target: "mac",
    gcp: {},
    agent_classes: [
      {
        id: "lead",
        name: "Coordinator",
        description: "Coordinate the work",
        instances: 1,
        tools: { memory: true, notes: "read", mcp: [] },
      },
      {
        id: "research",
        name: "Researcher",
        description: "Check evidence",
        instances: 3,
        tools: {
          memory: true,
          workspace: true,
          knowledge: true,
          mcp: [{ connection_id: "github", tool_names: ["search"] }],
        },
      },
    ],
    topology: { mode: "hub", entrypoint: "lead:main", edges: [] },
    connections: [
      {
        id: "github",
        name: "GitHub",
        source: { registry_id: "io.github/github" },
        required_environment: ["GITHUB_TOKEN"],
        env: { TOKEN: "${GITHUB_TOKEN}" },
      },
      { id: "unused", name: "Unused connector", source: {}, env: {} },
    ],
    knowledge: { mode: "remote" },
    flows: [],
  };
}

test("fresh conversation has no invented plan, and a proposal takes precedence over the applied blueprint", () => {
  assert.equal(guidePlan({ stage: "purpose" }), null);
  const blueprint = plan(),
    proposal = { ...plan(), project: { name: "Revised team" } };
  assert.equal(guidePlan({ blueprint, proposal }), proposal);
  assert.equal(guidePlan({ blueprint, proposal: null }), blueprint);
});

test("a running setup animates its live draft and returns to saved publication when the operation ends", () => {
  const blueprint = plan(),
    proposal = { ...plan(), project: { name: "Previous proposal" } },
    live_proposal = { ...plan(), project: { name: "Streaming revision" } },
    published = { ...plan(), project: { name: "Completed revision" } };
  const running = {
    id: "observatory", version: 1, blueprint, proposal,
    live_design: { guide_id: "observatory", guide_version: 1, request_id: "a".repeat(64), blueprint: live_proposal, brief: {} },
    operation: { status: "running", version: 1, request_id: "a".repeat(64) },
  };
  assert.equal(guidePlan(running), live_proposal);
  assert.equal(guidePlan({ ...running, operation: { ...running.operation, status: "completed" }, proposal: published }), published);
  assert.equal(guidePlan({ ...running, operation: { status: "failed" } }), proposal);
  assert.equal(guidePlan({ ...running, operation: { status: "interrupted" } }), proposal);
  assert.equal(guidePlan({ id: "book-club", blueprint }), blueprint);
});

test("a staged preview owns both halves and cannot mix another operation or manual override", () => {
  const saved = plan(), draft = { ...plan(), project: { name: "Checked draft" } };
  const savedBrief = { facts: {}, requirements: [] }, draftBrief = { facts: { outcome: {
    summary: "A newly understood job", status: "confirmed", source_message_ids: ["answer"] } }, requirements: [] };
  const state = { id: "owned-guide", version: 2, proposal: saved, brief: savedBrief,
    operation: { status: "running", version: 2, request_id: "a".repeat(64) },
    live_design: { guide_id: "owned-guide", guide_version: 2, request_id: "a".repeat(64), blueprint: draft, brief: draftBrief } };
  const original = structuredClone(state);
  assert.equal(guideStagedDesign(state), state.live_design);
  assert.equal(guidePlan(state), draft);
  assert.equal(guideDesignBrief(state), draftBrief);
  assert.equal(guideDesignBrief(state, saved), savedBrief);
  assert.equal(guideDesignBrief(state, structuredClone(draft)), savedBrief, "A separately owned manual plan cannot borrow the stage's context");
  const mutations = [value => value.operation.status = "completed", value => value.operation.status = "failed",
    value => value.version++, value => value.operation.version++, value => value.live_design.guide_version++,
    value => value.live_design.guide_id = "other-guide", value => value.live_design.request_id = "b".repeat(64),
    value => delete value.live_design.brief, value => value.live_design.brief = [],
    value => delete value.live_design.blueprint, value => value.live_design.blueprint = [],
    value => value.operation.request_id = "not-bound", value => value.version = true];
  for (const mutate of mutations) {
    const invalid = structuredClone(state); mutate(invalid);
    assert.equal(guideStagedDesign(invalid), null);
    assert.equal(guidePlan(invalid), invalid.proposal);
    assert.equal(guideDesignBrief(invalid), invalid.brief);
  }
  assert.deepEqual(state, original);
  assert.equal(guidePlan({ ...state, live_design: undefined, live_proposal: draft }), saved,
    "A legacy unpaired stage is not enough to establish the current preview");
});

test("a checked staged pair and its unchanged saved publication keep one animation source", async () => {
  const { createSimulation, simulationSourceKey } = await import("./simulation-state.mjs");
  const draft = plan(), earlierBrief = { facts: {}, requirements: [] };
  const checkedBrief = { facts: { outcome: { summary: "Compare the supplied documents with citations",
    status: "confirmed", source_message_ids: ["current-answer"] } }, requirements: [] };
  const running = { id: "owned-guide", version: 1, proposal: null, brief: earlierBrief,
    operation: { status: "running", version: 1, request_id: "a".repeat(64) }, live_design: {
      guide_id: "owned-guide", guide_version: 1, request_id: "a".repeat(64), blueprint: draft, brief: checkedBrief } };
  const complete = { ...running, version: 2, proposal: draft, brief: checkedBrief,
    operation: { ...running.operation, status: "completed" } };
  const source = state => {
    const blueprint = guidePlan(state), brief = guideDesignBrief(state);
    return simulationSourceKey(blueprint, createSimulation(blueprint, "__overview__", brief.facts?.interfaces, brief));
  };
  assert.equal(source(running), source(complete));
  assert.notEqual(source(running), simulationSourceKey(draft, createSimulation(draft, "__overview__", undefined, earlierBrief)),
    "The old mixed topology/brief request would have introduced another source");
});

test("a request-bound topology awaiting context never borrows the saved brief", async () => {
  const { createSimulation, simulationSourceKey } = await import("./simulation-state.mjs");
  const saved = plan(), draft = { ...plan(), project: { name: "New checked topology" } };
  const savedBrief = { facts: { ownership: { summary: "Earlier owner and permission context",
    status: "confirmed", source_message_ids: ["earlier-answer"] } }, requirements: [] };
  const state = { id: "owned-guide", version: 2, proposal: saved, brief: savedBrief,
    operation: { status: "running", version: 2, request_id: "a".repeat(64) }, live_design: {
      guide_id: "owned-guide", guide_version: 2, request_id: "a".repeat(64),
      blueprint: draft, brief: null, context_pending: true } };
  const original = structuredClone(state);
  assert.equal(guideStagedDesign(state), state.live_design);
  assert.equal(guidePlan(state), draft);
  assert.equal(guideDesignBrief(state), null);
  assert.equal(guideDesignBrief(state, saved), savedBrief);
  const earlyModel = createSimulation(draft, "__overview__", undefined, guideDesignBrief(state));
  assert.equal(Object.hasOwn(earlyModel, "design_context"), false);
  const paired = structuredClone(state);
  delete paired.live_design.context_pending;
  paired.live_design.brief = { facts: { ownership: { summary: "Current owner context",
    status: "confirmed", source_message_ids: ["current-answer"] } }, requirements: [] };
  const currentModel = createSimulation(paired.live_design.blueprint, "__overview__", undefined, guideDesignBrief(paired));
  assert.notEqual(simulationSourceKey(draft, earlyModel), simulationSourceKey(paired.live_design.blueprint, currentModel),
    "The actual current understanding schedules a revised animation source");
  for (const mutate of [value => delete value.live_design.context_pending,
    value => value.live_design.context_pending = false, value => value.live_design.brief = {},
    value => value.live_design.brief = undefined, value => value.operation.status = "completed",
    value => value.operation.request_id = "b".repeat(64), value => value.id = "other-guide",
    value => value.version++, value => value.operation.version++, value => value.live_design.guide_version++]) {
    const invalid = structuredClone(state); mutate(invalid);
    assert.equal(guideStagedDesign(invalid), null);
    assert.equal(guidePlan(invalid), invalid.proposal);
    assert.equal(guideDesignBrief(invalid), invalid.brief);
  }
  assert.deepEqual(state, original);
});

test("draft defaults do not become user decisions before the relevant answers", () => {
  const blueprint = plan();
  const initialFacts = setupFacts({
    proposal: blueprint,
    answers: [{ stage: "purpose" }],
  });
  assert.ok(
    initialFacts
      .filter((fact) => fact.label !== "Current access")
      .every((fact) => fact.pending),
  );
  const answers = [
    { stage: "knowledge" },
    {
      stage: "autonomy",
      selected: ["drafts"],
      question: { options: [{ id: "drafts", label: "Ideas and drafts" }] },
    },
  ];
  const next = setupFacts({ proposal: blueprint, answers });
  assert.equal(next.find((fact) => fact.label === "Runs on").pending, true);
  assert.equal(
    next.find((fact) => fact.label === "Knowledge").value,
    "Existing knowledge service",
  );
  assert.equal(
    next.find((fact) => fact.label === "Your preference").value,
    "Ideas and drafts",
  );
  const ready = setupFacts({ blueprint, status: "ready", answers });
  assert.equal(
    ready.find((fact) => fact.label === "Runs on").value,
    "On your Mac",
  );
});

test("canonical proposed knowledge is shown without promoting it to a confirmed decision", () => {
  const proposal = plan();
  proposal.knowledge.mode = "none";
  for (const agent of proposal.agent_classes)
    agent.tools = { notes: "none", memory: false, knowledge: false };
  const state = {
    proposal,
    completed_stages: ["connections", "autonomy", "destination"],
    answers: [{ stage: "purpose" }],
    brief: { facts: {
      first_version: {
        status: "confirmed", summary: "No knowledge server in this version.",
        source_message_ids: ["initial-request"],
      },
      knowledge: {
        status: "proposed", summary: "Entered information and proposed application records.",
        source_message_ids: ["initial-request"],
      },
    } },
  };
  const before = structuredClone(state);
  assert.deepEqual(setupFacts(state).find((fact) => fact.label === "Knowledge"), {
    label: "Knowledge", value: "Proposed: No shared knowledge service", pending: true,
  });
  assert.equal(planGraph(proposal).nodes.some((node) => node.id === "knowledge:elephant"), false);
  assert.deepEqual(state, before);
});

test("confirmed canonical knowledge choices do not require legacy stage bookkeeping", () => {
  const proposal = plan();
  for (const agent of proposal.agent_classes)
    agent.tools = { notes: "none", memory: false, knowledge: false };
  for (const [mode, value] of [
    ["none", "No shared knowledge service"],
    ["local", "New knowledge service"],
    ["remote", "Existing knowledge service"],
  ]) {
    proposal.knowledge.mode = mode;
    const state = { proposal, brief: { facts: { knowledge: {
      status: "confirmed", summary: "The recorded choice.", source_message_ids: ["user-choice"],
    } } } };
    assert.deepEqual(setupFacts(state).find((fact) => fact.label === "Knowledge"), {
      label: "Knowledge", value, pending: false,
    });
  }
});

test("missing and unresolved knowledge stay unknown even when defaults or unrelated prose mention no server", () => {
  const proposal = plan();
  proposal.knowledge.mode = "none";
  const state = { proposal, brief: { facts: { first_version: {
    status: "confirmed", summary: "No knowledge server.", source_message_ids: ["user-choice"],
  } } } };
  const unknown = { label: "Knowledge", value: "Not decided yet", pending: true };
  assert.deepEqual(setupFacts(state).find((fact) => fact.label === "Knowledge"), unknown);
  const reopened = {
    ...state,
    status: "ready", completed_stages: ["knowledge"], answers: [{ stage: "knowledge" }],
    brief: { facts: { ...state.brief.facts, knowledge: {
      status: "unresolved", summary: "The choice is being reconsidered.", source_message_ids: [],
    } } },
  };
  assert.deepEqual(setupFacts(reopened).find((fact) => fact.label === "Knowledge"), unknown);
  const legacy = setupFacts({ ...state, answers: [{ stage: "knowledge" }] });
  assert.equal(legacy.find((fact) => fact.label === "Knowledge").pending, false);
});

test("reopened hosting and authority override earlier completed choices in the plan summary", () => {
  const proposal = plan();
  proposal.target = "gcp";
  proposal.gcp.project = "proposed-project";
  const state = { proposal, status: "ready", completed_stages: ["destination", "autonomy"],
    answers: [{ stage: "autonomy", selected: ["automatic"], question: {
      options: [{ id: "automatic", label: "Act automatically" }] } }],
    brief: { facts: {
      destination: { status: "unresolved", summary: "Reconsider cloud versus local hosting." },
      authority: { status: "unresolved", summary: "Reconsider which actions need confirmation." },
    } } };
  const before = structuredClone(state);
  const summary = () => setupFacts(state);
  assert.deepEqual(summary().find(fact => fact.label === "Runs on"), {
    label: "Runs on", value: "We'll decide together", pending: true,
  });
  assert.deepEqual(summary().find(fact => fact.label === "Your preference"), {
    label: "Your preference", value: "Not decided yet", pending: true,
  });
  assert.equal(summary().some(fact => fact.label === "Project"), false);
  assert.deepEqual(state, before);

  state.brief.facts.destination.status = "proposed";
  state.brief.facts.authority = { status: "proposed", summary: "Prepare changes, ask before publishing." };
  assert.deepEqual(summary().find(fact => fact.label === "Runs on"), {
    label: "Runs on", value: "Proposed: On Google Cloud", pending: true,
  });
  assert.deepEqual(summary().find(fact => fact.label === "Your preference"), {
    label: "Your preference", value: "Proposed: Prepare changes, ask before publishing.", pending: true,
  });
  assert.equal(summary().some(fact => fact.label === "Project"), false);

  state.brief.facts.destination.status = "confirmed";
  state.brief.facts.authority.status = "confirmed";
  assert.deepEqual(summary().find(fact => fact.label === "Your preference"), {
    label: "Your preference", value: "Prepare changes, ask before publishing.", pending: false,
  });
  assert.deepEqual(summary().find(fact => fact.label === "Project"), {
    label: "Project", value: "proposed-project",
  });
});

test("connection metadata describes installation requirements without claiming live readiness", () => {
  const blueprint = plan(),
    connection = blueprint.connections[0];
  connection.headers = {
    Authorization: "Bearer ${GITHUB_TOKEN}",
    "X-Account": "${GITHUB_ACCOUNT}",
  };
  const detail = connectionDetails(connection, blueprint);
  assert.deepEqual(detail.environment, ["GITHUB_TOKEN", "GITHUB_ACCOUNT"]);
  assert.deepEqual(
    detail.users.map((agent) => agent.id),
    ["research"],
  );
  assert.equal(detail.status, "Setup at install");
  assert.equal(
    connectionDetails(blueprint.connections[1], blueprint).status,
    "Not assigned",
  );
});

test("graph aggregates real class identities and only links actual topology and granted connections", () => {
  const blueprint = plan(),
    graph = planGraph(blueprint);
  assert.deepEqual(
    graph.nodes.map((node) => node.id),
    ["agent:lead", "agent:research", "connection:github", "knowledge:elephant"],
  );
  assert.equal(
    graph.nodes.find((node) => node.id === "agent:research").count,
    3,
  );
  assert.equal(graph.nodes.find((node) => node.entrypoint).id, "agent:lead");
  assert.equal(graph.edges.length, 3);
  assert.equal(
    graph.edges.filter((edge) => edge.kind === "communication").length,
    1,
  );
  assert.equal(
    graph.edges.find((edge) => edge.kind === "connection").a,
    "agent:research",
  );
  assert.equal(graph.grouped, true);
  blueprint.topology = { mode: "manual", entrypoint: "lead:main", edges: [] };
  assert.equal(
    planGraph(blueprint).edges.filter((edge) => edge.kind === "communication")
      .length,
    0,
  );
});

test("manual count controls create a guide request without mutating accepted configuration", () => {
  const agent = plan().agent_classes[1],
    before = JSON.stringify(agent);
  const request = classCountRequest(agent, 4);
  assert.match(request, /Researcher class \(research\) from 3 to 4 agents/);
  assert.match(request, /identities, topology and workflows/);
  assert.equal(JSON.stringify(agent), before);
});

test("tool summary reflects actual grants and no implied shared notes or memory", () => {
  assert.deepEqual(toolSummary({}), []);
  assert.deepEqual(
    toolSummary({ workspace: true, notes: "none", memory: false, mcp: [] }),
    ["Workspace text files"],
  );
  assert.deepEqual(
    toolSummary({
      notes: "read",
      memory: true,
      mcp: [{ connection_id: "github" }],
    }),
    ["Read shared notes", "Conversation memory", "1 app connection"],
  );
});

test("current access reflects overrides even when the original preference was drafts", () => {
  const draft = plan();
  draft.agent_classes[0].tools.shell = true;
  draft.agent_classes[0].tools.builtins = true;
  const facts = setupFacts({ status: "ready", proposal: draft }, draft);
  assert.match(
    facts.find((fact) => fact.label === "Current access").value,
    /Shell, Built-in tools/,
  );
});
