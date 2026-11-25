import { afterEach, describe, expect, it, vi } from "vitest";

import { createHttpConsoleTransport as libraryTransport } from "../../../packages/console-core/src/headless";
import { createTwoRootGateway, namesAMob, type TwoRootGateway } from "./fixtures/two-root-gateway";
import { CONSOLE_COMMAND_NAMES, createHttpConsoleTransport as consoleTransport, type MetaConsoleTransport } from "./headless";

// A request names a mob (`mob_id`, `mobs`) only for a gateway that lists
// the `mob_addressing` capability. Any other gateway is sent the requests
// of a console that knows one mob, for every member it lists.

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

/** Everything a console asks of a member, and the reads over the host's values. */
async function everyRequest(transport: MetaConsoleTransport, identities: readonly string[]): Promise<void> {
  await transport.capabilities();
  await transport.loadExperience();
  await transport.queryTimeline({ mode: "recent", limit: 50 });
  const stop = transport.subscribeTimeline({}, () => {});
  for (const identity of identities) {
    await transport.send({ identity, content: "hello", origin: "console", idempotencyKey: `send:${identity}` }).catch(() => undefined);
    await transport.send({
      identity,
      content: "with a file",
      origin: "console",
      idempotencyKey: `upload:${identity}`,
      attachments: [new File(["x"], "note.png", { type: "image/png" })],
    }).catch(() => undefined);
    await transport.queryTimeline({ identity, mode: "recent", limit: 50 });
    await transport.executeCommand!({
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      target: inspectTarget(identity) as never,
    }).catch(() => undefined);
    const stopOne = transport.subscribeTimeline({ identity }, () => {});
    await new Promise((resolve) => setTimeout(resolve, 0));
    stopOne();
  }
  await new Promise((resolve) => setTimeout(resolve, 0));
  stop();
}

const MEMBERS = ["planner", "reviewer", "research/scout", "support/reviewer", "support/triage", "support/support-lab/tester"] as const;

function connect(gateway: TwoRootGateway, create: typeof consoleTransport): MetaConsoleTransport {
  vi.stubGlobal("fetch", gateway.fetch);
  return create({ baseUrl: "" }) as MetaConsoleTransport;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe.each(COPIES)("a gateway that does not list mob addressing, through %s", (_name, create) => {
  it("is sent no request that names a mob", async () => {
    const gateway = createTwoRootGateway({ addressing: false });
    await everyRequest(connect(gateway, create), MEMBERS);
    expect(gateway.requests.length).toBeGreaterThan(20);
    expect(gateway.requests.filter(namesAMob)).toEqual([]);
  });

  it("is sent each member's identity as the console keys it", async () => {
    const gateway = createTwoRootGateway({ addressing: false });
    await everyRequest(connect(gateway, create), MEMBERS);
    const named = gateway.calls("meta/console/send").map((request) => request.params?.identity);
    expect(named).toEqual(MEMBERS.flatMap((identity) => [identity, identity]));
  });
});

describe.each(COPIES)("a gateway whose capabilities the console has not read, through %s", (_name, create) => {
  it("is sent no request that names a mob", async () => {
    const gateway = createTwoRootGateway({ addressing: true });
    const transport = connect(gateway, create);
    await transport.loadExperience();
    await transport.send({ identity: "support/reviewer", content: "hello", origin: "console", idempotencyKey: "early" }).catch(() => undefined);
    await transport.queryTimeline({ mode: "recent", limit: 50 });
    expect(gateway.requests.filter(namesAMob)).toEqual([]);
  });
});
