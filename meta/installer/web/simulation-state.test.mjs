import assert from "node:assert/strict";
import test from "node:test";
import { createSimulation, simulationSourceKey } from "./simulation-state.mjs";
import { fingerprint } from "./state.mjs";
import { animationRequestBody, animationRequestMatches, acknowledgeAnimationRefinement } from "./animation-publication.mjs";
import { animationDraftSelection, createAnimationDraftStore } from "./animation-drafts.mjs";

function agent(id, extra = {}) {
  return {
    id,
    name: id.replaceAll("_", " "),
    description: `Continuing responsibility for ${id}`,
    instructions: `Own the ${id} responsibility and preserve evidence.`,
    instances: 1,
    members: [],
    tools: { memory: true, workspace: false, notes: "none", mcp: [] },
    ...extra,
  };
}

function plan(extra = {}) {
  return {
    project: { name: "Test team" },
    agent_classes: [agent("owner")],
    topology: { mode: "manual", entrypoint: "owner:main", edges: [] },
    connections: [],
    knowledge: { mode: "none" },
    flows: [],
    ...extra,
  };
}

function reviewPlan() {
  return plan({
    agent_classes: [agent("owner"), agent("evidence"), agent("finance")],
    topology: { mode: "hub", entrypoint: "owner:main", edges: [] },
    flows: [
      {
        id: "review",
        name: "Review a proposal",
        steps: [
          {
            id: "evidence",
            agent: "evidence:main",
            prompt: "Check original customer evidence.",
            depends_on: [],
          },
          {
            id: "finance",
            agent: "finance:main",
            prompt: "Check original financial assumptions.",
            depends_on: [],
          },
          {
            id: "critique",
            agent: "owner:main",
            prompt: "Review source findings against the original files.",
            depends_on: ["evidence", "finance"],
          },
          {
            id: "verdict",
            agent: "owner:main",
            prompt: "Produce a cited scored verdict.",
            depends_on: ["evidence", "finance", "critique"],
          },
        ],
      },
    ],
  });
}

function assertReferences(model) {
  const nodeIds = new Set(model.nodes.map((node) => node.id));
  const edgeIds = new Set(model.edges.map((edge) => edge.id));
  assert.equal(nodeIds.size, model.nodes.length);
  assert.equal(edgeIds.size, model.edges.length);
  for (const edge of model.edges) {
    assert.ok(nodeIds.has(edge.source), edge.id);
    assert.ok(nodeIds.has(edge.target), edge.id);
  }
  for (const phase of model.phases) {
    assert.ok(
      phase.activeNodeIds.every((id) => nodeIds.has(id)),
      phase.id,
    );
    assert.ok(
      phase.activeEdgeIds.every((id) => edgeIds.has(id)),
      phase.id,
    );
  }
}

for (const sizes of [[1, 512, 64], Array(8).fill(512)]) {
  test(`continuous context retains every named owner without rescanning ${sizes.reduce((a, b) => a + b)} identities per owner`, () => {
    const classes = sizes.map((count, index) => agent(`cohort${index}`, {
      instances: count,
      members: Array.from({ length: count }, (_, member) => ({
        key: `owner${member}`, name: `Context ${index}/${member}`,
        instructions: `Preserve the distinct source context ${index}/${member}.`,
      })),
    }));
    const source = plan({ agent_classes: classes,
      topology: { mode: "manual", entrypoint: "cohort0:owner0", edges: [] } });
    const before = JSON.stringify(source), total = sizes.reduce((a, b) => a + b);
    let classReads = 0;
    // Instrument identities at the existing source projection seam, counting
    // work rather than asserting machine-specific timing or rendered quality.
    const flatMap = classes.flatMap.bind(classes);
    Object.defineProperty(classes, "flatMap", { value: callback => flatMap(callback).map(identity => new Proxy(identity, {
      get(target, key, receiver) {
        if (key === "classId") classReads += 1;
        return Reflect.get(target, key, receiver);
      },
    })) });
    const model = createSimulation(source, "__overview__");
    const owners = model.nodes.filter(node => node.kind === "agent");
    assert.equal(owners.length, total);
    for (const [index, count] of sizes.entries()) {
      for (let member = 0; member < count; member++) {
        const node = owners.find(item => item.id === `cohort${index}:owner${member}`);
        assert.equal(node.name, `Context ${index}/${member}`);
        assert.equal(node.instructions, `${classes[index].instructions}\n\nPreserve the distinct source context ${index}/${member}.`);
      }
    }
    assert.ok(classReads < total * (sizes.length + 10),
      `Member context repeatedly scanned the roster: ${classReads} class reads for ${total} identities.`);
    assert.equal(JSON.stringify(source), before);
    assertReferences(model);
  });
}

function deepFreeze(value) {
  for (const child of Object.values(value || {})) {
    if (child && typeof child === "object") deepFreeze(child);
  }
  return Object.freeze(value);
}

test("confirmed human interface context is exact design evidence, not a Console route or new capability", () => {
  const value = deepFreeze(reviewPlan());
  const fact = deepFreeze({ status: "confirmed", summary: "  Personal browser accounts choose an authorized project.\nNative Console is separate operator inspection. 🐘  ",
    source_message_ids: ["user-interface-choice", "user-operator-choice"], ignored: "not part of the interface fact" });
  const model = createSimulation(value, "__overview__", fact);
  assert.equal(model.interface_context.status, "confirmed");
  assert.equal(model.interface_context.summary, fact.summary);
  assert.deepEqual(model.interface_context.source_message_ids, fact.source_message_ids);
  assert.notEqual(model.interface_context.source_message_ids, fact.source_message_ids);
  assert.match(model.interface_context.interpretation, /design requirements, not verified UI/);
  assert.match(model.interface_context.interpretation, /everyday user interfaces distinct from runtime\/operator Console/);
  assert.equal(Object.hasOwn(model.interface_context, "ignored"), false);
  const boundaries = model.nodes.filter(node => ["input", "output"].includes(node.kind));
  assert.deepEqual(boundaries.map(node => [node.name, node.role]), [["You", "Human interface"], ["Result for you", "Human result boundary"]]);
  assert.ok(boundaries.every(node => /configured/.test(node.instructions)));
  assert.ok(boundaries.every(node => /intended design/.test(node.instructions)));
  assert.ok(boundaries.every(node => !/owner.*unresolved|ownership.*unresolved/.test(node.instructions)));
  assert.equal(model.edges.some(edge => boundaries.some(node => edge.source === node.id || edge.target === node.id)), false);
  assert.doesNotMatch(JSON.stringify(boundaries), /MobKit console|default console conversation|Respond in the console/);
  assert.doesNotMatch(JSON.stringify(model.phases), /Talk to .* in the console|response in your console/);
  const legacy = createSimulation(value, "__overview__");
  assert.deepEqual(model.nodes.filter(node => node.kind === "agent"), legacy.nodes.filter(node => node.kind === "agent"));
  assert.deepEqual(model.edges, legacy.edges);
  assert.equal(model.entrypoint, value.topology.entrypoint);
  assertReferences(model);
  const workflow = createSimulation(value, "review", fact);
  assert.deepEqual(workflow.edges, createSimulation(value, "review").edges);
  assert.doesNotMatch(JSON.stringify(workflow.phases), /in the console|to the console|your console/);
  assertReferences(workflow);
});

test("Console-only requirements remain verbatim context while unknown and unconfirmed callers invent no surface", () => {
  const value = plan(), fact = { status: "confirmed", summary: "Use the native Console conversation directly, without an everyday custom interface.", source_message_ids: ["user-console-choice"] };
  const model = createSimulation(value, undefined, fact);
  assert.equal(model.interface_context.summary, fact.summary);
  assert.equal(model.edges.length, 0, 'Naming an interface does not establish its contact identity');
  for (const input of [undefined, null, {}, { ...fact, status: "proposed" }, { ...fact, status: "unresolved" },
    { ...fact, source_message_ids: [] }, { ...fact, source_message_ids: ["same", "same"] },
    { ...fact, source_message_ids: [1] }, { ...fact, summary: " " }, { ...fact, summary: "🐘".repeat(1601) }]) {
    const unknown = createSimulation(value, undefined, input);
    assert.equal(unknown.interface_context.status, "unresolved");
    assert.equal(unknown.interface_context.summary, null);
    assert.deepEqual(unknown.interface_context.source_message_ids, []);
    assert.match(unknown.phases[0].description, /human interface is not confirmed/);
    assert.doesNotMatch(JSON.stringify(unknown), /MobKit console|default console conversation|Talk to .* in the console/);
  }
  assert.equal(createSimulation(value, undefined, { ...fact, summary: "🐘".repeat(1600) }).interface_context.status, "confirmed");
});

test("an interface-only revision changes actual model/request hashes and refuses a delayed older receipt and draft clear", async () => {
  const value = deepFreeze(plan());
  const a = { status: "confirmed", summary: "Use the native Console for conversation.", source_message_ids: ["user-console"] };
  const b = { status: "confirmed", summary: "Use personal browser accounts for everyday work; Console is operator inspection.", source_message_ids: ["user-browser"] };
  const modelA = createSimulation(value, "__overview__", a), modelB = createSimulation(value, "__overview__", b);
  const modelC = createSimulation(value, "__overview__", { ...b, source_message_ids: ["newer-confirmation"] });
  const streamId = "same-guide:__overview__";
  const requestA = { streamId, proposalKey: simulationSourceKey(value, modelA), attempt: 0 };
  const requestB = { ...requestA, proposalKey: simulationSourceKey(value, modelB) };
  const bodyA = animationRequestBody(value, modelA, "", streamId, "same-guide");
  const bodyB = animationRequestBody(value, modelB, "", streamId, "same-guide");
  const binding = async body => ({ setup_id: body.setup_id, stream_id: body.stream_id,
    blueprint_hash: await fingerprint(body.blueprint), model_hash: await fingerprint(body.model),
    request_fingerprint: await fingerprint({ blueprint: body.blueprint, model: body.model }) });
  const bindingA = await binding(bodyA), bindingB = await binding(bodyB);
  assert.equal(bindingA.blueprint_hash, bindingB.blueprint_hash);
  assert.notEqual(bindingA.model_hash, bindingB.model_hash);
  assert.notEqual(bindingA.request_fingerprint, bindingB.request_fingerprint);
  assert.notEqual(await fingerprint(modelB), await fingerprint(modelC));
  assert.equal(animationRequestMatches(requestA, requestB), false);
  const receiptA = { ...bindingA, id: "a".repeat(64), native_request_key: "b".repeat(64), source_generation: 3, status: "published" };
  const receiptB = { ...bindingB, id: "c".repeat(64), native_request_key: "d".repeat(64), source_generation: 4, status: "queued" };
  const drafts = createAnimationDraftStore(null), selection = animationDraftSelection("same-guide", requestB.proposalKey, "__overview__");
  drafts.attach(selection, await fingerprint({ blueprint: value, model: modelB }), false);
  drafts.update(selection, " Keep my newer browser-route refinement.\n🐘 ");
  const submitted = drafts.capture(selection), before = drafts.read(selection);
  assert.equal(acknowledgeAnimationRefinement(drafts, selection, submitted, receiptA, bindingA, requestA, requestB), null);
  assert.equal(acknowledgeAnimationRefinement(drafts, selection, submitted, receiptA, bindingB, requestB, requestB), null);
  assert.deepEqual(drafts.read(selection), before);
  assert.deepEqual(acknowledgeAnimationRefinement(drafts, selection, submitted, receiptB, bindingB, requestB, requestB), { accepted: true, cleared: true });
  assert.equal(drafts.read(selection).text, "");
});

test("named durable owners remain distinct even with shared class tools", () => {
  const value = plan({
    agent_classes: [
      agent("school", {
        instances: 2,
        members: [
          {
            key: "oak",
            name: "Oak assistant",
            instructions: "Own Oak school cases.",
          },
          {
            key: "pine",
            name: "Pine assistant",
            instructions: "Own Pine school cases.",
          },
        ],
        tools: { workspace: true, memory: true, notes: "read", mcp: [] },
      }),
    ],
    topology: { mode: "manual", entrypoint: "school:pine", edges: [] },
  });
  const model = createSimulation(value);
  const agents = model.nodes.filter((node) => node.kind === "agent");
  assert.deepEqual(
    agents.map((node) => [node.id, node.name]),
    [
      ["school:oak", "Oak assistant"],
      ["school:pine", "Pine assistant"],
    ],
  );
  assert.match(agents[0].instructions, /Own Oak school cases/);
  assert.match(agents[1].instructions, /Own Pine school cases/);
  assert.equal(model.entrypoint, "school:pine");
  assert.equal(model.edges.filter((edge) => edge.kind === "peer").length, 0);
  assert.equal(model.edges.filter((edge) => edge.kind === "tool").length, 4);
  assertReferences(model);
});

test("native DAG phases preserve parallel work and original evidence at synthesis", () => {
  const model = createSimulation(reviewPlan());
  const work = model.phases.filter((phase) => phase.kind === "work");
  assert.deepEqual(
    work.map((phase) => phase.stepIds),
    [["evidence", "finance"], ["critique"], ["verdict"]],
  );
  assert.deepEqual(work[0].activeNodeIds, ["evidence:main", "finance:main"]);
  const verdictInputs = model.edges.filter(
    (edge) => edge.kind === "flow" && edge.targetStepId === "verdict",
  );
  assert.deepEqual(
    verdictInputs.map((edge) => edge.sourceStepId),
    ["evidence", "finance", "critique"],
  );
  const self = verdictInputs.find((edge) => edge.sourceStepId === "critique");
  assert.equal(self.source, "owner:main");
  assert.equal(self.target, "owner:main");
  const finalInputPhase = model.phases.find(
    (phase) => phase.kind === "handoff" && phase.stepIds.includes("verdict"),
  );
  assert.deepEqual(
    finalInputPhase.activeEdgeIds,
    verdictInputs.map((edge) => edge.id),
  );
  assert.equal(model.edges.filter((edge) => edge.kind === "peer").length, 2);
  assert.equal(
    model.edges.filter((edge) => edge.kind === "activation").length,
    2,
  );
  assert.equal(model.edges.filter((edge) => edge.kind === "output").length, 1);
  assertReferences(model);
});

test("an unconfigured human contact does not reach the native default or specialists while the native runner dispatches root tasks", () => {
  const model = createSimulation(reviewPlan());
  const human = model.nodes.find(node => node.kind === 'input');
  const humanLinks = model.edges.filter(edge => edge.source === human.id || edge.target === human.id);
  assert.deepEqual(humanLinks, []);
  assert.deepEqual(model.phases[0].activeNodeIds, [human.id]);
  assert.deepEqual(model.phases[0].activeEdgeIds, []);
  assert.match(human.instructions, /No human contact edge is configured/);
  assert.match(human.instructions, /otherwise leave ownership open/);
  const runner = model.nodes.find(node => node.kind === 'workflow');
  assert.ok(runner);
  assert.deepEqual(model.edges.filter(edge => edge.kind === 'activation').map(edge => [edge.source, edge.target]), [[runner.id, 'evidence:main'], [runner.id, 'finance:main']]);
  assert.equal(model.edges.some(edge => edge.source === 'owner:main' && edge.kind === 'activation'), false);
  assertReferences(model);
});

test("changing the native default and full peer reachability never manufactures human request or delivery routes", () => {
  const value = deepFreeze(plan({
    agent_classes: [agent("operator"), agent("personal"), agent("specialist", { instances: 5 })],
    topology: { mode: "mesh", entrypoint: "operator:main", edges: [] },
  }));
  const fact = deepFreeze({ status: "confirmed", summary: "Personal browser accounts use the everyday application. Native Console is reserved for operator administration.",
    source_message_ids: ["user-personal-ui", "user-admin-ui"] });
  for (const entrypoint of ["operator:main", "personal:main", "specialist:5"]) {
    const current = { ...value, topology: { ...value.topology, entrypoint } };
    const model = createSimulation(current, "__overview__", fact);
    assert.equal(model.entrypoint, entrypoint);
    assert.equal(model.edges.filter(edge => edge.kind === "peer").length, 21);
    const boundaries = model.nodes.filter(node => ["input", "output"].includes(node.kind));
    assert.equal(model.edges.some(edge => boundaries.some(node => edge.source === node.id || edge.target === node.id)), false);
    assert.ok(model.phases.filter(phase => ["input", "output"].includes(phase.kind)).every(phase =>
      phase.activeEdgeIds.length === 0 && phase.activeNodeIds.every(id => boundaries.some(node => node.id === id))));
    const defaultPhase = model.phases.find(phase => phase.kind === "configuration");
    assert.deepEqual(defaultPhase.activeNodeIds, [entrypoint]);
    assert.match(defaultPhase.description, /does not establish who receives/);
    assert.equal(model.interface_context.summary, fact.summary);
    assertReferences(model);
  }
});

test("explicit intended conversation stays available for illustration without configuring a human route", () => {
  const value = deepFreeze(plan({
    agent_classes: [agent("operator"), agent("companion", {
      instructions: "Own the person's ongoing conversation, consult relevant expertise, and return one coherent answer.",
    }), agent("specialist")],
    topology: { mode: "manual", entrypoint: "operator:main", edges: [] },
  }));
  const source = deepFreeze({facts: {
    ownership: {status: "confirmed", summary: "The companion owns the person's conversation and brings back specialist findings.", source_message_ids: ["user-owner"]},
    interfaces: {status: "confirmed", summary: "Use the personal application for questions and answers.", source_message_ids: ["user-interface"]},
  }, requirements: []});
  const before = JSON.stringify([value, source]);
  const withoutBrief = createSimulation(value, "__overview__");
  const model = createSimulation(value, "__overview__", source.facts.interfaces, source);
  assert.equal(model.entrypoint, "operator:main");
  assert.deepEqual(model.nodes.map(node => node.id), withoutBrief.nodes.map(node => node.id));
  assert.deepEqual(model.edges, withoutBrief.edges);
  assert.deepEqual(model.edges, []);
  assert.deepEqual(model.design_context.facts.ownership, source.facts.ownership);
  assert.equal(model.nodes.find(node => node.id === "companion:main").instructions, value.agent_classes[1].instructions);
  for (const node of model.nodes.filter(node => ["input", "output"].includes(node.kind))) {
    assert.match(node.instructions, /current design_context and agent instructions/);
    assert.match(node.instructions, /intended design/);
    assert.doesNotMatch(node.instructions, /owner.*unresolved|ownership.*unresolved/);
  }
  assert.match(model.interface_context.interpretation, /do not establish that intended ownership is unknown/);
  assert.equal(JSON.stringify([value, source]), before);
  assertReferences(model);
});

test("only assigned resources get edges, with exact MCP names and distinct notes authority", () => {
  const value = plan({
    agent_classes: [
      agent("owner", {
        tools: {
          notes: "write",
          memory: true,
          knowledge: true,
          schedule: true,
          mcp: [
            { connection_id: "drive", tool_names: ["get_file", "export_file"] },
          ],
        },
      }),
      agent("reader", {
        tools: { notes: "read", memory: false, knowledge: false, mcp: [] },
      }),
    ],
    connections: [
      { id: "drive", name: "Google Drive", transport: "stdio" },
      { id: "slack", name: "Slack", transport: "http" },
    ],
    knowledge: { mode: "remote" },
  });
  const model = createSimulation(value);
  assert.equal(
    model.nodes.filter((node) => node.kind === "connector").length,
    1,
  );
  assert.equal(
    model.nodes.some((node) => node.name === "Slack"),
    false,
  );
  const knowledge = model.edges.filter(
    (edge) => edge.capability === "knowledge",
  );
  assert.deepEqual(
    knowledge.map((edge) => [edge.source, edge.access]),
    [["owner:main", "read"]],
  );
  assert.deepEqual(
    model.edges
      .filter((edge) => edge.capability === "notes")
      .map((edge) => edge.access),
    ["write", "read"],
  );
  assert.deepEqual(
    model.edges.find((edge) => edge.capability === "mcp").toolNames,
    ["get_file", "export_file"],
  );
  assert.equal(
    model.nodes.some((node) => node.kind === "memory"),
    false,
  );
  assert.match(
    model.nodes.find((node) => node.id === "resource:tool:schedule")
      .instructions,
    /does not install a recurring trigger/,
  );
  assert.match(
    model.nodes.find((node) => node.kind === "knowledge").instructions,
    /separate operator-led ingestion/,
  );
  value.knowledge.mode = "none";
  assert.equal(
    createSimulation(value).nodes.some((node) => node.kind === "knowledge"),
    false,
  );
  assertReferences(model);
});

test("a missing source definition cannot invent a connector node or edge", () => {
  const value = plan({
    agent_classes: [
      agent("owner", {
        tools: { mcp: [{ connection_id: "unknown", tool_names: ["read"] }] },
      }),
    ],
  });
  const model = createSimulation(value);
  assert.equal(
    model.nodes.some((node) => node.kind === "connector"),
    false,
  );
  assert.equal(
    model.edges.some((edge) => edge.capability === "mcp"),
    false,
  );
  assertReferences(model);
});

test("single-agent and disconnected no-flow plans do not fabricate delegation", () => {
  const single = createSimulation(plan());
  assert.equal(single.selectedFlowId, null);
  assert.deepEqual(
    single.edges.map((edge) => edge.kind),
    [],
  );
  assert.deepEqual(
    single.phases.filter((phase) => phase.kind === "work").map(phase => phase.activeNodeIds),
    [],
  );
  assert.deepEqual(single.phases.find(phase => phase.kind === "configuration").activeNodeIds, ["owner:main"]);
  assert.ok(single.phases.every(phase => phase.stepIds.length === 0));
  const disconnected = createSimulation(
    plan({ agent_classes: [agent("owner"), agent("specialist")] }),
  );
  assert.equal(
    disconnected.nodes.filter((node) => node.kind === "agent").length,
    2,
  );
  assert.equal(
    disconnected.edges.some(
      (edge) =>
        edge.source === "specialist:main" || edge.target === "specialist:main",
    ),
    false,
  );
  assertReferences(single);
  assertReferences(disconnected);
});

test("flow selection changes only the selected execution links and has a defined fallback", () => {
  const value = reviewPlan();
  value.flows.push({
    id: "quick",
    name: "Quick response",
    steps: [
      {
        id: "answer",
        agent: "owner:main",
        prompt: "Answer directly.",
        depends_on: [],
      },
    ],
  });
  const model = createSimulation(value, "quick");
  assert.equal(model.selectedFlowId, "quick");
  assert.equal(model.flows.length, 2);
  assert.equal(
    model.edges.some((edge) => edge.kind === "flow"),
    false,
  );
  assert.deepEqual(
    model.phases
      .filter((phase) => phase.kind === "work")
      .map((phase) => phase.stepIds),
    [["answer"]],
  );
  assert.equal(createSimulation(value, "missing").selectedFlowId, "review");
  assertReferences(model);
});

test("preview never mutates or lends nested proposal data", () => {
  const value = reviewPlan();
  value.agent_classes[0].tools.mcp = [
    { connection_id: "drive", tool_names: ["read_file"] },
  ];
  value.connections.push({ id: "drive", name: "Drive", transport: "stdio" });
  const before = JSON.stringify(value);
  deepFreeze(value);
  const model = createSimulation(value);
  model.nodes
    .find((node) => node.id === "owner:main")
    .tools.mcp[0].tool_names.push("write_file");
  model.flows[0].steps[0].depends_on.push("verdict");
  model.edges
    .find((edge) => edge.capability === "mcp")
    .toolNames.push("delete_file");
  assert.equal(JSON.stringify(value), before);
  const again = createSimulation(value);
  assert.deepEqual(
    again.nodes.find((node) => node.id === "owner:main").tools.mcp[0]
      .tool_names,
    ["read_file"],
  );
});

test("64-agent mesh remains finite and keeps every configured identity and link", () => {
  const value = plan({
    agent_classes: Array.from({ length: 4 }, (_, index) =>
      agent(`group_${index}`, { instances: 16 }),
    ),
    topology: { mode: "mesh", entrypoint: "group_0:1", edges: [] },
  });
  const model = createSimulation(value);
  assert.equal(model.nodes.filter((node) => node.kind === "agent").length, 64);
  assert.equal(model.edges.filter((edge) => edge.kind === "peer").length, 2016);
  assert.equal(model.phases.length, 4);
  assertReferences(model);
});

test("empty and invalid DAG inputs end in a finite explicit state", () => {
  assert.deepEqual(createSimulation(null), {
    nodes: [],
    edges: [],
    phases: [],
    flows: [],
    selectedFlowId: null,
    entrypoint: null,
  });
  const value = plan({
    flows: [
      {
        id: "cycle",
        name: "Invalid cycle",
        steps: [
          { id: "a", agent: "owner:main", prompt: "A", depends_on: ["b"] },
          { id: "b", agent: "owner:main", prompt: "B", depends_on: ["a"] },
        ],
      },
    ],
  });
  const model = createSimulation(value);
  assert.equal(model.flows[0].valid, false);
  assert.equal(model.phases.at(-1).kind, "blocked");
  assert.equal(
    model.edges.some((edge) => edge.kind === "flow" || edge.kind === "output"),
    false,
  );
  value.flows[0].steps = [
    { id: "missing", agent: "absent:main", prompt: "Missing", depends_on: [] },
  ];
  assert.equal(createSimulation(value).phases.at(-1).kind, "blocked");
  assertReferences(model);
});
