import { afterEach, describe, expect, it, vi } from "vitest";

import { createHttpConsoleTransport as libraryTransport } from "../../../packages/console-core/src/headless";
import { ASLEEP_MOB, createTwoRootGateway, HOSTED_MOB, namesAMob, type TwoRootGateway } from "./fixtures/two-root-gateway";
import { CONSOLE_COMMAND_NAMES, createHttpConsoleTransport as consoleTransport, type MetaConsoleTransport } from "./headless";

// A mob that is not loaded stays not loaded until a person asks for it. A
// request the console makes on its own (a refresh, a repair after a gap in
// the stream, a refresh after a frame) says so, and the transport holds it
// back when its member's mob is listed as not loaded: nothing is sent. A
// person's request is sent, and the mob wakes for it.

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

async function connected(gateway: TwoRootGateway, create: typeof consoleTransport): Promise<MetaConsoleTransport> {
  vi.stubGlobal("fetch", gateway.fetch);
  const transport = create({ baseUrl: "" }) as MetaConsoleTransport;
  await transport.capabilities();
  await transport.loadExperience();
  gateway.clear();
  return transport;
}

const refusal = async (request: Promise<unknown>) => {
  try {
    await request;
    return "answered";
  } catch (error) {
    return (error as { kind?: unknown }).kind;
  }
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe.each(COPIES)("a request the console makes on its own, through %s", (_name, create) => {
  it("is held back for a member of a mob listed as asleep or unavailable: nothing is sent", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    const transport = await connected(gateway, create);
    for (const identity of ["support/reviewer", "archive/keeper", "billing/clerk"]) {
      expect(await refusal(transport.queryTimeline({ identity, mode: "recent", limit: 50, automatic: true } as never))).toBe("console-mob-not-loaded");
      expect(await refusal(transport.executeCommand!({
        command: CONSOLE_COMMAND_NAMES.inspectIdentity,
        target: inspectTarget(identity) as never,
        automatic: true,
      } as never))).toBe("console-mob-not-loaded");
    }
    expect(gateway.requests).toEqual([]);
  });

  it("is sent for a member of a loaded mob and for the launch mob's", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    for (const identity of ["support/reviewer", "reviewer", "research/scout"]) {
      expect(await refusal(transport.queryTimeline({ identity, mode: "recent", limit: 50, automatic: true } as never))).toBe("answered");
    }
    expect(gateway.calls("meta/console/query_timeline").length).toBe(3);
  });

  it("names no mob that is not loaded when it reads over the host's values", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    const transport = await connected(gateway, create);
    await transport.loadExperience();
    await transport.queryTimeline({ mode: "recent", limit: 50, automatic: true } as never);
    expect(gateway.requests.length).toBe(2);
    expect(gateway.requests.every((request) => !("mob_id" in (request.params ?? {})) && !request.query.some(([name]) => name === "mob_id"))).toBe(true);
  });
});

describe.each(COPIES)("a person's request, through %s", (_name, create) => {
  it("is sent for a member of a mob listed as asleep, by its door", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    const transport = await connected(gateway, create);
    await transport.queryTimeline({ identity: "support/reviewer", mode: "recent", limit: 50 });
    const read = gateway.calls("meta/console/query_timeline")[0];
    expect({ mob_id: read?.params?.mob_id, identity: read?.params?.identity }).toEqual({ mob_id: HOSTED_MOB, identity: "reviewer" });
    await transport.send({ identity: "support/reviewer", content: "wake up", origin: "console", idempotencyKey: "wake" });
    expect(gateway.calls("meta/console/send")[0]?.params?.mob_id).toBe(HOSTED_MOB);
  });

  it("opens a mob that is asleep with one ordinary request that names it", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    const transport = await connected(gateway, create);
    await (transport as MetaConsoleTransport & { openMob(mobId: string): Promise<void> }).openMob(ASLEEP_MOB);
    expect(gateway.requests.map((request) => [request.method, request.params])).toEqual([["meta/status", { mob_id: ASLEEP_MOB }]]);
    expect(gateway.requests.filter(namesAMob).length).toBe(1);
  });
});
