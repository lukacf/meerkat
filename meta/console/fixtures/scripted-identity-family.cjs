"use strict";

// A scripted family of the long-term agent changes and reads the console
// calls (`meta/identity/*`), for the console's own rows: the unit and
// component rows drive it through a transport, the browser row through the
// mock console server. It keeps agents in memory and answers every call in
// the wire's own shapes: a change's typed outcome (`done`,
// `pending_decision`, `pending_preparation`), a refusal as the one JSON-RPC
// error (code -32058, the refusal's sentence as the message, the typed
// refusal as `data`), the flat directory verdict, and the record a read
// answers. The refusal sentences are the gateway's own.

const crypto = require("node:crypto");

const IDENTITY_REFUSED_CODE = -32058;

const SENTENCES = {
  invalid_name: "The name is not a valid agent name: choose another.",
  name_taken: "The name is taken: choose another. A removed agent's name is never available again.",
  profile_unknown: "The mob has no profile of this name: name one of the mob's profiles.",
  peer_unknown: "The peer is no agent you may name: list agents of the mob, or other agents of the same request.",
  change_denied: "The host's rules deny this change for you: it was not made, and repeating it changes nothing.",
  identity_unknown: "No agent has this name: list the agents and name one.",
  stale_spec: "The agent changed since you read it: read it again and send the current spec digest.",
  operation_reused: "This operation id was used with another request: send a new id for a new request, or the same request to read its result.",
  owner_read_only: "This agent is declared by the host's configuration, and a run-time change does not write the host's configuration: change it there, or release the agent from configuration first (adopt), after which it is changed at run time.",
  not_archived: "The agent is not archived: there is nothing to unarchive.",
  decision_not_found: "No such decision.",
  changes_held: "The host holds changes of agents for now: retry the same request later.",
  unsupported: "The request needs something this release does not support: leave it out.",
};

const RETRYABLE = new Set(["changes_held"]);

function refusal(kind, field, members = {}, detail) {
  return {
    kind,
    ...(field ? { field } : {}),
    retryable: RETRYABLE.has(kind),
    detail: detail || SENTENCES[kind] || "The change was refused.",
    ...members,
  };
}

function stable(value) {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${stable(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(text) {
  return `sha256:${crypto.createHash("sha256").update(text).digest("hex")}`;
}

const NAME = /^[a-z][a-z0-9-]{0,62}$/;

/**
 * The family. `options.requester` is the console caller as the gateway
 * attributes it (`{kind: "human", id}` for a signed-in person, `{kind:
 * "host", id: "console"}` otherwise); `options.now` a clock.
 */
function createScriptedIdentityFamily(options = {}) {
  const requester = options.requester || { kind: "human", id: "principal:[\"human\",\"example\",\"dana\"]" };
  const now = options.now || (() => Date.now());
  const flags = { available: true, can_create: true, can_change: true, can_remove: true, ...(options.flags || {}) };
  // Per agent: the two affordances the experience read carries. Absent
  // means both true.
  const affordances = new Map();
  const profiles = options.profiles || [
    { profile: "worker", verdict: "allowed" },
    { profile: "reviewer", verdict: "needs_decision", lines: ["A person decides each new reviewer."] },
    { profile: "operator", verdict: "not_allowed", reason: "The host's rules keep operators to its configuration." },
  ];
  const agents = new Map();
  const tombstones = new Set();
  const operations = new Map();
  const held = new Map();
  const calls = [];
  const scripted = [];
  let decisions = 0;

  function digestOf(agent) {
    return sha256(stable({
      name: agent.name, profile: agent.profile, display_name: agent.display_name ?? null,
      instructions: agent.instructions, context: agent.context ?? null, labels: agent.labels, peers: agent.peers,
    }));
  }

  function seed(agent) {
    const record = {
      name: agent.name,
      display_name: agent.display_name,
      profile: agent.profile || "worker",
      spec_revision: agent.spec_revision ?? 1,
      archived: agent.archived === true,
      readiness: agent.readiness || (agent.archived ? "unseated" : "dormant"),
      provenance: agent.provenance,
      open_operations: agent.open_operations || [],
      instructions: agent.instructions || [],
      context: agent.context,
      labels: agent.labels || {},
      peers: agent.peers || [],
      // Scripted only, never written: a change of this agent is refused
      // `owner_read_only`.
      configured: agent.configured === true,
    };
    agents.set(record.name, record);
    return record;
  }

  for (const agent of options.agents || [
    { name: "helper", display_name: "Helper", profile: "worker", configured: true, instructions: ["Help the team."] },
    {
      name: "warden", display_name: "The warden", profile: "worker",
      instructions: ["Guard the records.", "Answer in one paragraph."],
      labels: { team: "records" }, peers: ["helper"],
      provenance: { entered: "created", requester_kind: "agent", requester: "helper", at_ms: 1_762_000_000_000, consent: { form: "allowed" } },
    },
  ]) seed(agent);

  function operationId(requestId) {
    const namespace = crypto.createHash("sha256").update(`${requester.kind}:${requester.id}`).digest("hex").slice(0, 32);
    return `${requester.kind}.${namespace}.${requestId}`;
  }

  function summary(agent) {
    return {
      name: agent.name,
      // An agent without a display name is written with its name in that
      // member, as the gateway writes it.
      display_name: agent.display_name || agent.name,
      profile: agent.profile,
      spec_digest: digestOf(agent),
      spec_revision: agent.spec_revision,
      archived: agent.archived,
      readiness: agent.readiness,
      ...(agent.provenance ? { provenance: agent.provenance } : {}),
      ...(agent.open_operations.length ? { open_operations: agent.open_operations } : {}),
    };
  }

  function record(agent) {
    return {
      ...summary(agent),
      instructions: agent.instructions,
      ...(agent.context !== undefined ? { context: agent.context } : {}),
      labels: agent.labels,
      peers: agent.peers,
    };
  }

  function outcomeOf(agent, state, extra = {}) {
    return {
      name: agent.name,
      state,
      ...(state === "removed" ? {} : { spec_digest: digestOf(agent), spec_revision: agent.spec_revision }),
      ...extra,
    };
  }

  function provenance() {
    return {
      entered: "created",
      requester_kind: requester.kind,
      requester: requester.id,
      at_ms: now(),
      consent: { form: "allowed" },
    };
  }

  // The agent a change of one agent names, at the digest it read; a refusal
  // otherwise.
  function target(params) {
    const agent = agents.get(params.name);
    if (!agent) return { refused: refusal("identity_unknown", "name") };
    if (agent.configured) return { refused: refusal("owner_read_only", "name") };
    const current = digestOf(agent);
    if (params.expected_spec_digest !== current) {
      return {
        refused: refusal("stale_spec", "expected_spec_digest", {
          current_digest: current, current_revision: agent.spec_revision,
        }),
      };
    }
    return { agent };
  }

  function create(params) {
    const specs = Array.isArray(params.agents) ? params.agents : [];
    if (specs.length === 0) return { refused: refusal("invalid_request", "agents", { problem: "empty" }, "The request names no agent.") };
    for (const [index, spec] of specs.entries()) {
      if (typeof spec.name !== "string" || !NAME.test(spec.name)) return { refused: refusal("invalid_name", `agents[${index}].name`) };
      if (tombstones.has(spec.name)) return { refused: refusal("name_taken", `agents[${index}].name`, { by: "removed" }) };
      if (agents.has(spec.name)) return { refused: refusal("name_taken", `agents[${index}].name`, { by: "identity" }) };
      const profile = profiles.find(entry => entry.profile === spec.profile);
      if (!profile) return { refused: refusal("profile_unknown", `agents[${index}].profile`, { profile: spec.profile }) };
      if (profile.verdict === "not_allowed") return { refused: refusal("change_denied", `agents[${index}].profile`, { change: "create" }) };
      for (const [peerIndex, peer] of (spec.peers || []).entries()) {
        if (!agents.has(peer) && !specs.some(other => other.name === peer)) {
          return { refused: refusal("peer_unknown", `agents[${index}].peers[${peerIndex}]`) };
        }
      }
    }
    const commit = (preparation) => specs.map((spec) => {
      const agent = seed({
        name: spec.name, display_name: spec.display_name, profile: spec.profile,
        instructions: spec.instructions || [], context: spec.context, labels: spec.labels || {},
        peers: spec.peers || [], provenance: provenance(),
        readiness: preparation ? "preparation_pending" : "dormant",
      });
      // A first message that did not start leaves the seat owed; the script
      // says so with a first message that begins with "owed:".
      const owed = typeof spec.first_message === "string" && spec.first_message.startsWith("owed:");
      if (owed) agent.readiness = "unseated";
      return outcomeOf(agent, preparation ? "pending_preparation" : "committed", { seat: owed ? "owed" : "seated" });
    });
    const needsDecision = specs.some(spec => profiles.find(entry => entry.profile === spec.profile)?.verdict === "needs_decision");
    if (needsDecision) return { hold: () => commit(false) };
    if (specs.some(spec => spec.labels && spec.labels.preparation === "host")) return { preparation: commit(true) };
    return { agents: commit(false) };
  }

  function update(params) {
    const found = target(params);
    if (found.refused) return found;
    const agent = found.agent;
    const set = params.set || {};
    if (set.profile !== undefined && !profiles.some(entry => entry.profile === set.profile)) {
      return { refused: refusal("profile_unknown", "set.profile", { profile: set.profile }) };
    }
    for (const [index, peer] of (params.peers_add || []).entries()) {
      if (!agents.has(peer)) return { refused: refusal("peer_unknown", `peers_add[${index}]`) };
    }
    for (const key of set.labels_remove || []) {
      if (set.labels && Object.hasOwn(set.labels, key)) {
        return { refused: refusal("invalid_request", "set.labels_remove", { problem: "conflicting" }, "A label is both set and removed.") };
      }
    }
    const before = stable(agent.instructions);
    if (set.display_name !== undefined) agent.display_name = set.display_name;
    if (set.instructions !== undefined) agent.instructions = set.instructions;
    if (set.context !== undefined) agent.context = set.context;
    if (set.labels !== undefined) agent.labels = { ...agent.labels, ...set.labels };
    for (const key of set.labels_remove || []) delete agent.labels[key];
    if (set.profile !== undefined) agent.profile = set.profile;
    agent.peers = [...agent.peers.filter(peer => !(params.peers_remove || []).includes(peer)), ...(params.peers_add || []).filter(peer => !agent.peers.includes(peer))];
    agent.spec_revision += 1;
    // What became of an instructions change: the script's answer for this
    // agent, the next run by default, `unchanged` for the same text.
    const instructions = set.instructions === undefined
      ? undefined
      : before === stable(agent.instructions)
        ? { kind: "unchanged" }
        : (agent.instruction_outcome || { kind: "applies_at_next_run" });
    return { agents: [outcomeOf(agent, "committed", instructions ? { instructions } : {})] };
  }

  function whole(params, change) {
    const found = target(params);
    if (found.refused) return found;
    const agent = found.agent;
    if (change === "archive") {
      agent.archived = true;
      agent.readiness = "unseated";
      return { agents: [outcomeOf(agent, "archived")] };
    }
    if (change === "unarchive") {
      if (!agent.archived) return { refused: refusal("not_archived", "name") };
      agent.archived = false;
      agent.readiness = "dormant";
      return { agents: [outcomeOf(agent, "restored")] };
    }
    agents.delete(agent.name);
    tombstones.add(agent.name);
    for (const other of agents.values()) other.peers = other.peers.filter(peer => peer !== agent.name);
    return { agents: [outcomeOf(agent, "removed")] };
  }

  function change(method, params) {
    const { request_id: requestId, ...body } = params;
    if (typeof requestId !== "string" || !requestId) {
      return { error: refusal("invalid_request", "request_id", { problem: "empty" },
        "A change names its operation: send a request_id, and the same one again to read the change's result.") };
    }
    const id = operationId(requestId);
    const fingerprint = stable({ method, body });
    const known = operations.get(id);
    if (known) {
      if (known.fingerprint !== fingerprint) return { error: refusal("operation_reused", "request_id") };
      // The same request under the same id answers the same state again and
      // writes nothing.
      return { result: known.outcome, replayed: true };
    }
    const done = method === "meta/identity/create" ? create(body)
      : method === "meta/identity/update" ? update(body)
        : whole(body, method.slice("meta/identity/".length));
    if (done.refused) return { error: done.refused };
    let outcome;
    if (done.hold) {
      decisions += 1;
      const decisionId = `decision-${decisions}`;
      const deadlineMs = now() + 3_600_000;
      outcome = { outcome: "pending_decision", operation_id: id, decision_id: decisionId, deadline_ms: deadlineMs };
      held.set(id, { decisionId, deadlineMs, kind: `identity.${method.slice("meta/identity/".length)}`, commit: done.hold });
    } else if (done.preparation) {
      outcome = { outcome: "pending_preparation", operation_id: id, agents: done.preparation };
    } else {
      outcome = { outcome: "done", operation_id: id, agents: done.agents };
    }
    operations.set(id, { fingerprint, outcome });
    return { result: outcome };
  }

  function operationOf(params) {
    if (typeof params.request_id === "string" && params.operation_id === undefined) return operationId(params.request_id);
    if (typeof params.operation_id === "string" && params.request_id === undefined) return params.operation_id;
    return null;
  }

  // A page of a read: one stable order that `after` continues. The order is
  // the one the agents came to exist in, not name order, as the gateway's
  // own order is none a caller can compute.
  function page(entries, after, key) {
    const rest = after ? entries.slice(entries.findIndex(entry => entry[key] === after) + 1) : entries;
    const size = options.pageSize || 100;
    const shown = rest.slice(0, size);
    return { shown, next: rest.length > size ? shown[shown.length - 1][key] : undefined };
  }

  function answer(method, params) {
    switch (method) {
      case "meta/identity/create":
      case "meta/identity/update":
      case "meta/identity/archive":
      case "meta/identity/unarchive":
      case "meta/identity/remove":
        return change(method, params);
      case "meta/identity/withdraw": {
        const id = operationOf(params);
        const decision = id && held.get(id);
        if (!decision) return { error: refusal("decision_not_found") };
        held.delete(id);
        operations.delete(id);
        return { result: { operation_id: id, decision_id: decision.decisionId } };
      }
      case "meta/identity/operation": {
        const id = operationOf(params);
        if (!id) return { error: refusal("invalid_request", "request_id", { problem: "conflicting" }, "Name the operation by request_id or by operation_id.") };
        const known = operations.get(id);
        return { result: known ? known.outcome : { outcome: "unknown", operation_id: id } };
      }
      case "meta/identity/get": {
        const agent = agents.get(params.name);
        return agent ? { result: record(agent) } : { error: refusal("identity_unknown", "name") };
      }
      case "meta/identity/list": {
        const listed = [...agents.values()]
          .filter(agent => params.include_archived === true || !agent.archived)
          .filter(agent => !params.profile || agent.profile === params.profile)
          .map(summary);
        const { shown, next } = page(listed, params.after, "name");
        return { result: { agents: shown, ...(next ? { next } : {}) } };
      }
      case "meta/identity/directory": {
        const peers = [...agents.values()]
          .filter(agent => !agent.archived)
          .map(agent => ({
            name: agent.name,
            display_name: agent.display_name || agent.name,
            profile: agent.profile,
            verdict: "allowed",
          }));
        const { shown, next } = page(peers, params.after, "name");
        return { result: { profiles: params.after ? [] : profiles, peers: shown, ...(next ? { next } : {}) } };
      }
      default:
        return null;
    }
  }

  return {
    IDENTITY_REFUSED_CODE,
    flags,
    calls,
    agents,
    /** The methods this family serves, as `meta/capabilities` lists them. */
    methods: [
      "meta/identity/create", "meta/identity/update", "meta/identity/archive", "meta/identity/unarchive",
      "meta/identity/remove", "meta/identity/withdraw", "meta/identity/operation", "meta/identity/get",
      "meta/identity/list", "meta/identity/directory",
    ],
    seed,
    /** Set one agent's two affordances as the experience read carries them. */
    setAffordances(name, value) { affordances.set(name, value); },
    /** What became of the next instructions change of `name`. */
    setInstructionOutcome(name, outcome) { agents.get(name).instruction_outcome = outcome; },
    /**
     * Script the next call of `method`: `{refuse: {kind, field?, detail?,
     * ...}}` answers that refusal and writes nothing, `{fail: "after"}`
     * carries the call out and then loses the answer, `{fail: "before"}`
     * loses the call.
     */
    scriptNext(method, script) { scripted.push({ method, script }); },
    /** Decide a held change as a person would: it commits, or ends refused. */
    decide(decisionId, approve = true) {
      for (const [id, decision] of held) {
        if (decision.decisionId !== decisionId) continue;
        held.delete(id);
        const known = operations.get(id);
        if (approve) known.outcome = { outcome: "done", operation_id: id, agents: decision.commit() };
        else operations.delete(id);
        return true;
      }
      return false;
    },
    /**
     * The held changes as `meta/decisions/list` answers them to their
     * requester: a status row each (where the decision stands, never what
     * it holds).
     */
    decisionStatuses() {
      return [...held.values()].map(decision => ({
        decision_id: decision.decisionId,
        kind: decision.kind,
        phase: "open",
        deadline_at_ms: decision.deadlineMs,
      }));
    },
    /**
     * One call, as the console RPC answers it: `{result}`, `{error: {code,
     * message, data}}`, `{lost: true}` for a scripted transport loss, or
     * null for a method outside the family.
     */
    handle(method, params = {}) {
      if (!method.startsWith("meta/identity/")) return null;
      // The console transport names its caller on every command; the
      // gateway takes that member out before the strict parse.
      const { expected_caller_scope: _scope, ...own } = params;
      calls.push({ method, params: own });
      const index = scripted.findIndex(entry => entry.method === method);
      const script = index >= 0 ? scripted.splice(index, 1)[0].script : null;
      if (script?.fail === "before") return { lost: true };
      if (script?.refuse) {
        const { kind, field, detail, ...members } = script.refuse;
        const data = refusal(kind, field, members, detail);
        return { error: { code: IDENTITY_REFUSED_CODE, message: data.detail, data } };
      }
      const answered = answer(method, own);
      if (!answered) return null;
      if (script?.fail === "after") return { lost: true };
      if (answered.error) return { error: { code: IDENTITY_REFUSED_CODE, message: answered.error.detail, data: answered.error } };
      return { result: answered.result };
    },
    /** The roster rows of the family's agents with a member (not archived). */
    experienceAgents() {
      return [...agents.values()].filter(agent => !agent.archived).map((agent) => {
        const own = affordances.get(agent.name) || {};
        return {
          agent_id: agent.name,
          member_id: agent.name,
          identity: agent.name,
          label: agent.display_name || agent.name,
          kind: "identity",
          profile: agent.profile,
          role: agent.profile,
          state: "active",
          addressable: true,
          // Each affordance is the script's own answer for the agent, so a
          // row can turn one input off and leave every other on.
          affordances: {
            can_send_message: true,
            can_change_identity: own.can_change_identity !== false,
            can_remove_identity: own.can_remove_identity !== false,
          },
        };
      });
    },
    /** The section the experience read carries: each flag as scripted. */
    experienceSection() {
      return {
        available: flags.available,
        can_create: flags.can_create,
        can_change: flags.can_change,
        can_remove: flags.can_remove,
      };
    },
  };
}

module.exports = { createScriptedIdentityFamily, IDENTITY_REFUSED_CODE };
