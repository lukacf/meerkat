import React from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, namesAMob, type RecordedRequest, type TwoRootGateway } from "./fixtures/two-root-gateway";

// THE GUARD: a console on a gateway that hosts one mob sends the requests
// it always sent. The session below (open the console, write to a member of
// the launch mob, open a member of a mob nested under it, write to it) is
// recorded request by request and compared with the list a console sent
// before it knew of hosted mobs. A change to this list is a change to what
// every existing deployment receives: it is never updated to make a row
// pass.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

async function write(identity: string, text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
  await settle();
}

/** A request as the guard compares it: what was asked, without the values a clock or a random source made. */
function comparable(request: RecordedRequest): string {
  const params = request.params === undefined ? undefined : { ...request.params };
  if (params && typeof params.idempotency_key === "string") params.idempotency_key = "<key>";
  if (params && typeof params.origin === "string") params.origin = params.origin.replace(/panel-[0-9a-f-]{36}/, "panel-<id>");
  const query = request.query.map(([name, value]) => `${name}=${value}`).join("&");
  return [
    request.route + (query ? `?${query}` : ""),
    request.method ?? "",
    params === undefined ? "" : JSON.stringify(params),
    request.lastEventId ? `Last-Event-ID=${request.lastEventId}` : "",
  ].filter(Boolean).join(" ");
}

/** The session every guard runs. */
async function session(gateway: TwoRootGateway): Promise<string[]> {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId("chat-composer:planner");
  await settle(500);
  await write("planner", "hello planner");
  await act(async () => { fireEvent.click(await screen.findByTestId("sidebar-agent:research/scout")); });
  await write("research/scout", "hello scout");
  await settle(500);
  return gateway.requests.map(comparable);
}

let restoreAbortSignal = () => {};
beforeEach(() => {
  restoreAbortSignal = ensureAbortSignalAny();
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.unstubAllGlobals(); restoreAbortSignal();
});

const send = (identity: string, content: string) =>
  `POST /console/rpc meta/console/send ${JSON.stringify({
    identity,
    content,
    origin: "console:panel-<id>",
    origin_kind: "operator",
    idempotency_key: "<key>",
    handling_mode: "queue",
    expected_caller_scope: "caller-operator",
  })}`;
const timelineOf = (identity?: string) =>
  `POST /console/rpc meta/console/query_timeline ${JSON.stringify({ limit: 200, ...(identity ? { identity } : {}), mode: "recent" })}`;
const CAPABILITIES = "POST /console/rpc meta/capabilities {}";
const STREAM = "GET /console/timeline/stream";

/** What a console sent in this session before it knew of hosted mobs. */
const REQUESTS_OF_TODAY: string[] = [
  "POST /console/rpc meta/console/whoami {}",
  "GET /console/experience",
  "GET /console/modules",
  timelineOf(),
  CAPABILITIES,
  STREAM,
  CAPABILITIES,
  timelineOf(),
  timelineOf("planner"),
  CAPABILITIES,
  STREAM,
  CAPABILITIES,
  CAPABILITIES,
  send("planner", "hello planner"),
  timelineOf("research/scout"),
  CAPABILITIES,
  send("research/scout", "hello scout"),
];

describe("a console on a gateway that hosts one mob", () => {
  it("sends the requests of today, each as it was and in the order it was", async () => {
    const sent = await session(createTwoRootGateway({ addressing: false }));
    expect(sent).toEqual(REQUESTS_OF_TODAY);
  });

  it("names no mob in any request", async () => {
    const gateway = createTwoRootGateway({ addressing: false });
    await session(gateway);
    expect(gateway.requests.filter(namesAMob)).toEqual([]);
  });
});
