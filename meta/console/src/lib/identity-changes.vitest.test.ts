/**
 * Changes of long-term agents, the console's pure part: what the gateway's
 * flags offer, the requests a form writes, the outcome and the refusal in
 * words, and the request id of an action. The scripted family answers in
 * the wire's own shapes.
 */
import { describe, expect, it } from "vitest";

import { createScriptedIdentityFamily } from "../../fixtures/scripted-identity-family.cjs";
import { accessActionLabel, isKnownAccessAction } from "./access-actions";
import { agentDisplayTitle } from "./agents";
import { familiesFor } from "../panels/access-model";
import {
  IDENTITY_REFUSED_CODE,
  agentChangeConfirmWords,
  agentIdentityControls,
  agentRefRequest,
  agentShownName,
  archivedAgentControls,
  consentWords,
  createAgentsRequest,
  editDraftOf,
  identityChangeFlags,
  identityRefusalOf,
  instructionOutcomeWords,
  outcomeWords,
  parseAgentPage,
  parseAgentRecord,
  parseIdentityDirectory,
  parseIdentityOutcome,
  readAllPages,
  refusalFormField,
  refusalSentence,
  requestIdFor,
  requesterWords,
  seatWords,
  updateAgentRequest,
  verdictWords,
  type AgentRecord,
  type RequestIdSlot,
} from "./identity-changes";

const DIGEST = "sha256:0000000000000000000000000000000000000000000000000000000000000001";

function record(overrides: Partial<AgentRecord> = {}): AgentRecord {
  return {
    name: "keeper",
    displayName: "The keeper",
    profile: "worker",
    specDigest: DIGEST,
    specRevision: 2,
    archived: false,
    readiness: "dormant",
    openOperations: [],
    instructions: ["Keep the records.", "Answer in one paragraph."],
    context: { desk: "archive" },
    labels: { team: "records", shift: "night" },
    peers: ["helper"],
    ...overrides,
  };
}

describe("the access action words", () => {
  it("names the three permissions of agent changes in words", () => {
    expect(accessActionLabel("agent.identity.create")).toBe("Create long-term agents");
    expect(accessActionLabel("agent.identity.manage")).toBe("Edit and archive long-term agents");
    expect(accessActionLabel("agent.identity.remove")).toBe("Remove long-term agents for good");
    expect(accessActionLabel("agent.identity.*")).toBe("All long-term agent actions");
    for (const action of ["agent.identity.create", "agent.identity.manage", "agent.identity.remove"]) {
      expect(isKnownAccessAction(action)).toBe(true);
    }
    // The Access panel groups them with the other lifecycle permissions.
    expect(familiesFor(["agent.identity.create", "agent.identity.manage", "agent.identity.remove"]).map((family) => [family.id, family.actions]))
      .toEqual([["lifecycle", ["agent.identity.create", "agent.identity.manage", "agent.identity.remove"]]]);
  });
});

describe("what the gateway offers", () => {
  it("reads a flag as on only when the section says true", () => {
    expect(identityChangeFlags(undefined)).toEqual({ available: false, canCreate: false, canChange: false, canRemove: false });
    expect(identityChangeFlags({})).toEqual({ available: false, canCreate: false, canChange: false, canRemove: false });
    expect(identityChangeFlags({ identity_changes: { available: true, can_create: true, can_change: false, can_remove: true } }))
      .toEqual({ available: true, canCreate: true, canChange: false, canRemove: true });
    // A section that is not available offers nothing, whatever else it says.
    expect(identityChangeFlags({ identity_changes: { available: false, can_create: true, can_change: true, can_remove: true } }))
      .toEqual({ available: false, canCreate: false, canChange: false, canRemove: false });
    // Anything but `true` is off.
    expect(identityChangeFlags({ identity_changes: { available: true, can_create: "yes", can_change: 1 } as never }))
      .toEqual({ available: true, canCreate: false, canChange: false, canRemove: false });
  });

  it("offers an agent's controls by the section's flag and the agent's own affordance", () => {
    const all = { available: true, canCreate: true, canChange: true, canRemove: true };
    const both = { can_change_identity: true, can_remove_identity: true };
    expect(agentIdentityControls(all, both)).toEqual({ edit: true, archive: true, remove: true });
    expect(agentIdentityControls(all, { can_change_identity: false, can_remove_identity: true })).toEqual({ edit: false, archive: false, remove: true });
    expect(agentIdentityControls(all, { can_change_identity: true, can_remove_identity: false })).toEqual({ edit: true, archive: true, remove: false });
    expect(agentIdentityControls({ ...all, canChange: false }, both)).toEqual({ edit: false, archive: false, remove: true });
    expect(agentIdentityControls({ ...all, canRemove: false }, both)).toEqual({ edit: true, archive: true, remove: false });
    // An agent whose row carries no affordance (an older gateway) offers none.
    expect(agentIdentityControls(all, undefined)).toEqual({ edit: false, archive: false, remove: false });
    expect(agentIdentityControls(all, {})).toEqual({ edit: false, archive: false, remove: false });
  });

  it("offers an archived agent's controls by the section's flags", () => {
    const all = { available: true, canCreate: true, canChange: true, canRemove: true };
    expect(archivedAgentControls(all)).toEqual({ unarchive: true, remove: true });
    expect(archivedAgentControls({ ...all, canChange: false })).toEqual({ unarchive: false, remove: true });
    expect(archivedAgentControls({ ...all, canRemove: false })).toEqual({ unarchive: true, remove: false });
  });
});

describe("the requests a form writes", () => {
  it("writes a create of one agent with only what the form filled in", () => {
    expect(createAgentsRequest({ profile: "worker", name: " scout ", displayName: "", instructions: "  ", firstMessage: "", peers: [] }))
      .toEqual({ agents: [{ name: "scout", profile: "worker" }] });
    expect(createAgentsRequest({
      profile: "worker", name: "scout", displayName: "Scout", instructions: "Scout the archive.\n\nReport daily.",
      firstMessage: "Introduce yourself.", peers: ["warden", "helper"],
    })).toEqual({
      agents: [{
        name: "scout",
        profile: "worker",
        display_name: "Scout",
        // One text area is one section, as written.
        instructions: ["Scout the archive.\n\nReport daily."],
        first_message: "Introduce yourself.",
        peers: ["warden", "helper"],
      }],
    });
  });

  it("writes an update with only what changed, at the record's spec digest", () => {
    const current = record();
    const draft = editDraftOf(current);
    expect(draft.instructions).toEqual(["Keep the records.", "Answer in one paragraph."]);
    expect(updateAgentRequest(current, draft)).toEqual({
      request: { name: "keeper", expected_spec_digest: DIGEST, set: {} },
      changed: false,
    });
    const built = updateAgentRequest(current, {
      displayName: "Keeper of records",
      instructions: ["Keep them in order.", "   "],
      context: '{"desk": "vault"}',
      labels: [{ key: "team", value: "vault" }, { key: "floor", value: "2" }],
      profile: "reviewer",
      peers: ["warden"],
    });
    expect(built).toEqual({
      request: {
        name: "keeper",
        expected_spec_digest: DIGEST,
        set: {
          display_name: "Keeper of records",
          // A section left empty is dropped.
          instructions: ["Keep them in order."],
          context: { desk: "vault" },
          labels: { team: "vault", floor: "2" },
          labels_remove: ["shift"],
          profile: "reviewer",
        },
        peers_add: ["warden"],
        peers_remove: ["helper"],
      },
      changed: true,
    });
  });

  it("sends a context only when it is another JSON value, and says when it is none", () => {
    const current = record();
    // The same value written another way is no change.
    const same = updateAgentRequest(current, { ...editDraftOf(current), context: '{ "desk":"archive" }' });
    expect("request" in same && same.changed).toBe(false);
    expect(updateAgentRequest(current, { ...editDraftOf(current), context: "{desk}" })).toEqual({ contextError: "The context is not valid JSON." });
    // A context cannot be taken away: an update's absent context is unchanged.
    expect(updateAgentRequest(current, { ...editDraftOf(current), context: "" }))
      .toEqual({ contextError: "A context can be replaced, not removed." });
    const none = record({ context: undefined });
    const untouched = updateAgentRequest(none, editDraftOf(none));
    expect("request" in untouched && untouched.changed).toBe(false);
  });

  it("sends a display name the form emptied, so the gateway says what is wrong with it", () => {
    const current = record();
    const built = updateAgentRequest(current, { ...editDraftOf(current), displayName: " " });
    expect("request" in built && built.request.set).toEqual({ display_name: "" });
  });

  it("treats a display name that is only the agent's name as none", () => {
    // The gateway writes the name where an agent has no display name: the
    // form opens with an empty display name and, left alone, sends none.
    const unnamed = record({ name: "night-desk", displayName: "night-desk" });
    const draft = editDraftOf(unnamed);
    expect(draft.displayName).toBe("");
    const untouched = updateAgentRequest(unnamed, draft);
    expect("request" in untouched && untouched.changed).toBe(false);
    const named = updateAgentRequest(unnamed, { ...draft, displayName: "Night desk" });
    expect("request" in named && named.request.set).toEqual({ display_name: "Night desk" });
  });

  it("names the agent and the digest its record reads for a change of the whole agent", () => {
    expect(agentRefRequest(record())).toEqual({ name: "keeper", expected_spec_digest: DIGEST });
  });
});

describe("the request id of an action", () => {
  it("keeps the id for the same request and takes a new one for a changed request", () => {
    let minted = 0;
    const mint = () => `id-${(minted += 1)}`;
    const first = requestIdFor(null, "create", { agents: [{ name: "scout", profile: "worker" }] }, mint);
    expect(first.requestId).toBe("id-1");
    // The same request again, its members in another order: the same id.
    const again = requestIdFor(first, "create", { agents: [{ profile: "worker", name: "scout" }] }, mint);
    expect(again).toBe(first);
    expect(minted).toBe(1);
    // A changed request is another request.
    const changed = requestIdFor(again, "create", { agents: [{ name: "ranger", profile: "worker" }] }, mint);
    expect(changed.requestId).toBe("id-2");
    // The same body as another change is another request too.
    const slot: RequestIdSlot = requestIdFor(null, "archive", { name: "scout", expected_spec_digest: DIGEST }, mint);
    expect(requestIdFor(slot, "remove", { name: "scout", expected_spec_digest: DIGEST }, mint).requestId).not.toBe(slot.requestId);
  });
});

describe("reading the family's answers", () => {
  it("reads the outcomes the scripted family answers, and its refusal as the one error", () => {
    const family = createScriptedIdentityFamily({ now: () => 1_000 });
    const created = family.handle("meta/identity/create", {
      request_id: "r-1",
      agents: [{ name: "scout", profile: "worker", first_message: "owed: hello" }],
    });
    const outcome = parseIdentityOutcome(created.result);
    expect(outcome).toMatchObject({ outcome: "done", agents: [{ name: "scout", state: "committed", specRevision: 1, seat: "owed" }] });
    expect(outcome && "operationId" in outcome ? outcome.operationId : "").toMatch(/^human\.[0-9a-f]{32}\.r-1$/);

    const held = parseIdentityOutcome(family.handle("meta/identity/create", {
      request_id: "r-2",
      agents: [{ name: "auditor", profile: "reviewer" }],
    }).result);
    expect(held).toMatchObject({ outcome: "pending_decision", decisionId: "decision-1", deadlineMs: 3_601_000 });

    const preparing = parseIdentityOutcome(family.handle("meta/identity/create", {
      request_id: "r-3",
      agents: [{ name: "porter", profile: "worker", labels: { preparation: "host" } }],
    }).result);
    expect(preparing).toMatchObject({ outcome: "pending_preparation", agents: [{ name: "porter", state: "pending_preparation" }] });

    const refused = family.handle("meta/identity/create", { request_id: "r-4", agents: [{ name: "scout", profile: "worker" }] });
    expect(refused.error.code).toBe(IDENTITY_REFUSED_CODE);
    const refusal = identityRefusalOf({ rpcError: refused.error });
    expect(refusal).toMatchObject({ kind: "name_taken", field: "agents[0].name", retryable: false });
    expect(refusal?.detail).toBe(refused.error.message);
    expect(refusal?.data.by).toBe("identity");
    // Another JSON-RPC error is no refusal of the family.
    expect(identityRefusalOf({ rpcError: { code: -32602, message: "no", data: { kind: "invalid" } } })).toBeNull();
    expect(identityRefusalOf(new Error("timeout"))).toBeNull();

    expect(parseIdentityOutcome(family.handle("meta/identity/operation", { request_id: "never" }).result))
      .toMatchObject({ outcome: "unknown" });
    expect(parseIdentityOutcome({ outcome: "refused", kind: "owner_read_only", field: "name", retryable: false, detail: "Change it in configuration." }))
      .toMatchObject({ outcome: "refused", refusal: { kind: "owner_read_only", field: "name", detail: "Change it in configuration." } });
    expect(parseIdentityOutcome({ outcome: "notify" })).toBeNull();
    expect(parseIdentityOutcome({ outcome: "pending_decision", operation_id: "x" })).toBeNull();
  });

  it("reads a record and a page as the family writes them, and keeps no word of how an agent entered", () => {
    const family = createScriptedIdentityFamily();
    const read = parseAgentRecord(family.handle("meta/identity/get", { name: "warden" }).result);
    expect(read).toMatchObject({
      name: "warden",
      displayName: "The warden",
      profile: "worker",
      specRevision: 1,
      archived: false,
      readiness: "dormant",
      instructions: ["Guard the records.", "Answer in one paragraph."],
      labels: { team: "records" },
      peers: ["helper"],
      provenance: { requesterKind: "agent", requester: "helper", atMs: 1_762_000_000_000, consentForm: "allowed" },
    });
    expect(read?.specDigest).toMatch(/^sha256:[0-9a-f]{64}$/);
    expect(JSON.stringify(read)).not.toMatch(/entered|created/);
    // An agent whose record says nothing of who asked reads without it.
    expect(parseAgentRecord(family.handle("meta/identity/get", { name: "helper" }).result)?.provenance).toBeUndefined();
    expect(parseAgentRecord({ name: "x" })).toBeNull();

    family.seed({ name: "oldhand", display_name: "Old hand", archived: true });
    const page = parseAgentPage(family.handle("meta/identity/list", { include_archived: true }).result);
    // The page's order is the gateway's own (here: the order the agents came
    // to exist in), not name order; the console keeps it.
    expect(page.agents.map((agent) => [agent.name, agent.archived])).toEqual([["helper", false], ["warden", false], ["oldhand", true]]);
    expect(page.next).toBeUndefined();
  });

  it("reads the directory's flat verdict and says each in words", () => {
    const directory = parseIdentityDirectory(createScriptedIdentityFamily().handle("meta/identity/directory", {}).result);
    expect(directory.profiles).toEqual([
      { profile: "worker", verdict: "allowed", lines: [] },
      { profile: "reviewer", verdict: "needs_decision", lines: ["A person decides each new reviewer."] },
      { profile: "operator", verdict: "not_allowed", lines: [], reason: "The host's rules keep operators to its configuration." },
    ]);
    expect(directory.peers.map((peer) => [peer.name, peer.displayName, peer.verdict])).toEqual([["helper", "Helper", "allowed"], ["warden", "The warden", "allowed"]]);
    expect(directory.profiles.map((profile) => verdictWords(profile))).toEqual([
      { label: "Allowed", tone: "ok", detail: [], pickable: true },
      { label: "Needs a decision", tone: "warn", detail: ["A person decides each new reviewer."], pickable: true },
      { label: "Not allowed", tone: "neutral", detail: ["The host's rules keep operators to its configuration."], pickable: false },
    ]);
    // A verdict this console does not know is shown as it is and stays pickable.
    expect(verdictWords({ verdict: "probation", lines: [] })).toMatchObject({ label: "probation", pickable: true });
    // The verdict is a member of the entry itself: a nested one is none.
    expect(parseIdentityDirectory({ profiles: [{ profile: "worker", verdict: { verdict: "allowed" } }], peers: [] }).profiles[0].verdict).toBe("");
  });

  it("reads every page of a paged read and stops on a cursor it was already given", async () => {
    const family = createScriptedIdentityFamily({ pageSize: 1 });
    family.seed({ name: "oldhand", archived: true });
    const names = await readAllPages(async (after) => {
      const page = parseAgentPage(family.handle("meta/identity/list", { include_archived: true, ...(after ? { after } : {}) }).result);
      return { items: page.agents.map((agent) => agent.name), next: page.next };
    });
    // Each page is read after the name the page before named, in the
    // gateway's order: nothing is sorted and nothing is skipped.
    expect(names).toEqual(["helper", "warden", "oldhand"]);
    let reads = 0;
    const looped = await readAllPages(async () => {
      reads += 1;
      return { items: [reads], next: "same" };
    });
    expect(looped).toEqual([1, 2]);
  });
});

describe("where a refusal belongs", () => {
  it("maps a path into the create and the update the form wrote onto its field", () => {
    expect(refusalFormField("create", "agents[0].name")).toBe("name");
    expect(refusalFormField("create", "agents[0].profile")).toBe("profile");
    expect(refusalFormField("create", "agents[0].display_name")).toBe("display_name");
    expect(refusalFormField("create", "agents[0].instructions")).toBe("instructions");
    expect(refusalFormField("create", "agents[0].instructions[1]")).toBe("instructions");
    expect(refusalFormField("create", "agents[0].first_message")).toBe("first_message");
    expect(refusalFormField("create", "agents[0].peers[1]")).toBe("peers");
    // The console's create names one agent: another agent's field is none of the form's.
    expect(refusalFormField("create", "agents[1].name")).toBeNull();
    expect(refusalFormField("create", "request_id")).toBeNull();
    expect(refusalFormField("create", undefined)).toBeNull();
    expect(refusalFormField("update", "set.display_name")).toBe("display_name");
    expect(refusalFormField("update", "set.instructions")).toBe("instructions");
    expect(refusalFormField("update", "set.context")).toBe("context");
    expect(refusalFormField("update", "set.labels")).toBe("labels");
    expect(refusalFormField("update", "set.labels_remove")).toBe("labels");
    expect(refusalFormField("update", "set.profile")).toBe("profile");
    expect(refusalFormField("update", "peers_add[0]")).toBe("peers");
    expect(refusalFormField("update", "peers_remove[2]")).toBe("peers");
    // The agent, its digest and the request id are no field of the form.
    expect(refusalFormField("update", "name")).toBeNull();
    expect(refusalFormField("update", "expected_spec_digest")).toBeNull();
    expect(refusalFormField("archive", "name")).toBeNull();
    expect(refusalFormField("update", "agents[0].name")).toBeNull();
  });

  it("says a refusal in the gateway's sentence and never in its kind", () => {
    expect(refusalSentence({ kind: "name_taken", retryable: false, detail: "The name is taken: choose another.", data: {} }))
      .toBe("The name is taken: choose another.");
    expect(refusalSentence({ kind: "name_taken", retryable: false, detail: "", data: {} })).toBe("The change was refused.");
  });
});

describe("an outcome in words", () => {
  const done = (state: string, extra: Record<string, unknown> = {}) =>
    parseIdentityOutcome({ outcome: "done", operation_id: "human.x.r", agents: [{ name: "scout", state, ...extra }] })!;

  it("says what became of an instructions change", () => {
    expect(instructionOutcomeWords(undefined)).toBeNull();
    expect(instructionOutcomeWords({ kind: "applies_at_next_run" })).toContain("reach the agent at its next run");
    expect(instructionOutcomeWords({ kind: "applies_at_next_run" })).toContain("never rewritten");
    expect(instructionOutcomeWords({ kind: "applies_after_compaction" })).toContain("after its next compaction");
    expect(instructionOutcomeWords({ kind: "unchanged" })).toBe("The instructions are unchanged.");
    const reasons: Record<string, string> = {
      prepared_authorization_required: "governed",
      slot_limit_exceeded: "as many instruction records as it can",
      active_set_budget_exceeded: "as large together as they can be",
      lowering_unavailable: "model cannot be given the change",
      body_too_large: "larger than one instruction record holds",
      record_unavailable: "records it at its next start",
    };
    for (const [reason, words] of Object.entries(reasons)) {
      const said = instructionOutcomeWords({ kind: "applies_at_next_session", reason });
      expect(said).toContain("at its next new conversation");
      expect(said).toContain(words);
      expect(said).not.toContain(reason);
    }
    // A reason this console does not know: the outcome without a reason.
    expect(instructionOutcomeWords({ kind: "applies_at_next_session", reason: "later_reason" }))
      .toBe("The new instructions reach the agent at its next new conversation.");
  });

  it("says a create, its owed seat, and each state of a change", () => {
    expect(seatWords("seated")).toBeNull();
    expect(seatWords("owed")).toContain("has not started yet");
    expect(outcomeWords("create", done("committed", { seat: "seated" }), "Scout")).toMatchObject({ tone: "ok", title: "Scout is created" });
    const owed = outcomeWords("create", done("committed", { seat: "owed" }), "Scout");
    expect(owed.lines.join(" ")).toContain("has not started yet");
    expect(owed.lines.join(" ")).not.toContain("owed");
    const updated = outcomeWords("update", done("committed", { instructions: { kind: "applies_at_next_run" } }), "Scout");
    expect(updated.title).toBe("Scout is updated");
    expect(updated.lines.join(" ")).toContain("at its next run");
    expect(outcomeWords("update", done("committed"), "Scout").lines).toEqual([]);
    expect(outcomeWords("archive", done("archived"), "Scout").title).toBe("Scout is archived");
    expect(outcomeWords("archive", done("archiving"), "Scout").title).toBe("Scout is stopping");
    expect(outcomeWords("unarchive", done("restored"), "Scout").title).toBe("Scout is back");
    const removed = outcomeWords("remove", done("removed"), "Scout");
    expect(removed.title).toBe("Scout is removed");
    expect(removed.lines.join(" ")).toContain("can never be used again");
    expect(outcomeWords("remove", done("retiring"), "Scout").lines.join(" ")).toContain("can never be used again");
    expect(outcomeWords("create", parseIdentityOutcome({ outcome: "pending_preparation", operation_id: "x", agents: [{ name: "scout", state: "pending_preparation" }] })!, "Scout"))
      .toMatchObject({ tone: "warn", title: "Scout is being prepared" });
    const pending = outcomeWords("create", parseIdentityOutcome({ outcome: "pending_decision", operation_id: "x", decision_id: "d", deadline_ms: 9 })!, "Scout");
    expect(pending.title).toBe("A person must decide this change");
    expect(pending.lines.join(" ")).toContain("cannot approve your own request");
  });

  it("says who asked and the consent in words", () => {
    const name = (key: string) => (key === 'principal:["human","example","dana"]' ? "dana" : key);
    expect(requesterWords({ requesterKind: "agent", requester: "helper", atMs: 1, consentForm: "allowed" }, name)).toEqual({ kind: "agent", identity: "helper" });
    expect(requesterWords({ requesterKind: "human", requester: 'principal:["human","example","dana"]', atMs: 1, consentForm: "decided" }, name))
      .toEqual({ kind: "person", name: "dana", key: 'principal:["human","example","dana"]' });
    expect(requesterWords({ requesterKind: "host", requester: "console", atMs: 1, consentForm: "allowed" }, name)).toEqual({ kind: "host", words: "This console" });
    expect(requesterWords({ requesterKind: "host", requester: "ops", atMs: 1, consentForm: "allowed" }, name)).toEqual({ kind: "host", words: "The host (ops)" });
    expect(consentWords("allowed")).toBe("Allowed by the host's rules");
    expect(consentWords("decided")).toBe("Decided by a person");
    expect(consentWords("later_form")).toBe("later_form");
  });

  it("shows an agent by its display name, or by its name in words where it has none", () => {
    expect(agentShownName({ name: "warden", displayName: "The warden" })).toBe("The warden");
    // The gateway writes the name where an agent has no display name: the
    // console then names the agent as the roster does, never by the raw name.
    expect(agentShownName({ name: "night-desk", displayName: "night-desk" })).toBe("Night desk");
    expect(agentShownName({ name: "night-desk" })).toBe("Night desk");
    expect(agentShownName({ name: "router:main", displayName: "router:main" }))
      .toBe(agentDisplayTitle({ label: "router:main", identity: "router:main", member_id: "router:main", agent_id: "router:main" }));
  });

  it("asks before a change of the whole agent, and says a removal is for good", () => {
    const remove = agentChangeConfirmWords("remove", "Scout");
    expect(remove.question).toBe("Remove Scout for good?");
    expect(remove.consequence).toContain("can never be used again");
    expect(remove.consequence).toContain("Archive is the reversible choice");
    expect(agentChangeConfirmWords("archive", "Scout").consequence).toContain("keeps its name");
    expect(agentChangeConfirmWords("unarchive", "Scout").confirm).toBe("Unarchive");
  });
});
