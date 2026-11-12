/**
 * Changes of long-term agents, the console's pure part: what the gateway's
 * flags offer, the requests a form writes, the outcome and the refusal in
 * words, and the request id of an action. The scripted family answers in
 * the wire's own shapes.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { createScriptedIdentityFamily } from "../../fixtures/scripted-identity-family.cjs";
import { accessActionLabel, isKnownAccessAction } from "./access-actions";
import { agentDisplayTitle } from "./agents";
import { familiesFor } from "../panels/access-model";
import {
  IDENTITY_REFUSED_CODE,
  REACH_CHOICES,
  REFUSAL_KINDS_WITH_A_SENTENCE,
  RUN_MODE_CHOICES,
  UNKNOWN_REFUSAL_SENTENCE,
  refusalDetail,
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
  keepFirstSchedules,
  outcomeWords,
  parseAgentPage,
  parseAgentRecord,
  parseChangePlan,
  parseIdentityDirectory,
  parseIdentityOutcome,
  planWords,
  reachWords,
  readAllPages,
  readRefusalSentence,
  refusalFormField,
  refusalSentence,
  requestIdFor,
  requesterWords,
  runModeWords,
  schedulesWords,
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

  it("opens Edit with an empty display name for an agent that has none", () => {
    // The record of an agent without a display name carries no such member:
    // the form opens empty and, left alone, sends none.
    const unnamed = record({ name: "night-desk", displayName: undefined });
    const draft = editDraftOf(unnamed);
    expect(draft.displayName).toBe("");
    const untouched = updateAgentRequest(unnamed, draft);
    expect("request" in untouched && untouched.changed).toBe(false);
    const named = updateAgentRequest(unnamed, { ...draft, displayName: "Night desk" });
    expect("request" in named && named.request.set).toEqual({ display_name: "Night desk" });
    // A display name is a display name, whatever it says: one that reads
    // like the agent's name opens as it is.
    expect(editDraftOf(record({ name: "night-desk", displayName: "night-desk" })).displayName).toBe("night-desk");
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

  it("reads a plan as the family writes it and says its outcome in words", () => {
    const family = createScriptedIdentityFamily();
    const digest = family.handle("meta/identity/get", { name: "warden" }).result.spec_digest;
    const ask = (profile: string) => family.handle("meta/identity/plan", {
      change: "update", request_id: `plan-${profile}`, name: "warden", expected_spec_digest: digest, set: { profile },
    });
    expect(parseChangePlan(ask("operator").result)).toEqual({ outcome: "allow" });
    family.setProfileChange("reviewer", "require_approval");
    expect(parseChangePlan(ask("reviewer").result)).toEqual({ outcome: "require_approval" });
    expect(parseChangePlan({ change: "profile" })).toBeNull();
    expect(parseChangePlan(null)).toBeNull();
    expect(planWords({ outcome: "allow" })).toEqual({ tone: "ok", label: "Allowed", line: "The host's rules allow this change." });
    // A change its host is only told about is made like an allowed one.
    expect(planWords({ outcome: "notify" })).toEqual(planWords({ outcome: "allow" }));
    expect(planWords({ outcome: "require_approval" }))
      .toEqual({ tone: "warn", label: "Needs a decision", line: "A person must decide this change before it is made." });
    // An outcome this console does not know says nothing: no raw token.
    expect(planWords({ outcome: "probation" })).toBeNull();
    // A plan writes nothing and holds nothing.
    expect(family.agents.get("warden").profile).toBe("worker");
    expect(family.decisionStatuses()).toEqual([]);
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

  it("says each refusal in the console's own sentence, and keeps the gateway's for the details", () => {
    // The gateway's vocabulary of refusal kinds is closed: the console has
    // a sentence of its own for every kind of it.
    const source = readFileSync(resolve(__dirname, "../../../crates/meerkat-meta/src/identity_creation/refusal.rs"), "utf8");
    const vocabulary = /closed_tokens!\(IdentityRefusalKind \{([^}]*)\}\);/.exec(source);
    const kinds = Array.from((vocabulary?.[1] ?? "").matchAll(/=> "([a-z_]+)"/g)).map((match) => match[1]!);
    expect(kinds.length).toBeGreaterThan(50);
    expect(kinds.filter((kind) => !REFUSAL_KINDS_WITH_A_SENTENCE.includes(kind))).toEqual([]);
    // Beside them only the gateway's other answers and the console's own.
    expect(REFUSAL_KINDS_WITH_A_SENTENCE.filter((kind) => !kinds.includes(kind)).sort())
      .toEqual(["access_denied", "caller_unknown", "no_decision_store", "not_offered", "read_only"]);
    const refusal = (kind: string, detail = "") => ({ kind, retryable: false, detail, data: {} });
    for (const kind of REFUSAL_KINDS_WITH_A_SENTENCE) {
      const sentence = refusalSentence(refusal(kind, "The gateway's own words about the mob and its request_id."));
      // A sentence of the console: a person's words, never the gateway's,
      // never the kind, and nothing a person cannot act on.
      expect(sentence).toMatch(/^[A-Z].*\.$/);
      expect(sentence).not.toBe(UNKNOWN_REFUSAL_SENTENCE);
      expect(sentence).not.toContain(kind);
      expect(sentence).not.toMatch(/\bmobs?\b|request.?id|operation.?id|digest|\badopt|sha256|\bspec\b|\bgateway\b|_/i);
    }
    expect(refusalSentence(refusal("name_taken", "The name is taken: choose another.")))
      .toBe("This name is taken. Choose another; a removed agent's name is never free again.");
    expect(refusalSentence(refusal("stale_spec"))).toBe("This agent changed since you opened it. Read it again, then make your change.");
    expect(refusalSentence(refusal("owner_read_only"))).toBe("This agent is set up in the host's configuration and cannot be changed here. Change it in the configuration.");
    // A kind the console does not know, or none: the plain sentence.
    expect(refusalSentence(refusal("quota_exceeded", "A sentence of a later gateway."))).toBe(UNKNOWN_REFUSAL_SENTENCE);
    expect(refusalSentence(refusal(""))).toBe(UNKNOWN_REFUSAL_SENTENCE);
    expect(refusalSentence(refusal("constructor"))).toBe(UNKNOWN_REFUSAL_SENTENCE);
    expect(UNKNOWN_REFUSAL_SENTENCE).toBe("The request was refused, and nothing was changed.");
    // What the details hold is the gateway's sentence, whole; none where it
    // gave none.
    expect(refusalDetail(refusal("name_taken", "The name is taken: choose another."))).toBe("The name is taken: choose another.");
    expect(refusalDetail(refusal("not_offered"))).toBeNull();
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
    // A person is named, and the person's key is not carried on.
    expect(requesterWords({ requesterKind: "human", requester: 'principal:["human","example","dana"]', atMs: 1, consentForm: "decided" }, name))
      .toEqual({ kind: "person", name: "dana" });
    expect(requesterWords({ requesterKind: "host", requester: "console", atMs: 1, consentForm: "allowed" }, name)).toEqual({ kind: "host", words: "This console" });
    expect(requesterWords({ requesterKind: "host", requester: "ops", atMs: 1, consentForm: "allowed" }, name)).toEqual({ kind: "host", words: "The host (ops)" });
    expect(consentWords("allowed")).toBe("Allowed by the host's rules");
    expect(consentWords("decided")).toBe("Decided by a person");
    expect(consentWords("later_form")).toBe("later_form");
  });

  it("shows an agent by its display name, or by its name in words where it has none", () => {
    expect(agentShownName({ name: "warden", displayName: "The warden" })).toBe("The warden");
    // An agent without a display name reads as the roster names it, never
    // by the raw name.
    expect(agentShownName({ name: "night-desk" })).toBe("Night desk");
    expect(agentShownName({ name: "router:main" }))
      .toBe(agentDisplayTitle({ label: "router:main", identity: "router:main", member_id: "router:main", agent_id: "router:main" }));
    // A display name is shown as the gateway gives it.
    expect(agentShownName({ name: "night-desk", displayName: "night-desk" })).toBe("night-desk");
  });

  it("asks before a change of the whole agent, and says a removal is for good", () => {
    const remove = agentChangeConfirmWords("remove", "Scout");
    expect(remove.question).toBe("Remove Scout for good?");
    expect(remove.consequence).toContain("can never be used again");
    expect(remove.consequence).toContain("Archive is the reversible choice");
    expect(agentChangeConfirmWords("archive", "Scout").consequence).toContain("keeps its name");
    expect(agentChangeConfirmWords("unarchive", "Scout").confirm).toBe("Unarchive");
  });

  it("says before an archive and a removal that the agent's last conversation is harvested", () => {
    // An agent that is archived or removed leaves as any agent leaves: its
    // last conversation is harvested. An unarchive harvests nothing.
    const harvest = "Its last conversation is harvested, as for any agent that leaves the team.";
    expect(agentChangeConfirmWords("archive", "Scout").consequence).toContain(harvest);
    expect(agentChangeConfirmWords("remove", "Scout").consequence).toContain(harvest);
    expect(agentChangeConfirmWords("unarchive", "Scout").consequence).not.toContain("harvest");
  });
});

describe("who reaches an agent and how it runs", () => {
  it("reads both from a record only where the record says so", () => {
    const family = createScriptedIdentityFamily();
    family.seed({ name: "clerk", profile: "worker", addressability: "internal_only", runtime_mode_override: "turn_driven" });
    const clerk = parseAgentRecord(family.handle("meta/identity/get", { name: "clerk" }).result);
    expect(clerk).toMatchObject({ addressability: "internal_only", runtimeMode: "turn_driven" });
    // An agent that can be messaged and runs as its profile says: its
    // record writes neither, and the console reads neither.
    const warden = parseAgentRecord(family.handle("meta/identity/get", { name: "warden" }).result);
    expect(warden !== null && "addressability" in warden).toBe(false);
    expect(warden !== null && "runtimeMode" in warden).toBe(false);
    // A page of the list carries both the same way.
    const page = parseAgentPage(family.handle("meta/identity/list", {}).result);
    expect(page.agents.find((agent) => agent.name === "clerk")).toMatchObject({ addressability: "internal_only", runtimeMode: "turn_driven" });
    // The form opens on what the record says.
    expect(editDraftOf(clerk!)).toMatchObject({ addressability: "internal_only", runtimeMode: "turn_driven" });
    expect(editDraftOf(warden!)).toMatchObject({ addressability: "addressable", runtimeMode: "" });
  });

  it("writes both on a create only when they are not the default, and on an update only when they changed", () => {
    const draft = { profile: "worker", name: "clerk", displayName: "", instructions: "", firstMessage: "", peers: [], addressability: "addressable", runtimeMode: "" };
    expect(createAgentsRequest(draft)).toEqual({ agents: [{ name: "clerk", profile: "worker" }] });
    expect(createAgentsRequest({ ...draft, addressability: "internal_only", runtimeMode: "autonomous_host" }))
      .toEqual({ agents: [{ name: "clerk", profile: "worker", addressability: "internal_only", runtime_mode_override: "autonomous_host" }] });

    const open = record();
    const same = updateAgentRequest(open, editDraftOf(open));
    expect(same).toMatchObject({ changed: false, request: { set: {} } });
    const changed = updateAgentRequest(open, { ...editDraftOf(open), addressability: "internal_only", runtimeMode: "turn_driven" });
    expect(changed).toMatchObject({
      changed: true,
      request: { name: "keeper", expected_spec_digest: DIGEST, set: { addressability: "internal_only", runtime_mode_override: "turn_driven" } },
    });
    // Back to an agent that can be messaged: the wire's own word for it.
    const internal = record({ addressability: "internal_only", runtimeMode: "turn_driven" });
    expect(updateAgentRequest(internal, { ...editDraftOf(internal), addressability: "addressable" }))
      .toMatchObject({ changed: true, request: { set: { addressability: "addressable" } } });
    expect(updateAgentRequest(internal, { ...editDraftOf(internal), runtimeMode: "autonomous_host" }))
      .toMatchObject({ changed: true, request: { set: { runtime_mode_override: "autonomous_host" } } });
    // An update cannot hand the way an agent runs back to its profile: a
    // form that names none sets none.
    const handedBack = updateAgentRequest(internal, { ...editDraftOf(internal), runtimeMode: "" });
    expect(handedBack).toMatchObject({ changed: false, request: { set: {} } });
  });

  it("says both in words, offers the choices a form lists, and names the form field of a refusal", () => {
    expect(reachWords(undefined)).toBe("Can receive messages");
    expect(reachWords("addressable")).toBe("Can receive messages");
    expect(reachWords("internal_only")).toBe("Internal only");
    expect(runModeWords(undefined)).toBe("As its profile says");
    expect(runModeWords("autonomous_host")).toBe("On its own");
    expect(runModeWords("turn_driven")).toBe("Turn by turn");
    // A value a later gateway writes reads in words, never as a token.
    expect(runModeWords("later_mode")).toBe("Later mode");
    expect(REACH_CHOICES.map((choice) => [choice.value, choice.label]))
      .toEqual([["addressable", "Can receive messages"], ["internal_only", "Internal only"]]);
    expect(RUN_MODE_CHOICES.map((choice) => [choice.value, choice.label]))
      .toEqual([["", "As its profile says"], ["autonomous_host", "On its own"], ["turn_driven", "Turn by turn"]]);
    for (const choice of [...REACH_CHOICES, ...RUN_MODE_CHOICES]) {
      expect(choice.hint).toMatch(/^[A-Z].*\.$/);
      expect(`${choice.label} ${choice.hint}`).not.toMatch(/_|addressab|autonomous|turn.driven|\bhost\b/i);
    }
    expect(refusalFormField("create", "agents[0].addressability")).toBe("addressability");
    expect(refusalFormField("create", "agents[0].runtime_mode_override")).toBe("runtime_mode");
    expect(refusalFormField("update", "set.addressability")).toBe("addressability");
    expect(refusalFormField("update", "set.runtime_mode_override")).toBe("runtime_mode");
  });
});

describe("what a change did to an agent's schedules", () => {
  const followed = (effect: string, schedules: Array<{ schedule_id: string; name?: string }>, extra: Record<string, unknown> = {}) => {
    const outcome = parseIdentityOutcome({
      outcome: "done",
      operation_id: "human.x.r",
      agents: [{ name: "scout", state: "archived", schedules: { effect, count: schedules.length, schedules, ...extra } }],
    });
    return outcome && outcome.outcome === "done" ? outcome.agents[0]!.schedules : undefined;
  };
  const morning = { schedule_id: "sched-01", name: "Morning report" };
  const weekly = { schedule_id: "sched-02", name: "Weekly digest" };
  const unnamed = { schedule_id: "sched-03" };

  it("reads the member of a first answer and says it with the schedules' names", () => {
    expect(followed("paused", [morning, weekly])).toEqual({
      effect: "paused",
      count: 2,
      schedules: [{ scheduleId: "sched-01", name: "Morning report" }, { scheduleId: "sched-02", name: "Weekly digest" }],
      incomplete: false,
    });
    expect(schedulesWords(followed("paused", [morning]))).toEqual(["Its schedule Morning report is paused until it is unarchived."]);
    expect(schedulesWords(followed("paused", [morning, weekly])))
      .toEqual(["Its 2 schedules are paused until it is unarchived: Morning report, Weekly digest."]);
    expect(schedulesWords(followed("resumed", [morning]))).toEqual(["Its schedule Morning report runs again."]);
    expect(schedulesWords(followed("resumed", [morning, weekly]))).toEqual(["Its 2 schedules run again: Morning report, Weekly digest."]);
    expect(schedulesWords(followed("ended", [morning]))).toEqual(["Its schedule Morning report is ended."]);
    expect(schedulesWords(followed("ended", [morning, weekly]))).toEqual(["Its 2 schedules are ended: Morning report, Weekly digest."]);
    // A schedule without a name is counted, never shown by its id.
    expect(schedulesWords(followed("ended", [morning, unnamed, weekly])))
      .toEqual(["Its 3 schedules are ended: Morning report, Weekly digest and 1 without a name."]);
    expect(schedulesWords(followed("paused", [unnamed]))).toEqual(["Its schedule is paused until it is unarchived."]);
    expect(schedulesWords(followed("paused", [unnamed, { schedule_id: "sched-04" }]))).toEqual(["Its 2 schedules are paused until it is unarchived."]);
    for (const effect of ["paused", "resumed", "ended"]) {
      expect(schedulesWords(followed(effect, [morning, unnamed])).join(" ")).not.toMatch(/sched-0\d|_/);
    }
    // A change that had no schedule to follow, an answer without the
    // member (a replay, a launch without schedules) and an effect this
    // console does not know say nothing.
    expect(schedulesWords(followed("paused", []))).toEqual([]);
    expect(schedulesWords(undefined)).toEqual([]);
    expect(schedulesWords(followed("rescheduled", [morning]))).toEqual([]);
    const plain = parseIdentityOutcome({ outcome: "done", operation_id: "x", agents: [{ name: "scout", state: "archived" }] });
    expect(plain && plain.outcome === "done" ? "schedules" in plain.agents[0]! : true).toBe(false);
  });

  it("says when not every schedule could be followed", () => {
    expect(schedulesWords(followed("paused", [morning], { incomplete: true }))).toEqual([
      "Its schedule Morning report is paused until it is unarchived.",
      "Not every schedule could be changed. Try again to follow the rest.",
    ]);
    expect(schedulesWords(followed("ended", [], { incomplete: true }))).toEqual(["Its schedules could not be changed. Try again to follow them."]);
    expect(followed("paused", [morning], { incomplete: true })?.incomplete).toBe(true);
    // Only `true` says so.
    expect(followed("paused", [morning], { incomplete: "yes" })?.incomplete).toBe(false);
  });

  it("says a change's outcome with what became of the schedules, the ids apart", () => {
    const outcome = parseIdentityOutcome({
      outcome: "done",
      operation_id: "human.x.r",
      agents: [{ name: "scout", state: "archived", schedules: { effect: "paused", count: 2, schedules: [morning, unnamed] } }],
    })!;
    const words = outcomeWords("archive", outcome, "Scout");
    expect(words.title).toBe("Scout is archived");
    expect(words.lines).toContain("Its 2 schedules are paused until it is unarchived: Morning report and 1 without a name.");
    expect(words.lines.join(" ")).not.toContain("sched-0");
  });

  it("keeps what the first answer said when the same request is answered again without it", () => {
    const first = parseIdentityOutcome({
      outcome: "done",
      operation_id: "human.x.r",
      agents: [{ name: "scout", state: "archived", schedules: { effect: "paused", count: 1, schedules: [morning], incomplete: true } }],
    })!;
    // A replay states the agent's state and nothing of its schedules.
    const replay = parseIdentityOutcome({ outcome: "done", operation_id: "human.x.r", agents: [{ name: "scout", state: "archived" }] })!;
    const kept = keepFirstSchedules(first, replay);
    expect(kept).toMatchObject({
      outcome: "done",
      agents: [{ name: "scout", state: "archived", schedules: { effect: "paused", count: 1, incomplete: true, askedAgain: true } }],
    });
    expect(schedulesWords(kept.outcome === "done" ? kept.agents[0]!.schedules : undefined)).toEqual([
      "Its schedule Morning report is paused until it is unarchived.",
      "Not every schedule could be changed at first. The team was asked again to follow the rest.",
    ]);
    // An answer that says what became of the schedules itself is kept as it is.
    const second = parseIdentityOutcome({
      outcome: "done",
      operation_id: "human.x.r",
      agents: [{ name: "scout", state: "archived", schedules: { effect: "paused", count: 2, schedules: [morning, weekly] } }],
    })!;
    expect(keepFirstSchedules(first, second)).toEqual(second);
    // Another agent's schedules are nobody else's, and an outcome that is
    // no change's (a decision that waits) is kept as it is.
    const other = parseIdentityOutcome({ outcome: "done", operation_id: "human.x.r", agents: [{ name: "ranger", state: "archived" }] })!;
    expect(keepFirstSchedules(first, other)).toEqual(other);
    const waiting = parseIdentityOutcome({ outcome: "pending_decision", operation_id: "x", decision_id: "d", deadline_ms: 9 })!;
    expect(keepFirstSchedules(first, waiting)).toEqual(waiting);
    expect(keepFirstSchedules(waiting, replay)).toEqual(replay);
  });

  it("follows the scripted family: the first answer says it, a replay does not", () => {
    const family = createScriptedIdentityFamily();
    family.seed({ name: "scout", profile: "worker" });
    family.setSchedules("scout", [
      { schedule_id: "sched-01", name: "Morning report" },
      { schedule_id: "sched-02", name: "Weekly digest", state: "paused_by_name" },
      { schedule_id: "sched-03" },
    ]);
    const digest = () => family.handle("meta/identity/get", { name: "scout" }).result.spec_digest;
    const archive = { request_id: "a-1", name: "scout", expected_spec_digest: digest() };
    const first = family.handle("meta/identity/archive", archive).result;
    // An archive pauses the schedules that run; one somebody paused by name
    // is left as it is.
    expect(first.agents[0].schedules).toEqual({
      effect: "paused", count: 2, schedules: [{ schedule_id: "sched-01", name: "Morning report" }, { schedule_id: "sched-03" }],
    });
    const again = family.handle("meta/identity/archive", archive).result;
    expect("schedules" in again.agents[0]).toBe(false);
    expect(again.agents[0].state).toBe(first.agents[0].state);
    // An unarchive resumes exactly the schedules the archive paused.
    const back = family.handle("meta/identity/unarchive", { request_id: "u-1", name: "scout", expected_spec_digest: digest() }).result;
    expect(back.agents[0].schedules.effect).toBe("resumed");
    expect(back.agents[0].schedules.schedules.map((schedule: { schedule_id: string }) => schedule.schedule_id)).toEqual(["sched-01", "sched-03"]);
    expect(family.schedulesOf("scout").map((schedule: { state: string }) => schedule.state)).toEqual(["active", "paused_by_name", "active"]);
    // A removal ends every one. A pass that cannot read one says so, and
    // the same request asked again follows the rest.
    family.failSchedulesAfter("scout", 1);
    const remove = { request_id: "r-1", name: "scout", expected_spec_digest: digest() };
    const removed = family.handle("meta/identity/remove", remove).result;
    expect(removed.agents[0].schedules).toMatchObject({ effect: "ended", count: 1, incomplete: true });
    expect(family.schedulesOf("scout")).toHaveLength(2);
    expect("schedules" in family.handle("meta/identity/remove", remove).result.agents[0]).toBe(false);
    expect(family.schedulesOf("scout")).toHaveLength(0);
    // A family that is given no schedules says nothing of them.
    const plain = createScriptedIdentityFamily();
    const warden = plain.handle("meta/identity/get", { name: "warden" }).result.spec_digest;
    expect("schedules" in plain.handle("meta/identity/archive", { request_id: "a-2", name: "warden", expected_spec_digest: warden }).result.agents[0]).toBe(false);
  });
});

describe("what a host's preparation of its agents refuses", () => {
  const refusal = (kind: string, data: Record<string, unknown> = {}, detail = "") => ({ kind, retryable: false, detail, data });

  it("says a move into a profile whose agents the host prepares in words of its own", () => {
    // The move is refused `unsupported` with a feature of its own. It is no
    // matter of versions: the profile's agents are prepared before their
    // first use, and this one was not.
    const moved = refusalSentence(refusal("unsupported", { feature: "move_into_prepared_profile" }));
    expect(moved).toBe("An agent cannot be moved into this profile: its host prepares the agents of this profile before their first use. Create a new agent in this profile instead.");
    expect(moved).not.toMatch(/version|_/);
    // Any other feature, and none, keep the kind's sentence.
    const plain = "This change needs something this version does not support. Leave it out.";
    expect(refusalSentence(refusal("unsupported", { feature: "another feature" }))).toBe(plain);
    expect(refusalSentence(refusal("unsupported"))).toBe(plain);
    // The feature alone says nothing: another kind that carries it keeps its own sentence.
    expect(refusalSentence(refusal("change_denied", { feature: "move_into_prepared_profile" })))
      .toBe("The host's rules do not allow you this change. It was not made, and asking again changes nothing.");
    // The scripted family answers the move, and its plan, in the wire's shape.
    const family = createScriptedIdentityFamily();
    family.setProfileChange("reviewer", "prepared");
    const digest = family.handle("meta/identity/get", { name: "warden" }).result.spec_digest;
    const body = { name: "warden", expected_spec_digest: digest, set: { profile: "reviewer" } };
    for (const answer of [
      family.handle("meta/identity/plan", { change: "update", request_id: "p-1", ...body }),
      family.handle("meta/identity/update", { request_id: "u-1", ...body }),
    ]) {
      expect(answer.error.data).toMatchObject({ kind: "unsupported", field: "set.profile", feature: "move_into_prepared_profile", retryable: false });
      expect(refusalFormField("update", answer.error.data.field)).toBe("profile");
    }
    expect(family.agents.get("warden").profile).toBe("worker");
  });

  it("says a read that could not see the team's open requests as a read to repeat", () => {
    // A get and a list name what is open for each agent. Where the team's
    // held changes cannot be read, the read is refused with the kind a
    // change that cannot be held is refused with: for a read it means
    // "read again", and nothing about a decision or a change.
    const unread = refusal("approval_unavailable", {}, "The mob's held changes could not be read now, so what is open for its agents is not known: read again later.");
    const said = readRefusalSentence(unread);
    expect(said).toBe("The team's open requests could not be read just now, so the console cannot say what waits for its agents. Try again shortly.");
    expect(said).not.toMatch(/decision|nobody|Nothing was made|\bmob\b|_/);
    // A change refused with that kind keeps the change's sentence.
    expect(refusalSentence(unread)).toBe("This change needs a person's decision, and this team has nobody who can be asked. Nothing was made.");
    // Every other refusal of a read reads as it does anywhere.
    for (const kind of ["identity_unknown", "access_denied", "mob_not_running", "a_kind_of_a_later_gateway"]) {
      expect(readRefusalSentence(refusal(kind))).toBe(refusalSentence(refusal(kind)));
    }
  });
});
