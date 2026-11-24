// A scripted gateway for console rows about hosted mobs: the launch mob
// and, when asked, a second root the host created, a mob nested under each,
// a root that is asleep and a root that is unavailable. It answers the
// console's own routes through `fetch` and records every request, so a row
// can say exactly what the console sent.
//
// The mobs and their members:
//
//   ops-team        the launch mob            planner, reviewer
//     research      a member's (planner's)    research/scout
//   support         the host's, loaded        support/reviewer, support/triage
//     support-lab   a member's (triage's)     support-lab/tester
//   archive         the host's, asleep        (no rows, no count)
//   billing         the host's, unavailable   (no rows, no count)
//
// `reviewer` is a member of both roots on purpose: one name, two members.
//
// What it does as a gateway that hosts mobs does:
// - it lists `mob_addressing` in `meta/capabilities.feature_capabilities`
//   when `addressing` is on;
// - without `mobs=*` every read answers for the launch mob alone, with the
//   mob a launch member created; with it, the hosted roots are listed and
//   the members of the loaded ones have rows;
// - a member of a hosted root is served by its own door only: a request
//   names the mob (`mob_id`) and the member as that door knows it. A request
//   that names no mob reaches the launch mob's members and nobody else;
// - a frame of a hosted root names its mob (`mob_id`) beside its identity,
//   and is served only on a stream opened with `mobs=*`.

export const LAUNCH_MOB = "ops-team";
export const LAUNCH_NESTED_MOB = "research";
export const HOSTED_MOB = "support";
export const HOSTED_NESTED_MOB = "support-lab";
export const ASLEEP_MOB = "archive";
export const UNAVAILABLE_MOB = "billing";

import { createScriptedIdentityFamily } from "../../../fixtures/scripted-identity-family.cjs";

/** The agents' records of one door, scripted (fixtures/scripted-identity-family.cjs). */
export type AgentRecordFamily = ReturnType<typeof createScriptedIdentityFamily>;

/** One request the console sent. */
export interface RecordedRequest {
  /** `GET /console/experience`, `POST /console/rpc`, ... without the query. */
  route: string;
  /** The query parameters, in the order sent. */
  query: Array<[string, string]>;
  /** The JSON-RPC method, for a request of a request door. */
  method?: string;
  /** The JSON-RPC params, or a JSON body's members. */
  params?: Record<string, unknown>;
  /** `Last-Event-ID`, when the request carries one. */
  lastEventId?: string;
  /** The names of the file parts of a multipart request door's body (`file:<upload id>`). */
  files?: string[];
}

/** A member as the gateway holds it. */
interface FakeMember {
  /** The mob whose door serves it; null for the launch mob's door. */
  door: string | null;
  /** The member as that door knows it. */
  name: string;
  /** The member's own mob. */
  mob: string;
  /** Its key in the console's roster. */
  rowKey: string;
  label: string;
}

const MEMBERS: FakeMember[] = [
  { door: null, name: "planner", mob: LAUNCH_MOB, rowKey: "planner", label: "Planner" },
  { door: null, name: "reviewer", mob: LAUNCH_MOB, rowKey: "reviewer", label: "Launch reviewer" },
  { door: null, name: "research/scout", mob: LAUNCH_NESTED_MOB, rowKey: "research/scout", label: "Scout" },
  { door: HOSTED_MOB, name: "reviewer", mob: HOSTED_MOB, rowKey: "support/reviewer", label: "Support reviewer" },
  { door: HOSTED_MOB, name: "triage", mob: HOSTED_MOB, rowKey: "support/triage", label: "Triage" },
  { door: HOSTED_MOB, name: "support-lab/tester", mob: HOSTED_NESTED_MOB, rowKey: "support/support-lab/tester", label: "Tester" },
];

/** A frame the gateway serves. */
export interface FakeFrame {
  /** The door's mob; absent for the launch mob. */
  mob?: string;
  /** The member as its door knows it. */
  identity: string;
  kind: string;
  payload?: Record<string, unknown>;
  interactionId?: string;
  /** More members of the frame as served (its `source`, its `session_id`). */
  extra?: Record<string, unknown>;
}

export interface TwoRootGatewayOptions {
  /** Whether the gateway judges a request's mob and lists the capability. Default true. */
  addressing?: boolean;
  /** Whether the host created mobs beside the launch mob. Default true. */
  hosted?: boolean;
  /** The incarnation token of the hosted root `support`. */
  hostedIncarnation?: string;
  /** The state of the hosted root `support`: `running`, or `dormant` (asleep: no rows, no count). */
  hostedState?: "running" | "dormant" | "unavailable";
  /** The member the console opens first. */
  initialAgent?: string;
  /** Refuse a person's opening of a mob, quoting the mob's id as a gateway does. */
  refuseOpen?: boolean;
  /** Answer `meta/capabilities` this much later, so the console's first roster is read before it knows what the gateway hosts. */
  capabilitiesDelayMs?: number;
  /** Whether a member of the launch mob created the mob `research`. Default true. Without it the launch mob's own roster lists one mob. */
  launchNested?: boolean;
  /** More members of the launch mob, each by its name there. A name may hold "/": one that begins with a hosted root's id spells the key of that root's member. */
  moreLaunchMembers?: string[];
  /** The attention bindings of the launch mob's door's work graph, as its snapshot lists them. */
  launchAttention?: Array<Record<string, unknown>>;
  /** Whether the console offers voice, and voice is ready for the launch mob's members. Voice is the launch mob's: the gateway refuses it for another mob. */
  voice?: boolean;
  /** The methods `meta/capabilities` lists, in place of the few the other rows need: a row that opens every pane lists them all. */
  methods?: string[] | null;
  /** How many reads of `meta/capabilities` fail (HTTP 503) before one answers. */
  capabilitiesFailures?: number;
  /** How many reads of the experience fail (HTTP 503) before one answers. */
  experienceFailures?: number;
  /** Answer a read of the modules this much later, so the read of the experience made beside it answers first. */
  modulesDelayMs?: number;
  /** Answer a read of the experience for every mob this much later, so a read made beside it answers first. */
  everyMobExperienceDelayMs?: number;
  /** The schedules the hosted root's door lists for any of its agents, as that door writes them. */
  hostedSchedules?: unknown[];
  /**
   * The host's lists, as a gateway serves them today: each entry names an
   * agent by its bare name and no mob. `meta/gating/questions` answers
   * these sets and `meta/gating/pending` these approvals.
   */
  hostQuestionSets?: unknown[];
  hostPendingApprovals?: unknown[];
  /** The gating audit's records (`meta/gating/audit`), as the gateway writes them. */
  hostAuditEntries?: unknown[];
  /** Whether the host also lists a root that is asleep (`archive`) and one that is unavailable (`billing`). Default true. */
  sleepers?: boolean;
  /** More members of each agent row's `affordances`, by door: the launch mob's agents, and the hosted roots'. */
  affordances?: { launch?: Record<string, boolean>; hosted?: Record<string, boolean> };
  /** Whether every agent's model takes images (`model_capabilities.image_input`). Default false. */
  imageInput?: boolean;
  /**
   * Refuse a schedule's create, update and resume for an agent of the
   * hosted root as a gateway does while no schedule fire reaches that mob:
   * the existing kind `schedule_host_unavailable`, with a message that
   * names the agent and the mob's id.
   */
  hostedSchedulesNeverSent?: boolean;
  /**
   * Keep agents' records, one family per door: the launch mob's and the
   * hosted root's. Both hold an agent `reviewer` made from one definition
   * (the same spec digest), so a change that reaches the wrong door is
   * carried out there. A request of the `meta/identity/*` family that
   * names no mob is the launch mob's, whatever else it carries.
   */
  agentRecords?: boolean;
  /** Answer the experience with no `mobs` member at all, as a gateway does for an empty list: the caller may address no mob beside the launch mob's own. */
  listNoMobs?: boolean;
  /** More members of the experience document: what a pane needs to be offered (access, memory, workgraph). */
  experience?: Record<string, unknown>;
  /**
   * The work items of each door's work graph. A WorkGraph request is served
   * for the door its `mob_id` names, the launch mob's without one: a get
   * answers that door's item, a claim or a close changes it when the
   * request names the item's revision and is a conflict otherwise, and a
   * snapshot lists that door's items. Null: the gateway keeps no work graph.
   */
  workGraph?: { launch: FakeWorkItem[]; hosted: FakeWorkItem[] } | null;
  /**
   * Refuse a claim or a close that names the hosted root, as a door does
   * that serves the method for the launch mob alone.
   */
  hostedWorkGraphChangesRefused?: boolean;
}

/** A work item as a door's work graph holds it. */
export interface FakeWorkItem {
  id: string;
  title: string;
  status: string;
  revision: number;
}

export interface TwoRootGateway {
  fetch: typeof fetch;
  /** Every request, in the order received. */
  requests: RecordedRequest[];
  /** The requests of one JSON-RPC method. */
  calls(method: string): RecordedRequest[];
  /** Serve a frame on the open streams that may see it, and keep it for timeline reads. */
  push(frame: FakeFrame): void;
  /** Change what the gateway is, between two reads. */
  set(options: Partial<TwoRootGatewayOptions>): void;
  /** Tell every open stream that it cannot replay what the console missed. */
  gap(): void;
  /** How many times a request that named the hosted root woke it. */
  wakes(): number;
  /** The agents' records of each door (`agentRecords`), else null. */
  families: { launch: AgentRecordFamily; hosted: AgentRecordFamily } | null;
  /** The work items of one door's work graph as they stand (`workGraph`), copied. */
  workItems(door: "launch" | "hosted"): FakeWorkItem[];
  /** Forget the recorded requests. */
  clear(): void;
}

interface StoredFrame extends FakeFrame {
  seq: number;
}

const json = (value: unknown, status = 200): Response =>
  new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });

export function createTwoRootGateway(initial: TwoRootGatewayOptions = {}): TwoRootGateway {
  const options: Required<TwoRootGatewayOptions> = {
    addressing: true,
    hosted: true,
    hostedIncarnation: "support-incarnation-1",
    hostedState: "running",
    initialAgent: "planner",
    refuseOpen: false,
    capabilitiesDelayMs: 0,
    launchNested: true,
    moreLaunchMembers: [],
    launchAttention: [],
    voice: false,
    methods: null,
    capabilitiesFailures: 0,
    experienceFailures: 0,
    modulesDelayMs: 0,
    everyMobExperienceDelayMs: 0,
    listNoMobs: false,
    experience: {},
    workGraph: null,
    hostedWorkGraphChangesRefused: false,
    agentRecords: false,
    hostedSchedulesNeverSent: false,
    hostQuestionSets: [],
    hostPendingApprovals: [],
    hostAuditEntries: [],
    hostedSchedules: [],
    sleepers: true,
    affordances: {},
    imageInput: false,
    ...initial,
  };
  const reviewerRecord = { name: "reviewer", display_name: "Reviewer", profile: "worker", instructions: ["Review the work."] };
  const families = options.agentRecords
    ? {
      launch: createScriptedIdentityFamily({ agents: [reviewerRecord, { name: "planner", display_name: "Planner", profile: "worker", instructions: ["Plan the work."] }] }),
      hosted: createScriptedIdentityFamily({
        agents: [
          reviewerRecord,
          // Created by the hosted root's own `reviewer`: the door names it as it knows it.
          {
            name: "triage", display_name: "Triage", profile: "worker", instructions: ["Sort the requests."],
            provenance: { entered: "created", requester_kind: "agent", requester: "reviewer", at_ms: 1_762_000_000_000, consent: { form: "allowed" } },
          },
        ],
      }),
    }
    : null;
  const requests: RecordedRequest[] = [];
  const frames: StoredFrame[] = [];
  const streams = new Set<{ everyMob: boolean; controller: ReadableStreamDefaultController<Uint8Array> }>();
  const encoder = new TextEncoder();
  let interactions = 0;
  let wakes = 0;

  const hostedListed = () => options.addressing && options.hosted;
  const hostedLoaded = () => hostedListed() && options.hostedState === "running";

  /** The members with rows, for a read with or without `mobs=*`. */
  /** The members the gateway holds: the fixed ones, and the launch mob's further members. */
  const members = (): FakeMember[] => [
    ...MEMBERS,
    ...(options.moreLaunchMembers ?? []).map((name) => ({ door: null, name, mob: LAUNCH_MOB, rowKey: name, label: name })),
  ];

  const rows = (everyMob: boolean): FakeMember[] =>
    members().filter((member) => (member.mob !== LAUNCH_NESTED_MOB || (options.launchNested && !options.listNoMobs))
      && (member.door === null || (everyMob && hostedLoaded() && !options.listNoMobs)));

  // A row names the mob its member sits in and the member's bare name there
  // (`member_mob_id`, `member_name`), beside its key.
  const agentRow = (member: FakeMember) => ({
    identity: member.rowKey,
    member_id: member.rowKey,
    agent_id: member.rowKey,
    member_mob_id: member.mob,
    member_name: member.mob === (member.door ?? LAUNCH_MOB) ? member.name : member.name.slice(member.mob.length + 1),
    label: member.label,
    kind: "member",
    role: "analyst",
    state: "running",
    addressable: true,
    mob_id: member.mob,
    affordances: {
      ...(families
        ? { can_send_message: true, can_change_identity: true, can_remove_identity: true }
        : { can_send_message: true }),
      ...(member.door === null ? options.affordances.launch : options.affordances.hosted),
    },
    model_capabilities: { image_input: options.imageInput },
  });

  const mobs = (everyMob: boolean) => {
    const count = (mob: string) => rows(everyMob).filter((member) => member.mob === mob).length;
    // A gateway that does not judge a request's mob lists a mob as before:
    // no provenance, no default mark, no incarnation.
    const fields = (value: Record<string, unknown>) => (options.addressing ? value : {});
    const launch = [
      { mob_id: LAUNCH_MOB, state: "running", member_count: count(LAUNCH_MOB), ...fields({ provenance: "host", default: true, incarnation: "ops-incarnation-1" }) },
      { mob_id: LAUNCH_NESTED_MOB, parent_mob_id: LAUNCH_MOB, host_identity: "planner", state: "running", member_count: count(LAUNCH_NESTED_MOB), ...fields({ provenance: "member", incarnation: "research-incarnation-1" }) },
    ];
    if (!options.launchNested) launch.pop();
    if (!everyMob || !hostedListed()) return launch;
    const support = hostedLoaded()
      ? [
        { mob_id: HOSTED_MOB, state: "running", member_count: count(HOSTED_MOB), provenance: "host", incarnation: options.hostedIncarnation },
        { mob_id: HOSTED_NESTED_MOB, parent_mob_id: HOSTED_MOB, host_identity: "support/triage", state: "running", member_count: count(HOSTED_NESTED_MOB), provenance: "member", incarnation: "support-lab-incarnation-1" },
      ]
      : [{ mob_id: HOSTED_MOB, state: options.hostedState, provenance: "host", incarnation: options.hostedIncarnation }];
    return [
      ...launch,
      ...support,
      ...(options.sleepers
        ? [
          { mob_id: ASLEEP_MOB, state: "dormant", provenance: "host", incarnation: "archive-incarnation-1" },
          { mob_id: UNAVAILABLE_MOB, state: "unavailable", provenance: "host", incarnation: "billing-incarnation-1" },
        ]
        : []),
    ];
  };

  const experience = (everyMob: boolean) => ({
    runtime_id: LAUNCH_MOB,
    storage_scope: "console-storage:v1:two-root",
    console_config: { title: "Operations", layout: { initial_agent: options.initialAgent } },
    agent_sidebar: { live_snapshot: { agents: rows(everyMob).map(agentRow) } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
    topology: { live_snapshot: options.listNoMobs ? { mob_id: LAUNCH_MOB } : { mob_id: LAUNCH_MOB, mobs: mobs(everyMob) } },
    ...(options.voice ? { voice: { readiness_method: "meta/console/voice/readiness" } } : {}),
    ...(families ? { identity_changes: families.launch.experienceSection() } : {}),
    ...options.experience,
  });

  /** The member a request names, by the door it names; undefined for a name that door does not hold. */
  const memberOf = (mobId: unknown, identity: unknown): FakeMember | undefined => {
    const door = typeof mobId === "string" && mobId && mobId !== LAUNCH_MOB ? mobId : null;
    if (door !== null && !hostedLoaded()) return undefined;
    return members().find((member) => member.door === door && member.name === identity);
  };

  const wireFrame = (frame: StoredFrame) => ({
    id: `frame:${frame.seq}`,
    cursor: `console:${frame.seq}`,
    kind: frame.kind,
    identity: frame.identity,
    ...(frame.mob ? { mob_id: frame.mob } : {}),
    ...(frame.interactionId ? { interaction_id: frame.interactionId } : {}),
    timestamp_ms: 1_700_000_000_000 + frame.seq,
    payload: frame.payload ?? {},
    ...(frame.extra ?? {}),
  });

  const sseBlock = (frame: StoredFrame) =>
    `id: console:${frame.seq}\nevent: console_frame\ndata: ${JSON.stringify({ type: "console_frame", frame: wireFrame(frame) })}\n\n`;

  const everyMobOf = (value: unknown): boolean => options.addressing && value === "*";
  // A list of the host that names members: without the selector it answers,
  // of the entries members made, the launch mob's members' alone, with it
  // the entries of every mob.
  const hostList = (entries: unknown[], selector: unknown): unknown[] => (everyMobOf(selector) ? entries : entries.filter((entry) => {
    const mob = (entry as { origin?: { mob_id?: unknown } } | null)?.origin?.mob_id;
    return typeof mob !== "string" || mob === "" || mob === LAUNCH_MOB;
  }));

  const rpc = (request: { id?: unknown; method: string; params?: Record<string, unknown> }): Response => {
    const params = request.params ?? {};
    const answer = (result: unknown) => json({ jsonrpc: "2.0", id: request.id, result });
    const refuse = (code: number, message: string, data?: unknown) =>
      json({ jsonrpc: "2.0", id: request.id, error: { code, message, ...(data === undefined ? {} : { data }) } });
    const mobId = options.addressing ? params.mob_id : undefined;
    // Serving a request that names a mob loads the mob: whatever the request
    // is, a mob that was asleep is awake after it.
    // A mob that is unavailable cannot be loaded: a request that names it is
    // refused and it stays as it is.
    if (mobId === HOSTED_MOB && hostedListed() && options.hostedState === "unavailable") {
      return refuse(-32004, `the mob ${HOSTED_MOB} cannot be loaded`, { kind: "mob_unavailable", mob_id: HOSTED_MOB });
    }
    if (mobId === HOSTED_MOB && hostedListed() && !options.refuseOpen && request.method !== "meta/console/voice/readiness") {
      if (options.hostedState !== "running") wakes += 1;
      options.hostedState = "running";
    }
    if (families && request.method.startsWith("meta/identity/")) {
      // One family per door. A request that names no mob is the launch
      // mob's: a bare name with no mob reaches the launch mob's agent of
      // that name, and a request id is known to the door that held it.
      const named = typeof mobId === "string" && mobId && mobId !== LAUNCH_MOB ? mobId : null;
      if (named !== null && !(named === HOSTED_MOB && hostedLoaded())) {
        return refuse(-32602, `the mob ${named} is not served here`, { kind: "foreign_mob_target", mob_id: named });
      }
      const { mob_id: _mob, expected_caller_scope: _scope, ...own } = params;
      const answered = (named ? families.hosted : families.launch).handle(request.method, own);
      if (!answered) return answer({});
      if (answered.lost) return json({ error: "lost" }, 504);
      if (answered.error) return refuse(answered.error.code, answered.error.message, answered.error.data);
      return answer(answered.result);
    }
    if (families && request.method === "meta/decisions/list") {
      // The host's one list of the requester's held changes, whatever door holds each.
      return answer({ decisions: [], statuses: [...families.launch.decisionStatuses(), ...families.hosted.decisionStatuses()], unreadable: [] });
    }
    if (options.workGraph && request.method.startsWith("meta/workgraph/")) {
      // One work graph per door. A request that names no mob is the launch
      // mob's, whatever item it names: an item id can be the same in both.
      const named = typeof mobId === "string" && mobId && mobId !== LAUNCH_MOB ? mobId : null;
      if (named !== null && !(named === HOSTED_MOB && hostedLoaded())) {
        return refuse(-32602, `the mob ${named} is not served here`, { kind: "foreign_mob_target", requested_mob_id: named });
      }
      const items = named ? options.workGraph.hosted : options.workGraph.launch;
      const wireItem = (item: FakeWorkItem) => ({
        ...item, realm_id: "realm-1", namespace: "default", priority: "medium",
        created_at: "2026-07-08T09:00:00Z", updated_at: "2026-07-08T09:00:00Z",
      });
      if (request.method === "meta/workgraph/snapshot") {
        return answer({ captured_at: "2026-07-08T09:00:00Z", items: items.map(wireItem), edges: [], attention: named ? [] : options.launchAttention });
      }
      // The ledger's tail: this gateway keeps none.
      if (request.method === "meta/workgraph/events") return answer({ events: [] });
      const item = items.find((candidate) => candidate.id === params.id);
      if (!item) return refuse(-32602, "no work item has that id", { kind: "work_item_not_found" });
      if (request.method === "meta/workgraph/get") return answer({ item: wireItem(item) });
      if (request.method === "meta/workgraph/claim" || request.method === "meta/workgraph/close") {
        if (named && options.hostedWorkGraphChangesRefused) {
          return refuse(-32602, `the mob ${named} is not served here`, { kind: "foreign_mob_target", requested_mob_id: named });
        }
        if (params.expected_revision !== item.revision) {
          return refuse(-32042, `workgraph conflict: stale revision ${String(params.expected_revision)}, item is at ${item.revision}`);
        }
        item.status = request.method === "meta/workgraph/claim" ? "in_progress" : "done";
        item.revision += 1;
        return answer({ item: wireItem(item) });
      }
      return answer({});
    }
    switch (request.method) {
      case "meta/gating/questions":
        return answer({ question_sets: hostList(options.hostQuestionSets, params.mobs) });
      case "meta/gating/pending":
        return answer({ pending: hostList(options.hostPendingApprovals, params.mobs) });
      case "meta/gating/audit":
        return answer({ entries: hostList(options.hostAuditEntries, params.mobs) });
      case "meta/console/whoami":
        return answer({ actor: { kind: "person", principal: "operator@example.test", assurance: "person_authenticated" }, posture: "enforced", can_administer: true, approver_ids: [], caller_scope: "caller-operator" });
      case "meta/capabilities":
        return answer({
          version: "test",
          methods: options.methods ?? [
            "meta/console/send", "meta/console/query_timeline", "meta/console/whoami", "meta/console/inspect_identity", "meta/schedule/list",
            ...(families ? [...families.launch.methods, "meta/decisions/list"] : []),
          ],
          feature_capabilities: options.addressing ? ["mob_addressing"] : [],
        });
      case "meta/console/query_timeline": {
        const everyMob = everyMobOf(params.mobs);
        const named = typeof params.identity === "string" ? params.identity : "";
        const door = typeof mobId === "string" && mobId && mobId !== LAUNCH_MOB ? mobId : undefined;
        const page = frames.filter((frame) =>
          named
            ? frame.identity === named && frame.mob === door
            : frame.mob === undefined || (everyMob && hostedListed()));
        const last = frames[frames.length - 1];
        return answer({ frames: page.map(wireFrame), available: true, exhausted: true, ...(last ? { latest_cursor: `console:${last.seq}` } : {}) });
      }
      case "meta/console/voice/readiness": {
        // Voice is the launch mob's wiring: another mob is refused for every caller.
        if (mobId !== undefined && mobId !== null && mobId !== LAUNCH_MOB) {
          return refuse(-32602, `the mob ${String(mobId)} is not served here`, { kind: "foreign_mob_target", mob_id: mobId });
        }
        const member = memberOf(undefined, params.identity);
        return answer({ identity: params.identity, available: Boolean(options.voice && member) });
      }
      case "meta/status": {
        if (options.refuseOpen) {
          return refuse(-32602, `the mob ${String(mobId)} is not served here`, { kind: "foreign_mob_target", mob_id: mobId });
        }
        // An ordinary request that names a mob: serving it loads the mob.
        if (mobId === HOSTED_MOB && hostedListed()) options.hostedState = "running";
        return answer({ status: "running" });
      }
      case "meta/console/inspect_identity": {
        const member = memberOf(mobId, params.identity);
        if (!member) return refuse(-32602, "no agent holds that name", { kind: "identity_not_found" });
        // A door answers with its member as it knows it, and with that
        // member's peers the same way: `triage` of the hosted root and
        // `planner` of the launch mob are each wired to their own mob's
        // `reviewer`.
        const peers = member.name === "triage" || member.name === "planner" ? ["reviewer"] : [];
        return answer({ identity: { identity: member.name, state: "active", addressability: "addressable" }, peers });
      }
      case "meta/fork_member": {
        if (!memberOf(mobId, params.source_member_id)) {
          return refuse(-32602, "no agent holds that name", { kind: "identity_not_found" });
        }
        // A door answers with its members as it knows them.
        return answer({ agent_identity: params.agent_identity, side_chat_of: params.source_member_id, display_name: "Helper" });
      }
      case "meta/schedule/create":
      case "meta/schedule/update":
      case "meta/schedule/resume": {
        if (typeof params.identity === "string" && !memberOf(mobId, params.identity)) {
          return refuse(-32602, "no agent holds that name", { kind: "identity_not_found" });
        }
        if (options.hostedSchedulesNeverSent && mobId === HOSTED_MOB) {
          return refuse(
            -32004,
            `this gateway sends the console schedules of its default mob's agents only, so a console schedule for ${String(params.identity)} of mob ${HOSTED_MOB} would never be sent`,
            { kind: "schedule_host_unavailable" },
          );
        }
        return answer({});
      }
      case "meta/schedule/list": {
        if (typeof params.identity === "string" && !memberOf(mobId, params.identity)) {
          return refuse(-32602, "no agent holds that name", { kind: "identity_not_found" });
        }
        // A door lists the schedules of the agent it was asked for, named as it knows it.
        return answer({
          identity: params.identity,
          schedules: mobId === HOSTED_MOB
            ? options.hostedSchedules.filter((schedule) => (schedule as { identity?: unknown } | null)?.identity === params.identity)
            : [],
        });
      }
      case "meta/console/send": {
        // So does a send: the mob wakes for it.
        if (mobId === HOSTED_MOB && hostedListed()) options.hostedState = "running";
        const member = memberOf(mobId, params.identity);
        if (!member) return refuse(-32602, "no agent holds that name", { kind: "identity_not_found" });
        interactions += 1;
        return answer({ interaction_id: `interaction-${interactions}`, identity: member.name });
      }
      default: {
        // The one-door rule, whatever word the method names its member by:
        // a member is served by the door of its mob only. A new agent's
        // name (identity/create) names nobody yet.
        const named = request.method === "meta/identity/create" ? undefined
          : [params.identity, params.member_id, params.source_member_id, params.name].find((value) => typeof value === "string");
        if (named !== undefined && !memberOf(mobId, named)) {
          return refuse(-32602, "no agent holds that name", { kind: "identity_not_found" });
        }
        return answer(request.method.startsWith("meta/identity/") ? { name: params.name } : {});
      }
    }
  };

  const fetchImpl = async (input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> => {
    const url = new URL(String(input), "http://console.test");
    const verb = (init.method ?? "GET").toUpperCase();
    const headers = new Headers(init.headers);
    const recorded: RecordedRequest = { route: `${verb} ${url.pathname}`, query: [...url.searchParams.entries()] };
    const lastEventId = headers.get("Last-Event-ID");
    if (lastEventId) recorded.lastEventId = lastEventId;
    requests.push(recorded);
    const everyMob = everyMobOf(url.searchParams.get("mobs"));

    if (url.pathname === "/console/experience") {
      if (options.experienceFailures > 0) {
        options.experienceFailures -= 1;
        return json({ error: "unavailable" }, 503);
      }
      if (everyMob && options.everyMobExperienceDelayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, options.everyMobExperienceDelayMs));
      }
      return json(experience(everyMob));
    }
    if (url.pathname === "/console/modules") {
      if (options.modulesDelayMs > 0) await new Promise((resolve) => setTimeout(resolve, options.modulesDelayMs));
      return json({ modules: [] });
    }
    if (url.pathname === "/console/timeline/stream") {
      let entry: { everyMob: boolean; controller: ReadableStreamDefaultController<Uint8Array> } | undefined;
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          entry = { everyMob, controller };
          streams.add(entry);
        },
        cancel() {
          if (entry) streams.delete(entry);
        },
      });
      init.signal?.addEventListener("abort", () => {
        if (entry) streams.delete(entry);
      });
      return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
    }
    if (url.pathname === "/console/rpc" || url.pathname === "/console/rpc/multipart") {
      const raw = url.pathname === "/console/rpc"
        ? String(init.body)
        : String((init.body as FormData).get("payload"));
      if (url.pathname === "/console/rpc/multipart") {
        recorded.files = [...(init.body as FormData).keys()].filter((name) => name.startsWith("file:"));
      }
      const request = JSON.parse(raw) as { id?: unknown; method: string; params?: Record<string, unknown> };
      recorded.method = request.method;
      recorded.params = request.params ?? {};
      if (request.method === "meta/capabilities" && options.capabilitiesDelayMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, options.capabilitiesDelayMs));
      }
      if (request.method === "meta/capabilities" && options.capabilitiesFailures > 0) {
        options.capabilitiesFailures -= 1;
        return new Response("the gateway is busy", { status: 503 });
      }
      return rpc(request);
    }
    if (init.body && typeof init.body === "string") {
      try {
        recorded.params = JSON.parse(init.body) as Record<string, unknown>;
      } catch {
        // Not a JSON body: the route and query say enough.
      }
    }
    return new Response("not found", { status: 404 });
  };

  return {
    fetch: fetchImpl as typeof fetch,
    requests,
    calls: (method) => requests.filter((request) => request.method === method),
    push(frame) {
      const stored: StoredFrame = { ...frame, seq: frames.length + 1 };
      frames.push(stored);
      const block = encoder.encode(sseBlock(stored));
      for (const stream of streams) {
        // The host's one log: a frame of a hosted root is served to a caller
      // who may address it, whatever the list last said of the root.
      if (stored.mob === undefined || (stream.everyMob && hostedListed())) stream.controller.enqueue(block);
      }
    },
    set(next) {
      Object.assign(options, next);
    },
    wakes: () => wakes,
    workItems: (door) => (options.workGraph ? options.workGraph[door].map((item) => ({ ...item })) : []),
    families,
    gap() {
      const block = encoder.encode('event: replay_unavailable\ndata: {"reason":"source_reset","source_kind":"agent"}\n\n');
      for (const stream of streams) stream.controller.enqueue(block);
    },
    clear() {
      requests.length = 0;
    },
  };
}

/** Whether a recorded request names a mob in any way: `mob_id` or `mobs`, in its query or its params. */
export function namesAMob(request: RecordedRequest): boolean {
  const inQuery = request.query.some(([name]) => name === "mob_id" || name === "mobs");
  const params = request.params ?? {};
  return inQuery || "mob_id" in params || "mobs" in params;
}

/**
 * jsdom lacks `AbortSignal.any`, which every supported browser has and the
 * stock transport combines its timeout with: without it a whole-console row
 * opens no stream. Returns what puts the environment back.
 */
export function ensureAbortSignalAny(): () => void {
  const target = AbortSignal as unknown as { any?: (signals: AbortSignal[]) => AbortSignal };
  if (typeof target.any === "function") return () => {};
  target.any = (signals: AbortSignal[]) => {
    const controller = new AbortController();
    for (const signal of signals) {
      if (signal.aborted) {
        controller.abort(signal.reason);
        break;
      }
      signal.addEventListener("abort", () => controller.abort(signal.reason), { once: true });
    }
    return controller.signal;
  };
  return () => {
    delete target.any;
  };
}
