import { afterEach, describe, expect, it, vi } from "vitest";

import { createHttpConsoleTransport as libraryTransport } from "../../../packages/console-core/src/headless";
import type { ConsoleFrame } from "../types";
import { createTwoRootGateway, HOSTED_MOB, type TwoRootGateway } from "./fixtures/two-root-gateway";
import { createHttpConsoleTransport as consoleTransport, type MetaConsoleTransport } from "./headless";

// A frame of a mob the host created beside the launch mob names its mob
// beside the member. The console keys it by the member's own row, never by
// the launch mob's member of the same name.

const COPIES = [
  ["the console's transport", consoleTransport],
  ["the library's transport", libraryTransport],
] as const;

async function connected(gateway: TwoRootGateway, create: typeof consoleTransport): Promise<MetaConsoleTransport> {
  vi.stubGlobal("fetch", gateway.fetch);
  const transport = create({ baseUrl: "" }) as MetaConsoleTransport;
  await transport.capabilities();
  await transport.loadExperience();
  return transport;
}

const until = async (done: () => boolean) => {
  for (let turn = 0; turn < 200 && !done(); turn += 1) await new Promise((resolve) => setTimeout(resolve, 5));
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe.each(COPIES)("a frame on the console's one stream, through %s", (_name, create) => {
  it("lands on the row of its own mob's member: two members of one name stay apart", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const seen: ConsoleFrame[] = [];
    const stop = transport.subscribeTimeline({}, (frame) => seen.push(frame as ConsoleFrame));
    await new Promise((resolve) => setTimeout(resolve, 0));
    gateway.push({ identity: "reviewer", kind: "text_delta", payload: { delta: "from the launch mob" } });
    gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "text_delta", payload: { delta: "from the hosted root" } });
    gateway.push({ mob: HOSTED_MOB, identity: "support-lab/tester", kind: "text_delta", payload: { delta: "from the nested mob" } });
    gateway.push({ identity: "research/scout", kind: "text_delta", payload: { delta: "from the launch mob's nested mob" } });
    await until(() => seen.length >= 4);
    stop();
    expect(seen.map((frame) => frame.identity)).toEqual(["reviewer", "support/reviewer", "support/support-lab/tester", "research/scout"]);
  });

  it("is keyed the same way in a timeline read of the member and of every mob", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    gateway.push({ identity: "reviewer", kind: "text_delta", payload: { delta: "launch" } });
    gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "text_delta", payload: { delta: "hosted" } });
    const own = await transport.queryTimeline({ identity: "support/reviewer", mode: "recent", limit: 50 });
    expect(own.frames.map((frame) => frame.identity)).toEqual(["support/reviewer"]);
    const launch = await transport.queryTimeline({ identity: "reviewer", mode: "recent", limit: 50 });
    expect(launch.frames.map((frame) => frame.identity)).toEqual(["reviewer"]);
    const every = await transport.queryTimeline({ mode: "recent", limit: 50 });
    expect(every.frames.map((frame) => frame.identity)).toEqual(["reviewer", "support/reviewer"]);
  });

  it("is not handed on while its mob is in no list the console holds: neither from the stream nor from a read", async () => {
    const gateway = createTwoRootGateway();
    const transport = await connected(gateway, create);
    const seen: ConsoleFrame[] = [];
    const stop = transport.subscribeTimeline({}, (frame) => seen.push(frame as ConsoleFrame));
    await new Promise((resolve) => setTimeout(resolve, 0));
    gateway.push({ mob: "newcomer", identity: "reviewer", kind: "text_delta", payload: { delta: "from a mob not listed" } });
    gateway.push({ mob: "newcomer", identity: "reviewer", kind: "interaction_complete", interactionId: "n-1", payload: {} });
    gateway.push({ identity: "planner", kind: "text_delta", payload: { delta: "after it" } });
    await until(() => seen.some((frame) => frame.identity === "planner"));
    stop();
    expect(seen.map((frame) => frame.identity)).toEqual(["planner"]);
    const every = await transport.queryTimeline({ mode: "recent", limit: 50 });
    expect(every.frames.map((frame) => frame.identity)).toEqual(["planner"]);
  });
});
