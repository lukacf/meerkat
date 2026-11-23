import { afterEach, describe, expect, it, vi } from "vitest";

import type { MemberDoor } from "../../../packages/console-core/src/mob-door";
import { createNativeMcpAppsHost } from "../mcp-apps/native-host";
import { createConsolePanelService } from "./custom-panels";
import { createTwoRootGateway, HOSTED_MOB, namesAMob, type RecordedRequest } from "./fixtures/two-root-gateway";
import { createHttpConsoleTransport } from "./headless";
import { createVoiceRpc, queryVoiceAvailability, voiceParamsByDoor } from "./voice-session";

// The requests that do not go through the transport (voice, MCP Apps, a
// custom panel's service) name a member of a hosted root the way the
// transport does: by its mob and by the member as that mob's door knows
// it. A member behind the launch mob's door is named as before.

type Door = (identity: string) => MemberDoor;

/** The door getter a running console hands these paths: its connection's. */
async function consoleDoor(gateway = createTwoRootGateway()): Promise<{ gateway: ReturnType<typeof createTwoRootGateway>; door: Door; heldBack: (identity: string) => boolean }> {
  vi.stubGlobal("fetch", vi.fn(gateway.fetch));
  const transport = createHttpConsoleTransport({ baseUrl: "" });
  await transport.capabilities();
  await transport.loadExperience();
  gateway.clear();
  return {
    gateway,
    door: (identity) => transport.mobAddressing!.door(identity),
    heldBack: (identity) => transport.mobAddressing!.heldBack(identity),
  };
}

const last = (requests: RecordedRequest[]) => requests[requests.length - 1];
const header = (name: string): Record<string, unknown> => {
  const calls = vi.mocked(fetch).mock.calls;
  const init = calls[calls.length - 1]?.[1] as RequestInit;
  return JSON.parse(decodeURIComponent(new Headers(init.headers).get(name) || "null")) as Record<string, unknown>;
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a voice request", () => {
  it("names a member of a hosted root by its mob and its bare name", async () => {
    const { gateway, door } = await consoleDoor();
    const rpc = (createVoiceRpc as (...args: unknown[]) => ReturnType<typeof createVoiceRpc>)("", undefined, undefined, door);
    await rpc("meta/console/voice/close", { identity: "support/reviewer", request_id: "r1" }, 1_000);
    const sent = last(gateway.calls("meta/console/voice/close"));
    expect({ mob_id: sent?.params?.mob_id, identity: sent?.params?.identity, request_id: sent?.params?.request_id })
      .toEqual({ mob_id: HOSTED_MOB, identity: "reviewer", request_id: "r1" });
  });

  it("names a member behind the launch mob's door as before", async () => {
    const { gateway, door } = await consoleDoor();
    const rpc = (createVoiceRpc as (...args: unknown[]) => ReturnType<typeof createVoiceRpc>)("", undefined, undefined, door);
    await rpc("meta/console/voice/close", { identity: "research/scout", request_id: "r2" }, 1_000);
    expect(last(gateway.calls("meta/console/voice/close"))?.params).toEqual({ identity: "research/scout", request_id: "r2" });
  });

  it("is not made to ask whether voice is ready for a member of a hosted root: voice is the launch mob's", async () => {
    const { gateway, door } = await consoleDoor();
    const availability = await (queryVoiceAvailability as (...args: unknown[]) => ReturnType<typeof queryVoiceAvailability>)("", "support/reviewer", undefined, door);
    expect(availability).toBe("unavailable");
    expect(gateway.requests).toEqual([]);
    await (queryVoiceAvailability as (...args: unknown[]) => ReturnType<typeof queryVoiceAvailability>)("", "reviewer", undefined, door);
    expect(last(gateway.calls("meta/console/voice/readiness"))?.params).toEqual({ identity: "reviewer" });
  });
  it("names the member the same way in the close a leaving page sends", async () => {
    const { door } = await consoleDoor();
    expect(voiceParamsByDoor({ identity: "support/triage", request_id: "r3" }, door))
      .toEqual({ identity: "triage", mob_id: HOSTED_MOB, request_id: "r3" });
    const launch = { identity: "planner", request_id: "r4" };
    expect(voiceParamsByDoor(launch, door)).toBe(launch);
    expect(voiceParamsByDoor(launch)).toBe(launch);
  });
});

describe("an MCP App request", () => {
  const locator = { identity: "support/reviewer", sessionId: "session-1", toolCallId: "tool-1" };
  const host = (memberDoor: Door) => createNativeMcpAppsHost({
    baseUrl: "",
    sandboxProxyUrl: "https://sandbox.test/sandbox.html",
    authority: "authority-1",
    memberDoor,
  } as Parameters<typeof createNativeMcpAppsHost>[0]);

  it("is not made for a member of a hosted root, loaded or not: interactive views are the launch mob's in this version", async () => {
    // The routes of interactive views are served for the launch mob only.
    const { gateway, door } = await consoleDoor();
    expect(await host(door).resolve(locator, new AbortController().signal)).toBeNull();
    // A member of a mob nested under the hosted root: the same.
    expect(await host(door).resolve({ ...locator, identity: "support-lab/tester" }, new AbortController().signal)).toBeNull();
    expect(gateway.requests).toEqual([]);
  });

  it("names a member behind the launch mob's door as before", async () => {
    const { gateway, door } = await consoleDoor();
    await host(door).resolve({ ...locator, identity: "research/scout" }, new AbortController().signal).catch(() => undefined);
    const sent = last(gateway.requests);
    expect(sent?.params).toEqual({ identity: "research/scout", sessionId: "session-1", toolCallId: "tool-1" });
    expect(namesAMob(sent!)).toBe(false);
    expect(header("X-Console-Conversation")).toEqual({ scopeKey: "authority-1", identity: "research/scout" });
  });
  it("is not made for a member of a mob listed as asleep: the transcript shows without the app's view, and the mob stays asleep", async () => {
    const { gateway, door, heldBack } = await consoleDoor(createTwoRootGateway({ hostedState: "dormant" }));
    const asleepHost = createNativeMcpAppsHost({
      baseUrl: "",
      sandboxProxyUrl: "https://sandbox.test/sandbox.html",
      authority: "authority-1",
      memberDoor: door,
      memberHeldBack: heldBack,
    } as Parameters<typeof createNativeMcpAppsHost>[0]);
    expect(await asleepHost.resolve(locator, new AbortController().signal)).toBeNull();
    expect(gateway.requests).toEqual([]);
    expect(gateway.wakes()).toBe(0);
    // A member behind the launch mob's door is asked as before.
    await asleepHost.resolve({ ...locator, identity: "reviewer" }, new AbortController().signal).catch(() => undefined);
    expect(last(gateway.requests)?.route).toBe("POST /console/mcp-apps/resolve");
  });
});

describe("a custom panel's request", () => {
  const scope = (identity: string) => ({
    authority: { key: "authority-1" },
    conversation: { scopeKey: "authority-1", identity },
    readOnly: false,
  });
  const service = (memberDoor: Door) =>
    (createConsolePanelService as (...args: unknown[]) => ReturnType<typeof createConsolePanelService>)("", memberDoor);

  it("carries the locator of a hosted root's member by its mob and its bare name", async () => {
    const { door } = await consoleDoor();
    await service(door)({ path: "/modules/example/data" } as never, scope("support/reviewer") as never, new AbortController().signal).catch(() => undefined);
    expect(header("X-Console-Conversation")).toEqual({ scopeKey: "authority-1", identity: "reviewer", mob_id: HOSTED_MOB });
  });

  it("carries the locator of a member behind the launch mob's door as before", async () => {
    const { door } = await consoleDoor();
    await service(door)({ path: "/modules/example/data" } as never, scope("reviewer") as never, new AbortController().signal).catch(() => undefined);
    expect(header("X-Console-Conversation")).toEqual({ scopeKey: "authority-1", identity: "reviewer" });
  });
});
