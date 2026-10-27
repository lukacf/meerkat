import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "./headless";
import { beginConsoleSendAttempt, createConsoleSendAttempt, finishConsoleSendAttempt, type ConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, saveConsoleSendAttempts } from "./send-attempt-storage";

// Pending-row language and the explicit Check / Send again actions (Meta
// 0.8.45). Every Check ends in a visible result; Send again on an uncertain
// row reuses the attempt's idempotency key, so the server replays or admits
// it once.

const identity = "identity:queue-agent";
const agentLabel = "Queue agent";
const scope = "runtime/realm/principal";

function seed() {
  const target = { id: `chat:${identity}`, kind: "agent-chat", title: agentLabel, identity, memberId: identity };
  window.localStorage.setItem(`meta-console-dock-state:${scope}`, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", mode: "console", target }], activeTabId: "tab-1", focusedPanelId: "panel-1",
  }));
}
// `durable` mirrors the gateway's `send_dedupe.durable`; undefined omits the
// section, as an older gateway does.
function transport(send: MetaConsoleTransport["send"], brand?: string, durable?: boolean): MetaConsoleTransport {
  return {
    loadExperience: async () => ({ runtime_id: "queue-test", console_config: brand ? { brand: { label: brand } } : {},
      ...(durable === undefined ? {} : { send_dedupe: { durable } }), agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: agentLabel, kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } }, activity_feed: { filter_presets: [], active_preset_id: "all" } }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/console/inspect_identity"] }) as never,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {}, send,
    executeCommand: async (input) => ({ command: input.command, accepted: true, result: { identity: { identity } } }) as never,
    upload: async () => ({ blob_id: "blob" }) as never, blobUrl: (id) => `/blobs/${id}`,
  };
}
function savedRow(state: "outcome-unknown" | "definitely-rejected", extra: Partial<ConsoleSendAttempt> = {}): ConsoleSendAttempt {
  const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({ id: "row-1", scope, destination: identity,
    origin: "console:old-pane", idempotencyKey: "original-key", text: "Please summarise yesterday", now: 1 }),
  { owner: "old-tab", now: 2, handlingMode: "queue" });
  return { ...finishConsoleSendAttempt(attempted, { state, error: "Failed to fetch" }), ...extra };
}
const saved = () => JSON.parse(window.localStorage.getItem(consoleSendStorageKey(scope, identity)) ?? "{\"attempts\":[]}").attempts as ConsoleSendAttempt[];

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear(); window.sessionStorage.clear(); seed();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("pending row language", () => {
  it("words an uncertain row plainly, offers Check, Send again and Discard, and never shows protocol terms", async () => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("outcome-unknown", { failureKind: "connection_failed" })]);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(vi.fn(), "Example", true)} />);
    const row = await screen.findByTestId("pending-item:row-1");
    expect(within(row).getByText(`We couldn't confirm ${agentLabel} got this.`)).toBeVisible();
    expect(within(row).getByText("Couldn't reach Example (offline or signed out).")).toBeVisible();
    for (const name of ["Check", "Send again", "Discard"]) expect(within(row).getByRole("button", { name })).toBeEnabled();
    for (const term of [/acceptance/i, /Failed to fetch/, /receipt/i, /gateway/i]) expect(row.textContent).not.toMatch(term);
  });

  it("words a definite refusal as not sent, with Send again and Discard but no Check", async () => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("definitely-rejected", { failureKind: "unauthenticated" })]);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(vi.fn())} />);
    const row = await screen.findByTestId("pending-item:row-1");
    expect(within(row).getByText(`Not sent: this message never reached ${agentLabel}.`)).toBeVisible();
    expect(within(row).getByText("Sign in again or check this connection's authentication. Then review your saved message before choosing Send again.")).toBeVisible();
    expect(row.textContent).not.toMatch(/network isn.t allowed|permission to message/);
    expect(within(row).getByRole("button", { name: "Send again" })).toBeEnabled();
    expect(within(row).getByRole("button", { name: "Discard" })).toBeEnabled();
    expect(within(row).queryByRole("button", { name: "Check" })).toBeNull();
  });

  it("rewords a legacy untyped row saved by an older console without showing its stored error", async () => {
    // 0.8.43 and earlier saved the raw error string and no failure kind.
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("outcome-unknown")]);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(vi.fn())} />);
    const row = await screen.findByTestId("pending-item:row-1");
    expect(within(row).getByText(`We couldn't confirm ${agentLabel} got this.`)).toBeVisible();
    expect(row.textContent).not.toMatch(/Failed to fetch|Acceptance unknown/);
    expect(screen.queryByText(/Queue saved for this account and runtime/)).toBeNull();
  });
});

describe("Check always ends in a visible result", () => {
  it("shows Checking at once, then a miss keeps the row uncertain with its options", async () => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("outcome-unknown", { failureKind: "timeout" })]);
    const fake = transport(vi.fn());
    let finish!: (page: Awaited<ReturnType<MetaConsoleTransport["queryTimeline"]>>) => void;
    let checking = false;
    fake.executeCommand = vi.fn(async (input) => { checking = true; return { command: input.command, accepted: true, result: { identity: { identity } } } as never; });
    fake.queryTimeline = vi.fn(async () => checking ? new Promise(resolve => { finish = resolve; }) : { available: true, frames: [] });
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    const row = await screen.findByTestId("pending-item:row-1");
    fireEvent.click(within(row).getByRole("button", { name: "Check" }));
    expect(await within(row).findByText("Checking...")).toBeVisible();
    await waitFor(() => expect(finish).toBeTypeOf("function"));
    await act(async () => { finish({ available: true, frames: [] }); });
    expect(await within(row).findByText(`Not found in ${agentLabel}'s recent messages.`)).toBeVisible();
    for (const name of ["Check", "Discard"]) expect(within(row).getByRole("button", { name })).toBeEnabled();
    expect(saved()[0].state).toBe("outcome-unknown");
    expect(fake.send).not.toHaveBeenCalled();
  });

  it("clears the row and says when it was delivered when the check finds it", async () => {
    const row = savedRow("outcome-unknown", { failureKind: "connection_failed" });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [row]);
    const fake = transport(vi.fn());
    const deliveredAt = Date.UTC(2026, 9, 1, 14, 3);
    let checking = false;
    fake.executeCommand = vi.fn(async (input) => { checking = true; return { command: input.command, accepted: true, result: { identity: { identity } } } as never; });
    fake.queryTimeline = vi.fn(async () => checking ? { available: true, frames: [{ id: "receipt", event: "user_input", identity, cursor: "console:9",
      timestampMs: deliveredAt, interactionId: "turn-9", data: JSON.parse(row.envelopeJson!) }] } : { available: true, frames: [] });
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    fireEvent.click(within(await screen.findByTestId("pending-item:row-1")).getByRole("button", { name: "Check" }));
    await waitFor(() => expect(screen.queryByTestId("pending-item:row-1")).toBeNull());
    const notice = await screen.findByTestId("pending-delivered:row-1");
    expect(notice).toHaveTextContent(/^Delivered at /);
    expect(notice).toHaveTextContent(new Date(deliveredAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }));
    expect(saved()).toHaveLength(0);
  });

  it("names a failed check plainly instead of the raw fetch error", async () => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("outcome-unknown", { failureKind: "timeout" })]);
    const fake = transport(vi.fn(), "Example");
    fake.executeCommand = vi.fn(async () => { throw new TypeError("Failed to fetch"); });
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    const row = await screen.findByTestId("pending-item:row-1");
    fireEvent.click(within(row).getByRole("button", { name: "Check" }));
    expect(await within(row).findByTestId("pending-check:row-1")).toHaveTextContent("Couldn't check: couldn't reach Example (offline or signed out).");
    expect(row.textContent).not.toMatch(/Failed to fetch/);
    expect(saved()[0].state).toBe("outcome-unknown");
  });

  it("answers a check whose owner cannot be resolved instead of doing nothing", async () => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("outcome-unknown", { failureKind: "timeout" })]);
    const fake = transport(vi.fn());
    fake.executeCommand = vi.fn(async (input) => ({ command: input.command, accepted: true, result: {} }) as never);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={fake} />);
    const row = await screen.findByTestId("pending-item:row-1");
    fireEvent.click(within(row).getByRole("button", { name: "Check" }));
    expect(await within(row).findByTestId("pending-check:row-1")).toHaveTextContent(/^Couldn't check: /);
    expect(saved()[0].state).toBe("outcome-unknown");
  });
});

describe("Send again on an uncertain row", () => {
  it("resends the same saved message with its original idempotency key, once", async () => {
    const row = savedRow("outcome-unknown", { failureKind: "connection_failed" });
    saveConsoleSendAttempts(window.localStorage, scope, identity, [row]);
    const send = vi.fn(async () => ({ interaction_id: "replayed-turn", identity, input_frame_id: "existing-frame" }) as never);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(send, undefined, true)} />);
    fireEvent.click(within(await screen.findByTestId("pending-item:row-1")).getByRole("button", { name: "Send again" }));
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const envelope = JSON.parse(row.envelopeJson!);
    expect(send.mock.calls[0][0]).toMatchObject({ identity, idempotencyKey: envelope.idempotency_key,
      origin: envelope.origin, content: envelope.content, handlingMode: envelope.handling_mode });
    expect(envelope.idempotency_key).toBe("original-key");
    await waitFor(() => expect(screen.queryByTestId("pending-item:row-1")).toBeNull());
    expect(saved()).toHaveLength(0);
    expect(send).toHaveBeenCalledTimes(1);
  });
});

describe("Send again on an uncertain row follows the gateway's dedupe durability", () => {
  it.each([
    ["durable", true, true],
    ["not durable (in-memory store)", false, false],
    ["missing (older gateway)", undefined, false],
  ] as const)("send_dedupe %s", async (_label, durable, offered) => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("outcome-unknown", { failureKind: "connection_failed" })]);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(vi.fn(), "Example", durable)} />);
    const row = await screen.findByTestId("pending-item:row-1");
    // The wording stays honest either way: delivery is unconfirmed.
    expect(within(row).getByText(`We couldn't confirm ${agentLabel} got this.`)).toBeVisible();
    for (const name of ["Check", "Discard"]) expect(within(row).getByRole("button", { name })).toBeEnabled();
    if (offered) expect(within(row).getByRole("button", { name: "Send again" })).toBeEnabled();
    else expect(within(row).queryByRole("button", { name: "Send again" })).toBeNull();
  });

  it.each([[true], [false], [undefined]] as const)("a refused row keeps Send again when send_dedupe.durable is %s", async (durable) => {
    saveConsoleSendAttempts(window.localStorage, scope, identity, [savedRow("definitely-rejected", { failureKind: "unauthenticated" })]);
    render(<ConsoleApp baseUrl="" storageNamespace={scope} transport={transport(vi.fn(), undefined, durable)} />);
    const row = await screen.findByTestId("pending-item:row-1");
    expect(within(row).getByRole("button", { name: "Send again" })).toBeEnabled();
  });
});
