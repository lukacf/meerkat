import { afterEach, describe, expect, it, vi } from "vitest";

import { createHttpConsoleTransport as libraryTransport } from "../../../packages/console-core/src/headless";
import { CONSOLE_HOST_SCOPED_RPC_METHODS, CONSOLE_MEMBER_PARAMS_BY_METHOD, CONSOLE_RPC_METHODS } from "./contract";
import { createTwoRootGateway, HOSTED_MOB, type RecordedRequest, type TwoRootGateway } from "./fixtures/two-root-gateway";
import { CONSOLE_COMMAND_NAMES, consoleCommandMethod, createHttpConsoleTransport as consoleTransport, type MetaConsoleTransport } from "./headless";

// A member of a mob the host created beside the launch mob is served by
// that mob's own door: a request names the mob (`mob_id`) and the member as
// that door knows it. The console's key for the member stays
// `<member's own mob id>/<name>`, and every other member is addressed as
// before.

const COPIES = [
  ["the console's transport", consoleTransport],
  ["the library's transport", libraryTransport],
] as const;

const inspectTarget = (identity: string) => ({
  id: `inspect:${identity}`,
  kind: "meta/identity-inspect" as const,
  identity,
  memberId: identity,
  title: "Details",
});

/** A transport that has read the gateway's capabilities and its mobs, as a running console has. */
async function connected(gateway: TwoRootGateway, create: typeof consoleTransport): Promise<MetaConsoleTransport> {
  vi.stubGlobal("fetch", gateway.fetch);
  const transport = create({ baseUrl: "" }) as MetaConsoleTransport;
  await transport.capabilities();
  await transport.loadExperience();
  gateway.clear();
  return transport;
}

/** The mob and the member a request names. */
const named = (request: RecordedRequest | undefined) => ({
  mob_id: request?.params?.mob_id ?? new Map(request?.query).get("mob_id"),
  identity: request?.params?.identity ?? new Map(request?.query).get("identity"),
});

const last = (requests: RecordedRequest[]) => requests[requests.length - 1];

afterEach(() => {
  vi.unstubAllGlobals();
});

describe.each(COPIES)("a member of a hosted root, through %s", (_name, create) => {
  it("is sent a message by its mob and its bare name, and the answer is kept under its key", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const accepted = await transport.send({ identity: "support/reviewer", content: "hello", origin: "console", idempotencyKey: "one" });
    expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: HOSTED_MOB, identity: "reviewer" });
    expect(accepted.identity).toBe("support/reviewer");
  });

  it("is sent a message with a file the same way", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const accepted = await transport.send({
      identity: "support/triage",
      content: "see the file",
      origin: "console",
      idempotencyKey: "two",
      attachments: [new File(["x"], "note.png", { type: "image/png" })],
    });
    const request = last(gateway.calls("meta/console/send"));
    expect(request?.route).toBe("POST /console/rpc/multipart");
    expect(named(request)).toEqual({ mob_id: HOSTED_MOB, identity: "triage" });
    expect(accepted.identity).toBe("support/triage");
  });

  it("is named by the pair in a command and in a timeline read; a stream of its own is the one stream of every mob, kept to its frames", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    await transport.executeCommand!({ command: CONSOLE_COMMAND_NAMES.inspectIdentity, target: inspectTarget("support/reviewer") as never });
    expect(named(last(gateway.calls("meta/console/inspect_identity")))).toEqual({ mob_id: HOSTED_MOB, identity: "reviewer" });

    await transport.queryTimeline({ identity: "support/reviewer", mode: "recent", limit: 50 });
    expect(named(last(gateway.calls("meta/console/query_timeline")))).toEqual({ mob_id: HOSTED_MOB, identity: "reviewer" });

    // A member's own stream is not served for a member of another mob: the
    // transport opens the one stream of every mob the caller may address,
    // names no mob and no member, and hands on that member's frames only.
    const seen: Array<string | undefined> = [];
    const stop = transport.subscribeTimeline({ identity: "support/reviewer" }, (frame) => seen.push((frame as { identity?: string }).identity));
    await new Promise((resolve) => setTimeout(resolve, 0));
    const stream = last(gateway.requests.filter((request) => request.route === "GET /console/timeline/stream"));
    expect(stream?.query).toEqual([["mobs", "*"]]);
    gateway.push({ identity: "reviewer", kind: "text_delta", payload: { delta: "the launch mob's reviewer" } });
    gateway.push({ mob: HOSTED_MOB, identity: "triage", kind: "text_delta", payload: { delta: "another agent of the hosted root" } });
    gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "text_delta", payload: { delta: "the member" } });
    gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "text_delta", payload: { delta: "the member again" } });
    for (let turn = 0; turn < 100 && seen.length < 2; turn += 1) await new Promise((resolve) => setTimeout(resolve, 5));
    await new Promise((resolve) => setTimeout(resolve, 30));
    stop();
    expect(seen).toEqual(["support/reviewer", "support/reviewer"]);
    // A launch member's own stream is the request of before.
    const launch = transport.subscribeTimeline({ identity: "planner" }, () => {});
    await new Promise((resolve) => setTimeout(resolve, 0));
    launch();
    const own = last(gateway.requests.filter((request) => request.route === "GET /console/timeline/stream"));
    expect(named(own)).toEqual({ mob_id: undefined, identity: "planner" });
    expect(new Map(own?.query).has("mobs")).toBe(false);
  });
});

describe.each(COPIES)("a member of a mob nested under a hosted root, through %s", (_name, create) => {
  it("is named by the root and by its key", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const accepted = await transport.send({ identity: "support/support-lab/tester", content: "hello", origin: "console", idempotencyKey: "three" });
    expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: HOSTED_MOB, identity: "support-lab/tester" });
    expect(accepted.identity).toBe("support/support-lab/tester");
  });
});

describe.each(COPIES)("a member served by the launch mob's door, through %s", (_name, create) => {
  it("is addressed by what its row says, not by what its key spells: a launch member whose name begins with a hosted root's id", async () => {
    // The launch mob's member `support/x`: its key is the key the hosted
    // root `support` would give a member `x`. Its row names the launch
    // mob's door and the launch mob as the mob it sits in.
    const gateway = createTwoRootGateway({ moreLaunchMembers: ["support/x"] });
    const transport = await connected(gateway, create);
    const accepted = await transport.send({ identity: "support/x", content: "hello", origin: "console", idempotencyKey: "by-the-row" });
    expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: undefined, identity: "support/x" });
    expect(accepted.identity).toBe("support/x");
    await transport.executeCommand!({ command: CONSOLE_COMMAND_NAMES.inspectIdentity, target: inspectTarget("support/x") as never }).catch(() => undefined);
    expect(named(last(gateway.calls("meta/console/inspect_identity")))).toEqual({ mob_id: undefined, identity: "support/x" });
    await transport.queryTimeline({ identity: "support/x", mode: "recent", limit: 50 });
    expect(named(last(gateway.calls("meta/console/query_timeline")))).toEqual({ mob_id: undefined, identity: "support/x" });
    // The hosted root's own members are addressed at its door, as before.
    await transport.send({ identity: "support/reviewer", content: "hello", origin: "console", idempotencyKey: "by-the-row-2" });
    expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: HOSTED_MOB, identity: "reviewer" });
  });

  it("is not addressed by a key two rows spell for two members: nothing is sent for it", async () => {
    // The launch mob's member `support/reviewer` and the hosted root's
    // member `reviewer` spell one key, and no reading of it tells them apart.
    const gateway = createTwoRootGateway({ moreLaunchMembers: ["support/reviewer"] });
    const transport = await connected(gateway, create);
    const refusals = [
      await transport.send({ identity: "support/reviewer", content: "hello", origin: "console", idempotencyKey: "shared" }).then(() => null, (error) => error as { kind?: string }),
      await transport.executeCommand!({ command: CONSOLE_COMMAND_NAMES.inspectIdentity, target: inspectTarget("support/reviewer") as never }).then(() => null, (error) => error as { kind?: string }),
      await transport.queryTimeline({ identity: "support/reviewer", mode: "recent", limit: 50 }).then(() => null, (error) => error as { kind?: string }),
    ];
    expect(refusals.map((refused) => refused?.kind)).toEqual(Array(3).fill("console-member-door-unknown"));
    expect(gateway.requests).toEqual([]);
    // Every other member is addressed as it was.
    await transport.send({ identity: "support/triage", content: "hello", origin: "console", idempotencyKey: "shared-2" });
    expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: HOSTED_MOB, identity: "triage" });
    await transport.send({ identity: "planner", content: "hello", origin: "console", idempotencyKey: "shared-3" });
    expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: undefined, identity: "planner" });
  });

  it("is named as before, with no mob: a launch member and a member of a mob nested under it", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    for (const identity of ["reviewer", "research/scout"]) {
      const accepted = await transport.send({ identity, content: "hello", origin: "console", idempotencyKey: `launch:${identity}` });
      expect(named(last(gateway.calls("meta/console/send")))).toEqual({ mob_id: undefined, identity });
      expect(accepted.identity).toBe(identity);
      await transport.queryTimeline({ identity, mode: "recent", limit: 50 });
      expect(named(last(gateway.calls("meta/console/query_timeline")))).toEqual({ mob_id: undefined, identity });
    }
  });
});

// The row a person acted on decides the door: a command sent for that row
// (`about`) goes to the row's door, whatever name its params carry. A name
// an answer gave is never read as a member of the launch mob.
describe.each(COPIES)("a command sent for the row a person acted on, through %s", (_name, create) => {
  const roster = { id: "roster", kind: "meta/roster", title: "Roster" } as never;
  const run = (transport: MetaConsoleTransport, command: string, params: Record<string, unknown>, about: string, more: Record<string, unknown> = {}) =>
    transport.executeCommand!({ command, target: roster, params, about, ...more } as never);

  it("goes to that row's door with the bare name an answer gave, and with the console's key for the row alike", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    for (const name of ["reviewer", "support/reviewer"]) {
      await run(transport, CONSOLE_COMMAND_NAMES.archiveAgent, { name, request_id: `request:${name}` }, "support/reviewer");
      const sent = last(gateway.calls("meta/identity/archive"));
      expect({ mob_id: sent?.params?.mob_id, name: sent?.params?.name }, name).toEqual({ mob_id: HOSTED_MOB, name: "reviewer" });
    }
  });

  it("carries that row's door where the command names no agent: a held change by its request id, the directory", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    for (const [command, method, params] of [
      [CONSOLE_COMMAND_NAMES.withdrawAgentChange, "meta/identity/withdraw", { request_id: "request-1" }],
      [CONSOLE_COMMAND_NAMES.getAgentChange, "meta/identity/operation", { request_id: "request-1" }],
      [CONSOLE_COMMAND_NAMES.listAgentDirectory, "meta/identity/directory", {}],
    ] as const) {
      await run(transport, command, { ...params }, "support/reviewer").catch(() => undefined);
      const sent = last(gateway.calls(method));
      expect(sent?.params?.mob_id, method).toBe(HOSTED_MOB);
      expect(Object.keys(sent?.params ?? {}), method).not.toContain("name");
    }
  });

  it("is sent as before for a row of the launch mob and of a mob nested under it", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    for (const about of ["reviewer", "research/scout"]) {
      await run(transport, CONSOLE_COMMAND_NAMES.archiveAgent, { name: about, request_id: "request-2" }, about).catch(() => undefined);
      const sent = last(gateway.calls("meta/identity/archive"));
      expect({ mob_id: sent?.params?.mob_id, name: sent?.params?.name }, about).toEqual({ mob_id: undefined, name: about });
      await run(transport, CONSOLE_COMMAND_NAMES.withdrawAgentChange, { request_id: "request-2" }, about).catch(() => undefined);
      expect(Object.keys(last(gateway.calls("meta/identity/withdraw"))?.params ?? {}), about).not.toContain("mob_id");
    }
  });

  it("is not sent on the console's own for a row whose mob is not loaded", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    const transport = await connected(gateway, create);
    const held = await run(transport, CONSOLE_COMMAND_NAMES.getAgentChange, { request_id: "request-3" }, "support/reviewer", { automatic: true })
      .then(() => null, (error) => error as { kind?: string });
    expect(held?.kind).toBe("console-mob-not-loaded");
    expect(gateway.requests).toEqual([]);
  });
});

describe.each(COPIES)("a command that names its member by another word, through %s", (_name, create) => {
  const chatTarget = (identity: string) => ({ id: identity, kind: "meta/identity-chat" as const, identity, memberId: identity, addressingMode: "identity" as const, title: "Chat" });

  it("stops a run of a hosted root's member by its door", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.stopMemberRun,
      target: chatTarget("support/reviewer") as never,
      params: { member_id: "support/reviewer", run_id: "run-1", reason: "Stopped from the console" },
    });
    expect(last(gateway.calls("meta/stop_member_run"))?.params).toEqual({
      member_id: "reviewer", run_id: "run-1", reason: "Stopped from the console", mob_id: HOSTED_MOB, expected_caller_scope: "caller-operator",
    });
  });

  it("forks a hosted root's member by its door, and keeps the answer under the console's keys", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const forked = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.forkMember,
      target: chatTarget("support/reviewer") as never,
      params: { source_member_id: "support/reviewer", agent_identity: "helper" },
    });
    expect(last(gateway.calls("meta/fork_member"))?.params).toEqual({
      source_member_id: "reviewer", agent_identity: "helper", mob_id: HOSTED_MOB, expected_caller_scope: "caller-operator",
    });
    // The door named the new member and its source as it knows them.
    expect(forked.result).toEqual({ agent_identity: "support/helper", side_chat_of: "support/reviewer", display_name: "Helper" });
  });

  it("reads a hosted root's agent record by its door", async () => {
    const gateway = createTwoRootGateway({ methods: ["meta/identity/get"] });
    const transport = await connected(gateway, create);
    const read = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.getAgentRecord,
      target: inspectTarget("support/reviewer") as never,
      params: { name: "support/reviewer" },
    });
    const sent = last(gateway.calls("meta/identity/get"));
    expect({ mob_id: sent?.params?.mob_id, name: sent?.params?.name }).toEqual({ mob_id: HOSTED_MOB, name: "reviewer" });
    // The door answers with the agent as it knows it; the record is handed on under the console's key.
    expect((read.result as { name?: string }).name).toBe("support/reviewer");
  });

  it("hands on a door's schedules under the console's key for the agent each belongs to", async () => {
    const any = { kind: "any" };
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS),
      hostedSchedules: [{
        schedule_id: "sch-1", identity: "triage", name: "Morning sort", content: "Sort.",
        trigger: { type: "calendar", timezone: "UTC", minute: { kind: "values", values: [0] }, hour: { kind: "values", values: [9] }, day_of_month: any, month: any, day_of_week: any },
        phase: "active", revision: 1, sender: "reviewer",
      }],
    });
    const transport = await connected(gateway, create);
    const listed = (await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.listSchedules,
      target: inspectTarget("support/triage") as never,
    })).result as { identity?: string; schedules?: Array<{ identity?: string; sender?: string }> };
    // The door names the agent as it knows it; the console keeps each schedule under its own key for the agent.
    expect(listed.identity).toBe("support/triage");
    expect(listed.schedules?.map((schedule) => schedule.identity)).toEqual(["support/triage"]);
    // Who it sends as may be a person: left as the door wrote it.
    expect(listed.schedules?.map((schedule) => schedule.sender)).toEqual(["reviewer"]);
  });

  it("hands on the peers an inspect answer names as members of that door's mob, never as launch members of the same name", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const inspect = async (identity: string) => (await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      target: inspectTarget(identity),
    })).result as { peers?: string[] };
    // The hosted root's `triage` is wired to the hosted root's `reviewer`.
    expect((await inspect("support/triage")).peers).toEqual(["support/reviewer"]);
    // The launch mob's `planner` to the launch mob's: the answer of before.
    expect((await inspect("planner")).peers).toEqual(["reviewer"]);
  });

  it("changes a hosted root's agent by its door, and keeps the answer under the console's key", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    for (const [command, method] of [
      [CONSOLE_COMMAND_NAMES.updateAgent, "meta/identity/update"],
      [CONSOLE_COMMAND_NAMES.archiveAgent, "meta/identity/archive"],
      [CONSOLE_COMMAND_NAMES.unarchiveAgent, "meta/identity/unarchive"],
      [CONSOLE_COMMAND_NAMES.removeAgent, "meta/identity/remove"],
    ] as const) {
      const changed = await transport.executeCommand!({
        command,
        target: { id: "roster", kind: "meta/roster", title: "Roster" } as never,
        params: { name: "support/reviewer", request_id: `request:${method}` },
      });
      const sent = last(gateway.calls(method));
      expect({ mob_id: sent?.params?.mob_id, name: sent?.params?.name }, method).toEqual({ mob_id: HOSTED_MOB, name: "reviewer" });
      expect(changed.result, method).toEqual({ name: "support/reviewer" });
    }
  });

  it("asks for the plan of a change to a hosted root's agent by that agent's door", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.planAgentChange,
      target: { id: "roster", kind: "meta/roster", title: "Roster" } as never,
      params: { name: "support/reviewer", change: "archive", request_id: "request:plan" },
    }).catch(() => undefined);
    const sent = last(gateway.calls("meta/identity/plan"));
    expect({ mob_id: sent?.params?.mob_id, name: sent?.params?.name }).toEqual({ mob_id: HOSTED_MOB, name: "reviewer" });
    // The plan of a launch agent's change: the request of before.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.planAgentChange,
      target: { id: "roster", kind: "meta/roster", title: "Roster" } as never,
      params: { name: "planner", change: "archive", request_id: "request:plan-2" },
    }).catch(() => undefined);
    const launch = last(gateway.calls("meta/identity/plan"));
    expect({ mob_id: launch?.params?.mob_id, name: launch?.params?.name }).toEqual({ mob_id: undefined, name: "planner" });
  });

  it("is refused by a gateway that hosts mobs when it names a hosted root's member and no mob, whatever the word", async () => {
    // A request that names a hosted root's member and no mob: the scripted
    // gateway serves a member by the door of its mob only.
    const gateway = createTwoRootGateway();
    const ask = async (method: string, params: Record<string, unknown>) => {
      const response = await gateway.fetch("/console/rpc", { method: "POST", body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }) });
      return (await response.json() as { error?: { data?: { kind?: string } } }).error?.data?.kind ?? "answered";
    };
    expect(await ask("meta/stop_member_run", { member_id: "support/reviewer", run_id: "run-1" })).toBe("identity_not_found");
    expect(await ask("meta/fork_member", { source_member_id: "support/reviewer", agent_identity: "helper" })).toBe("identity_not_found");
    expect(await ask("meta/identity/get", { name: "support/reviewer" })).toBe("identity_not_found");
    expect(await ask("meta/identity/archive", { name: "support/reviewer" })).toBe("identity_not_found");
    expect(await ask("meta/stop_member_run", { member_id: "reviewer", run_id: "run-1", mob_id: HOSTED_MOB })).toBe("answered");
    expect(await ask("meta/stop_member_run", { member_id: "reviewer", run_id: "run-1" })).toBe("answered");
  });

  it("keeps a schedule list's answer under the console's key for the member", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const listed = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.listSchedules,
      target: inspectTarget("support/triage") as never,
      params: { identity: "support/triage" },
    });
    expect(listed.result).toEqual({ identity: "support/triage", schedules: [] });
  });

  it("names a launch mob's member as before, and hands its answer on as it came", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const forked = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.forkMember,
      target: chatTarget("research/scout") as never,
      params: { source_member_id: "research/scout", agent_identity: "helper" },
    });
    expect(last(gateway.calls("meta/fork_member"))?.params).toEqual({
      source_member_id: "research/scout", agent_identity: "helper", expected_caller_scope: "caller-operator",
    });
    expect(forked.result).toEqual({ agent_identity: "helper", side_chat_of: "research/scout", display_name: "Helper" });
  });

  it("is held back when the console makes it on its own for a member of a mob listed as asleep", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    const transport = await connected(gateway, create);
    const held = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.getAgentRecord,
      target: inspectTarget("support/reviewer") as never,
      params: { name: "support/reviewer" },
      automatic: true,
    } as never).then(() => "answered", (error: { kind?: string }) => error.kind);
    expect(held).toBe("console-mob-not-loaded");
    expect(gateway.requests).toEqual([]);
  });
});

describe.each(COPIES)("a method of the host, through %s", (_name, create) => {
  it("is sent no mob, whatever member its params name: it answers from the host's values", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.listGatingPending,
      target: { id: "gating", kind: "meta/gating", title: "Approvals" } as never,
      params: { identity: "support/reviewer" },
    }).catch(() => undefined);
    const sent = last(gateway.requests);
    expect(sent?.method).toBe(CONSOLE_HOST_SCOPED_RPC_METHODS.find((method) => method === sent?.method));
    expect(named(sent)).toEqual({ mob_id: undefined, identity: "support/reviewer" });
  });

  // The access preview is the one method of the host the console sends with
  // a member (contract: CONSOLE_HOST_MEMBER_PARAM_BY_METHOD). For a member
  // of a hosted mob it carries the member's row key and no mob: the gateway
  // judges the preview in the member's own mob.
  it("the access preview about a hosted member carries the member's row key and no mob", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.previewAccess,
      target: { id: "access", kind: "meta/access", title: "Access" } as never,
      params: { subject: "someone@example.test", action: "console.send", identity: "support/reviewer" },
    }).catch(() => undefined);
    const sent = last(gateway.requests);
    expect(sent?.method).toBe(CONSOLE_RPC_METHODS.accessPreview);
    expect(sent?.params).toMatchObject({ subject: "someone@example.test", action: "console.send", identity: "support/reviewer" });
    expect(Object.keys(sent?.params ?? {})).not.toContain("mob_id");
    // About a launch member: the same request as before hosted mobs.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.previewAccess,
      target: { id: "access", kind: "meta/access", title: "Access" } as never,
      params: { action: "console.send", identity: "planner" },
    }).catch(() => undefined);
    expect(named(last(gateway.requests))).toEqual({ mob_id: undefined, identity: "planner" });
  });
});

// A command reaches a member's door only where the contract says how its
// method names a member (CONSOLE_MEMBER_PARAMS_BY_METHOD: the member of
// the params, a dotted path, `*` for every entry). Any other command that
// names a member of a hosted mob is not sent: sent as it is, it would be
// answered for a member of the launch mob that has the same name.
describe.each(COPIES)("a command that names a hosted member, through %s", (_name, create) => {
  const routingTarget = { id: "routing", kind: "meta/routing", title: "Routing" } as never;
  const sentTo = (gateway: TwoRootGateway, method: string) => last(gateway.calls(method))?.params as Record<string, unknown> | undefined;

  it("is not sent when the contract names no member for its method, and the person is told", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    // The routes are read for one mob and name no member.
    const refused = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.listRoutingRoutes,
      target: routingTarget,
      params: { identity: "support/reviewer" },
    }).then(() => null, (error) => error as { kind?: string; message?: string });
    expect(refused?.kind).toBe("console-member-door-unknown");
    expect(refused?.message).toBe("This action is not available yet for an agent of this team. Nothing was sent.");
    expect(gateway.requests).toEqual([]);

    // About a launch member, and about a member of a mob nested under the launch mob: sent as before.
    for (const identity of ["planner", "research/scout"]) {
      await transport.executeCommand!({ command: CONSOLE_COMMAND_NAMES.listRoutingRoutes, target: routingTarget, params: { identity } }).catch(() => undefined);
      expect(named(last(gateway.requests))).toEqual({ mob_id: undefined, identity });
    }
  });

  it("names the mob for exactly the methods the contract says name a member, sends a method of the host as it is, and sends no other", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const member = "support/reviewer";
    const words = ["identity", "member_id", "source_member_id", "name"];
    const byDoor = new Set<string>();
    const withoutMob: string[] = [];
    const unsent: Array<string | undefined> = [];
    const commandMethods = new Set<string>();
    for (const command of Object.values(CONSOLE_COMMAND_NAMES)) {
      commandMethods.add(consoleCommandMethod(command));
      gateway.clear();
      const failed = await transport.executeCommand!({
        command,
        target: inspectTarget(member) as never,
        params: { identity: member, member_id: member, source_member_id: member, name: member },
      }).then(() => null, (error) => error as { kind?: string });
      const sent = last(gateway.requests);
      if (!sent) unsent.push(failed?.kind);
      else if (sent.params?.mob_id === HOSTED_MOB) byDoor.add(sent.method || "");
      else withoutMob.push(sent.method || "");
    }
    // The contract's methods that name their member by one of these words at the top of the params.
    const named = Object.entries(CONSOLE_MEMBER_PARAMS_BY_METHOD)
      .filter(([method, paths]) => commandMethods.has(method) && paths.some((path) => words.includes(path)))
      .map(([method]) => method);
    expect(named.length).toBeGreaterThan(20);
    expect([...byDoor].sort()).toEqual([...named].sort());
    // Sent with no mob: the methods of the host only.
    expect(withoutMob.filter((method) => !(CONSOLE_HOST_SCOPED_RPC_METHODS as readonly string[]).includes(method))).toEqual([]);
    expect(withoutMob.length).toBeGreaterThan(0);
    // Every other command was not sent, each for this one reason.
    expect(unsent.length).toBeGreaterThan(0);
    expect([...new Set(unsent)]).toEqual(["console-member-door-unknown"]);
  });

  it("names the member where the contract's path says, also deep in the params", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    // A work item of a card in a hosted root's agent's chat, handed to an
    // agent of that root: `target.identity`.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.workgraphAttentionReassign,
      target: { id: "workgraph", kind: "meta/workgraph", title: "Work" } as never,
      params: { item_id: "w-1", target: { kind: "identity", identity: "support/reviewer" } },
      about: "support/triage",
    } as never).catch(() => undefined);
    expect(sentTo(gateway, CONSOLE_RPC_METHODS.workgraphAttentionReassign)).toMatchObject({ mob_id: HOSTED_MOB, target: { kind: "identity", identity: "reviewer" } });
    // With no row named, a work graph command is the launch mob's door's:
    // a work item is reassigned within the mob whose graph holds it, so one
    // that names a hosted root's agent is not sent.
    gateway.clear();
    const refused = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.workgraphAttentionReassign,
      target: { id: "workgraph", kind: "meta/workgraph", title: "Work" } as never,
      params: { item_id: "w-1", target: { kind: "identity", identity: "support/reviewer" } },
    }).then(() => null, (error) => error as { kind?: string });
    expect(refused?.kind).toBe("console-member-door-unknown");
    expect(gateway.requests).toEqual([]);

    // A wiring between two agents of the hosted root: every operation's two ends.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.topologyPlan,
      target: { id: "topology", kind: "meta/topology", title: "Topology" } as never,
      params: { operations: [
        { op: "wire", edge: { a: { identity: "support/reviewer" }, b: { identity: "support/triage" } } },
        { op: "unwire", edge: { a: { identity: "support/triage" }, b: { identity: "support-lab/tester" } } },
      ] },
    }).catch(() => undefined);
    expect(sentTo(gateway, CONSOLE_RPC_METHODS.topologyPlan)).toMatchObject({
      mob_id: HOSTED_MOB,
      operations: [
        { op: "wire", edge: { a: { identity: "reviewer" }, b: { identity: "triage" } } },
        { op: "unwire", edge: { a: { identity: "triage" }, b: { identity: "support-lab/tester" } } },
      ],
    });

    // A memory decision about a hosted root's agent goes to that root's door.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.decideMemoryQuarantine,
      target: { id: "memory", kind: "meta/memory", title: "Memory" } as never,
      params: { identity: "support/reviewer", memory_id: "m-1", verdict: "approve" },
    }).catch(() => undefined);
    expect(sentTo(gateway, CONSOLE_RPC_METHODS.memoryQuarantineDecide)).toMatchObject({ mob_id: HOSTED_MOB, identity: "reviewer" });

    // The same commands about launch agents: the requests of before, with no mob.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.topologyPlan,
      target: { id: "topology", kind: "meta/topology", title: "Topology" } as never,
      params: { operations: [{ op: "wire", edge: { a: { identity: "planner" }, b: { identity: "research/scout" } } }] },
    }).catch(() => undefined);
    const launch = sentTo(gateway, CONSOLE_RPC_METHODS.topologyPlan);
    expect(launch).toMatchObject({ operations: [{ op: "wire", edge: { a: { identity: "planner" }, b: { identity: "research/scout" } } }] });
    expect(Object.keys(launch ?? {})).not.toContain("mob_id");
  });

  it("is not sent when its members are behind two doors of their own: one request reaches one door", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const refused = await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.topologyPlan,
      target: { id: "topology", kind: "meta/topology", title: "Topology" } as never,
      params: { operations: [{ op: "wire", edge: { a: { identity: "support/reviewer" }, b: { identity: "archive/keeper" } } }] },
    }).then(() => null, (error) => error as { kind?: string });
    expect(refused?.kind).toBe("console-member-door-unknown");
    expect(gateway.requests).toEqual([]);
  });

  it("is not sent when it names a hosted member beside a member behind the launch mob's door: the launch mob's door is a door", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    // A launch member, and a member of a mob nested under the launch mob.
    for (const behindTheLaunchDoor of ["planner", "research/scout"]) {
      for (const [a, b] of [["support/reviewer", behindTheLaunchDoor], [behindTheLaunchDoor, "support/reviewer"]]) {
        const refused = await transport.executeCommand!({
          command: CONSOLE_COMMAND_NAMES.topologyPlan,
          target: { id: "topology", kind: "meta/topology", title: "Topology" } as never,
          params: { operations: [{ op: "wire", edge: { a: { identity: a }, b: { identity: b } } }] },
        }).then(() => null, (error) => error as { kind?: string; message?: string });
        expect(refused?.kind, `${a} and ${b}`).toBe("console-member-door-unknown");
        expect(refused?.message, `${a} and ${b}`).toBe("This action is not available yet for an agent of this team. Nothing was sent.");
      }
    }
    expect(gateway.requests).toEqual([]);
  });

  it("is not sent for a row behind one door when its params name a member behind another, the launch mob's door among them", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const reassign = (about: string, identity: string) =>
      transport.executeCommand!({
        command: CONSOLE_COMMAND_NAMES.workgraphAttentionReassign,
        target: { id: "workgraph", kind: "meta/workgraph", title: "Work" } as never,
        params: { binding_id: "b-1", target: { kind: "identity", identity } },
        about,
      } as never).then(() => null, (error) => error as { kind?: string });
    // A row of the launch mob's door, and a hosted root's agent as the new owner.
    // No command of the page is of this shape: the commands it sends for a
    // row name no member. The rule holds for any that does.
    for (const about of ["planner", "research/scout"]) {
      expect((await reassign(about, "support/reviewer"))?.kind, about).toBe("console-member-door-unknown");
    }
    // A hosted root's row, and a member of a mob nested under the launch mob.
    expect((await reassign("support/reviewer", "research/scout"))?.kind).toBe("console-member-door-unknown");
    // A hosted root's row, and an agent of another hosted root.
    expect((await reassign("support/reviewer", "archive/keeper"))?.kind).toBe("console-member-door-unknown");
    expect(gateway.requests).toEqual([]);

    // One door is served as it was: a hosted row with an agent of its door,
    // by the console's key and by the name the door's own answer gave.
    for (const identity of ["support/triage", "triage"]) {
      await reassign("support/reviewer", identity);
      expect(sentTo(gateway, CONSOLE_RPC_METHODS.workgraphAttentionReassign), identity)
        .toMatchObject({ mob_id: HOSTED_MOB, binding_id: "b-1", target: { kind: "identity", identity: "triage" } });
    }
    // And a row of the launch mob's door with a member behind it.
    gateway.clear();
    await reassign("planner", "research/scout");
    const launch = sentTo(gateway, CONSOLE_RPC_METHODS.workgraphAttentionReassign);
    expect(launch).toMatchObject({ binding_id: "b-1", target: { kind: "identity", identity: "research/scout" } });
    expect(Object.keys(launch ?? {})).not.toContain("mob_id");
  });

  it("is not sent for a value of the launch mob's door when it names a hosted member: the docked work graph is that door's", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    // The docked panel shows the graph of the launch mob's door and names
    // no row: a binding of that graph is not handed to another door's agent.
    const reassign = (identity: string) =>
      transport.executeCommand!({
        command: CONSOLE_COMMAND_NAMES.workgraphAttentionReassign,
        target: { id: "workgraph", kind: "meta/workgraph", title: "Work" } as never,
        params: { binding_id: "b-1", target: { kind: "identity", identity } },
        launchDoor: true,
      } as never).then(() => null, (error) => error as { kind?: string });
    for (const hosted of ["support/reviewer", "support/support-lab/tester", "archive/keeper"]) {
      expect((await reassign(hosted))?.kind, hosted).toBe("console-member-door-unknown");
    }
    expect(gateway.requests).toEqual([]);
    for (const behindTheLaunchDoor of ["planner", "research/scout"]) {
      await reassign(behindTheLaunchDoor);
      const sent = sentTo(gateway, CONSOLE_RPC_METHODS.workgraphAttentionReassign);
      expect(sent, behindTheLaunchDoor).toMatchObject({ binding_id: "b-1", target: { kind: "identity", identity: behindTheLaunchDoor } });
      expect(Object.keys(sent ?? {}), behindTheLaunchDoor).not.toContain("mob_id");
    }
  });

  it("withdraws and reads a held change by its request id with no member: the mob alone, from the row it is sent for", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    // The contract names no member for these two: a `name` beside the request id is not rewritten.
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.withdrawAgentChange,
      target: { id: "roster", kind: "meta/roster", title: "Roster" } as never,
      params: { request_id: "request-1" },
      about: "support/reviewer",
    } as never).catch(() => undefined);
    expect(sentTo(gateway, CONSOLE_RPC_METHODS.identityWithdraw)).toEqual(expect.objectContaining({ mob_id: HOSTED_MOB, request_id: "request-1" }));
    expect(Object.keys(sentTo(gateway, CONSOLE_RPC_METHODS.identityWithdraw) ?? {})).not.toContain("name");
  });
});

describe.each(COPIES)("the reads over the host's values, through %s", (_name, create) => {
  it("ask for every mob the caller may address, and name no mob", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    await transport.loadExperience();
    await transport.queryTimeline({ mode: "recent", limit: 50 });
    const stop = transport.subscribeTimeline({}, () => {});
    await new Promise((resolve) => setTimeout(resolve, 0));
    stop();
    const experience = last(gateway.requests.filter((request) => request.route === "GET /console/experience"));
    const stream = last(gateway.requests.filter((request) => request.route === "GET /console/timeline/stream"));
    const read = last(gateway.calls("meta/console/query_timeline"));
    expect(experience?.query).toEqual([["mobs", "*"]]);
    expect(stream?.query).toEqual([["mobs", "*"]]);
    expect(read?.params?.mobs).toBe("*");
    expect(read?.params && "mob_id" in read.params).toBe(false);
  });

  it("list the hosted roots only once the console asks for every mob", async () => {
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    const transport = create({ baseUrl: "" }) as MetaConsoleTransport;
    const mobIds = async () => {
      const experience = await transport.loadExperience() as { topology?: { live_snapshot?: { mobs?: Array<{ mob_id: string }> } } };
      return (experience.topology?.live_snapshot?.mobs ?? []).map((mob) => mob.mob_id);
    };
    expect(await mobIds()).toEqual(["ops-team", "research"]);
    await transport.capabilities();
    expect(await mobIds()).toEqual(["ops-team", "research", "support", "support-lab", "archive", "billing"]);
  });
});
