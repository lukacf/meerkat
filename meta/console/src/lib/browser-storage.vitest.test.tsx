import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleWhoami, MetaConsoleTransport } from "./headless";
import { createConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, saveConsoleSendAttempts } from "./send-attempt-storage";

// Browser storage on a shared profile: what one person leaves behind in the
// browser is theirs alone, and it goes when they sign out.

const identity = "identity:storage-agent";
const CALLER_KEY = "meta-console-caller:v1:";
type Caller = "a" | "b";

function experienceFor(caller: Caller, consoleConfig: Record<string, unknown> = {}) {
  return {
    runtime_id: "storage-test",
    storage_scope: `console-storage:v1:${caller}`,
    console_config: { layout: { initial_agent: identity }, ...consoleConfig },
    agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Storage agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
  };
}
const whoamiFor = (caller: Caller): ConsoleWhoami => ({
  actorKind: "person", principal: `${caller}@example.test`, approverIds: [], canAdminister: false, callerScope: `caller-${caller}`,
});
/** The default embedded scope the console keeps a caller's queue under. */
const namespaceOf = (caller: Caller) => JSON.stringify(["", `console-storage:v1:${caller}`]);
const busyHistory = { available: true, frames: [{ id: "busy", event: "interaction_started", identity, interactionId: "busy", timestampMs: 1, data: { content: "working" } }] };

function transport(state: { caller: Caller }, consoleConfig: Record<string, unknown> = {}): MetaConsoleTransport {
  return {
    loadExperience: vi.fn(async () => experienceFor(state.caller, consoleConfig)) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline"] }) as never,
    whoami: vi.fn(async () => whoamiFor(state.caller)),
    queryTimeline: async () => busyHistory as never,
    subscribeTimeline: () => () => {},
    send: vi.fn(async (input) => ({ interaction_id: "sent", identity: input.identity })),
    executeCommand: async () => ({ result: {} }) as never,
  };
}

async function compose(text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
  return textarea;
}
const settle = (ms = 200) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });
const storageKeys = (storage: Storage) => Array.from({ length: storage.length }, (_, index) => storage.key(index)!);

beforeEach(() => {
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
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("storage hygiene", () => {
  it("legacy_queue_offered_once", async () => {
    const legacyKey = `mobkit-pending-stack:${identity}`;
    window.localStorage.setItem(legacyKey, JSON.stringify([{ id: "pre-upgrade", text: "Queued before the upgrade", addedAt: 1 }]));
    const state: { caller: Caller } = { caller: "a" };
    const view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    const offer = await screen.findByRole("group", { name: "Older queued messages" });
    fireEvent.click(within(offer).getByRole("button", { name: "Import and send older queued messages" }));
    expect(within(await screen.findByTestId("pending-stack")).getByText("Queued before the upgrade")).toBeVisible();
    // Imported once: the unscoped bytes go, so no other account is offered them.
    await waitFor(() => expect(window.localStorage.getItem(legacyKey)).toBeNull());
    view.unmount();
    state.caller = "b";
    render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle();
    expect(screen.queryByRole("group", { name: "Older queued messages" })).toBeNull();
    expect(screen.queryByText("Queued before the upgrade")).toBeNull();
  });

  it("sign_out_purges_the_callers_browser_data", async () => {
    const other = createConsoleSendAttempt({ id: "queued-b", scope: namespaceOf("b"), destination: identity, origin: "console:pane", idempotencyKey: "key-b", text: "Queued by b", now: 1 });
    saveConsoleSendAttempts(window.localStorage, namespaceOf("b"), identity, [other]);
    const state: { caller: Caller } = { caller: "a" };
    render(<ConsoleApp baseUrl="" transport={transport(state, { sign_out: { href: "/logout", method: "post" } })} />);
    await compose("Queued by a");
    expect(within(await screen.findByTestId("pending-stack")).getByText("Queued by a")).toBeVisible();
    fireEvent.change(screen.getByTestId(`chat-composer:${identity}`), { target: { value: "Unsent draft of a" } });
    await settle(400);
    const encoded = encodeURIComponent(namespaceOf("a"));
    expect(storageKeys(window.sessionStorage).some((key) => key.startsWith("meta-composer-draft:") && key.includes(encoded))).toBe(true);
    const chip = await screen.findByTestId("identity-chip");
    fireEvent.click(within(chip).getByRole("button", { name: /a@example\.test/ }));
    const signOut = await screen.findByRole("button", { name: "Sign out" });
    const leave = vi.spyOn(HTMLFormElement.prototype, "submit").mockImplementation(() => {});
    fireEvent.submit(signOut.closest("form")!);
    // a's queue and drafts leave this browser; b's stay untouched.
    expect(window.localStorage.getItem(consoleSendStorageKey(namespaceOf("a"), identity))).toBeNull();
    expect(storageKeys(window.sessionStorage).filter((key) => key.includes(encoded))).toEqual([]);
    expect(JSON.parse(window.localStorage.getItem(consoleSendStorageKey(namespaceOf("b"), identity))!).attempts[0].text).toBe("Queued by b");
    expect(JSON.parse(window.localStorage.getItem(CALLER_KEY)!).caller).toBeNull();
    // Then the browser leaves for the host's sign-out.
    await waitFor(() => expect(leave).toHaveBeenCalledTimes(1));
  });

  it("sign-out waits for a stalled IndexedDB purge for at most 1.5 seconds", async () => {
    const state: { caller: Caller } = { caller: "a" };
    render(<ConsoleApp baseUrl="" transport={transport(state, { sign_out: { href: "/logout", method: "post" } })} />);
    const chip = await screen.findByTestId("identity-chip");
    await settle();
    // An IndexedDB whose open request never answers: the purge stalls.
    vi.stubGlobal("indexedDB", { open: () => ({}) });
    fireEvent.click(within(chip).getByRole("button", { name: /a@example\.test/ }));
    const signOut = await screen.findByRole("button", { name: "Sign out" });
    const leave = vi.spyOn(HTMLFormElement.prototype, "submit").mockImplementation(() => {});
    fireEvent.submit(signOut.closest("form")!);
    await settle(300);
    // Not at once: the browser waits for the purge.
    expect(leave).not.toHaveBeenCalled();
    await settle(1_500);
    // And not for good: it leaves once the bound passed.
    expect(leave).toHaveBeenCalledTimes(1);
  });

  it("files staged when a's login ended never reach b's console", async () => {
    vi.stubGlobal("URL", class extends URL {
      static createObjectURL = vi.fn(() => `blob:preview-${Math.random()}`);
      static revokeObjectURL = vi.fn();
    });
    const state: { caller: Caller } = { caller: "a" };
    const fake = transport(state);
    fake.loadExperience = vi.fn(async () => {
      const experience = experienceFor(state.caller);
      experience.agent_sidebar.live_snapshot.agents[0].model_capabilities.image_input = true;
      return experience;
    }) as never;
    fake.send = vi.fn(async () => {
      throw Object.assign(new Error("meta/console/send request failed 401"), {
        httpStatus: 401, responseRpcError: { code: -32600, message: "unauthorized", data: { kind: "unauthenticated" } },
      });
    });
    fake.upload = async () => ({ blob_id: "blob" }) as never;
    fake.blobUrl = (id) => `/blobs/${id}`;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle(300);
    const file = new File([new Uint8Array([1, 2, 3])], "a-private.png", { type: "image/png" });
    fireEvent.drop(document.querySelector(".composer__shell")!, { dataTransfer: { files: [file], items: [], types: ["Files"], getData: () => "" } });
    await waitFor(() => expect(screen.getAllByRole("button", { name: "Remove attachment" })).toHaveLength(1));
    await compose("a's message with a's file");
    expect(await screen.findByTestId("console-sign-in")).toBeVisible();
    // b signs in to this browser in another tab.
    state.caller = "b";
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: CALLER_KEY, newValue: JSON.stringify({ caller: "caller-b", at: Date.now() }) }));
    });
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(screen.getByTestId("identity-chip")).toHaveTextContent("b@example.test"));
    await settle(300);
    expect(screen.queryAllByRole("button", { name: "Remove attachment" })).toHaveLength(0);
  });

  it("a tab told of a sign-out removes its own drafts", async () => {
    const state: { caller: Caller } = { caller: "a" };
    render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    const composer = await screen.findByTestId(`chat-composer:${identity}`);
    fireEvent.change(composer, { target: { value: "Unsent draft in another tab" } });
    await settle(400);
    const encoded = encodeURIComponent(namespaceOf("a"));
    expect(storageKeys(window.sessionStorage).some((key) => key.includes(encoded))).toBe(true);
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: CALLER_KEY, newValue: JSON.stringify({ caller: null, at: Date.now() }) }));
    });
    expect(await screen.findByTestId("console-sign-in")).toBeVisible();
    await settle();
    expect(storageKeys(window.sessionStorage).filter((key) => key.includes(encoded))).toEqual([]);
  });

  it("a draft whose save was still waiting is not written after a sign-out", async () => {
    const state: { caller: Caller } = { caller: "a" };
    render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    const composer = await screen.findByTestId(`chat-composer:${identity}`);
    await settle(300);
    // Typed, and signed out in another tab before the debounced save ran:
    // the closing pane flushes the draft on its way out.
    fireEvent.change(composer, { target: { value: "Typed right before signing out" } });
    act(() => {
      window.dispatchEvent(new StorageEvent("storage", { key: CALLER_KEY, newValue: JSON.stringify({ caller: null, at: Date.now() }) }));
    });
    expect(await screen.findByTestId("console-sign-in")).toBeVisible();
    await settle(600);
    const encoded = encodeURIComponent(namespaceOf("a"));
    expect(storageKeys(window.sessionStorage).filter((key) => key.includes(encoded))).toEqual([]);
  });

  it("preferences_are_per_user", async () => {
    const state: { caller: Caller } = { caller: "a" };
    let view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    fireEvent.click(screen.getByTestId("sidebar-collapse-toggle"));
    fireEvent.click(screen.getByTestId("theme-toggle"));
    expect(document.querySelector(".shell")!.getAttribute("data-sidebar-collapsed")).toBe("true");
    expect(screen.getByTestId("meerkat-console").getAttribute("data-cc-theme")).toBe("dark");
    view.unmount();
    // b, on the same browser, starts from the defaults.
    state.caller = "b";
    view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    expect(document.querySelector(".shell")!.getAttribute("data-sidebar-collapsed")).toBe("false");
    expect(screen.getByTestId("meerkat-console").getAttribute("data-cc-theme")).toBe("light");
    view.unmount();
    // a finds a's own choices again.
    state.caller = "a";
    render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    expect(document.querySelector(".shell")!.getAttribute("data-sidebar-collapsed")).toBe("true");
    expect(screen.getByTestId("meerkat-console").getAttribute("data-cc-theme")).toBe("dark");
  });

  it("roster_sections_are_per_user", async () => {
    const state: { caller: Caller } = { caller: "a" };
    let view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    const section = await waitFor(() => {
      const toggle = document.querySelector<HTMLElement>('[data-testid^="sidebar-section-toggle:"]');
      expect(toggle).not.toBeNull();
      return toggle!.getAttribute("data-testid")!;
    });
    expect(screen.getByTestId(section).getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByTestId(section));
    expect(screen.getByTestId(section).getAttribute("aria-expanded")).toBe("false");
    view.unmount();
    // b, on the same browser, finds the roster as arranged by default.
    state.caller = "b";
    view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    expect((await screen.findByTestId(section)).getAttribute("aria-expanded")).toBe("true");
    view.unmount();
    state.caller = "a";
    render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(screen.getByTestId(section).getAttribute("aria-expanded")).toBe("false"));
  });

  it("a preference an earlier console kept for every user is read once", async () => {
    // An earlier console kept the theme for everyone using this browser.
    window.localStorage.setItem("meta-console-theme", "dark");
    const state: { caller: Caller } = { caller: "a" };
    let view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    expect(screen.getByTestId("meerkat-console").getAttribute("data-cc-theme")).toBe("dark");
    // The first user to load adopts it; the shared copy goes.
    expect(window.localStorage.getItem("meta-console-theme")).toBeNull();
    view.unmount();
    state.caller = "b";
    view = render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(screen.getByTestId("meerkat-console").getAttribute("data-cc-theme")).toBe("light"));
    view.unmount();
    state.caller = "a";
    render(<ConsoleApp baseUrl="" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(screen.getByTestId("meerkat-console").getAttribute("data-cc-theme")).toBe("dark"));
  });

  it("host_namespace_is_combined_with_storage_scope", async () => {
    const state: { caller: Caller } = { caller: "a" };
    let view = render(<ConsoleApp baseUrl="" storageNamespace="host-namespace" transport={transport(state)} />);
    await compose("Queued by a under the host's namespace");
    expect(within(await screen.findByTestId("pending-stack")).getByText("Queued by a under the host's namespace")).toBeVisible();
    view.unmount();
    state.caller = "b";
    view = render(<ConsoleApp baseUrl="" storageNamespace="host-namespace" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle();
    expect(screen.queryByText("Queued by a under the host's namespace")).toBeNull();
  });

  it("a queue kept under the host namespace alone is offered once", async () => {
    const earlier = createConsoleSendAttempt({ id: "host-only", scope: "host-namespace", destination: identity, origin: "console:pane", idempotencyKey: "host-key", text: "Queued before the scope was combined", now: 1 });
    saveConsoleSendAttempts(window.localStorage, "host-namespace", identity, [earlier]);
    const state: { caller: Caller } = { caller: "a" };
    const view = render(<ConsoleApp baseUrl="" storageNamespace="host-namespace" transport={transport(state)} />);
    const offer = await screen.findByRole("group", { name: "Older queued messages" });
    fireEvent.click(within(offer).getByRole("button", { name: "Import and send older queued messages" }));
    expect(within(await screen.findByTestId("pending-stack")).getByText("Queued before the scope was combined")).toBeVisible();
    await waitFor(() => expect(window.localStorage.getItem(consoleSendStorageKey("host-namespace", identity))).toBeNull());
    view.unmount();
    state.caller = "b";
    render(<ConsoleApp baseUrl="" storageNamespace="host-namespace" transport={transport(state)} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await settle();
    expect(screen.queryByRole("group", { name: "Older queued messages" })).toBeNull();
  });
});

describe("preferences across tabs", () => {
  it("pins_merge_across_tabs", async () => {
    const second = "identity:second-agent";
    const fake = transport({ caller: "a" });
    fake.loadExperience = vi.fn(async () => {
      const experience = experienceFor("a");
      experience.agent_sidebar.live_snapshot.agents.push({ ...experience.agent_sidebar.live_snapshot.agents[0], identity: second, member_id: second, agent_id: second, label: "Second agent" });
      return experience;
    }) as never;
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`sidebar-agent:${identity}`);
    const pinsKey = () => storageKeys(window.localStorage).find((key) => key.startsWith("meta-console-sidebar-agent-pins:"));
    // Another tab of the same person pins the second agent.
    const key = `meta-console-sidebar-agent-pins:${encodeURIComponent(namespaceOf("a"))}`;
    window.localStorage.setItem(key, JSON.stringify([second]));
    act(() => { window.dispatchEvent(new StorageEvent("storage", { key, newValue: JSON.stringify([second]) })); });
    // This tab pins the first agent: both pins are kept.
    fireEvent.click(screen.getAllByRole("button", { name: /^Pin Storage agent/ })[0]);
    await waitFor(() => expect(JSON.parse(window.localStorage.getItem(pinsKey()!)!).sort()).toEqual([identity, second].sort()));
  });

  const twoAgents = () => {
    const second = "identity:second-agent";
    const fake = transport({ caller: "a" });
    fake.loadExperience = vi.fn(async () => {
      const experience = experienceFor("a");
      experience.agent_sidebar.live_snapshot.agents.push({ ...experience.agent_sidebar.live_snapshot.agents[0], identity: second, member_id: second, agent_id: second, label: "Second agent" });
      return experience;
    }) as never;
    const key = `meta-console-sidebar-agent-pins:${encodeURIComponent(namespaceOf("a"))}`;
    return { second, fake, key };
  };

  it("a pin click keeps a pin another tab stored that this tab has not heard of", async () => {
    const { second, fake, key } = twoAgents();
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`sidebar-agent:${identity}`);
    await settle();
    // Another tab of the same person pins the second agent; no storage event
    // has reached this tab yet.
    window.localStorage.setItem(key, JSON.stringify([second]));
    fireEvent.click(screen.getAllByRole("button", { name: /^Pin Storage agent/ })[0]);
    await waitFor(() => expect(JSON.parse(window.localStorage.getItem(key)!).sort()).toEqual([identity, second].sort()));
  });

  it("another tab's pins show here without a click", async () => {
    const { second, fake, key } = twoAgents();
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`sidebar-agent:${identity}`);
    await settle();
    expect(screen.queryAllByRole("button", { name: /^Unpin Second agent/ })).toHaveLength(0);
    window.localStorage.setItem(key, JSON.stringify([second]));
    act(() => { window.dispatchEvent(new StorageEvent("storage", { key, newValue: JSON.stringify([second]) })); });
    await waitFor(() => expect(screen.queryAllByRole("button", { name: /^Unpin Second agent/ }).length).toBeGreaterThan(0));
  });

  it("a pin click pins under React StrictMode", async () => {
    const { fake, key } = twoAgents();
    render(<React.StrictMode><ConsoleApp baseUrl="" transport={fake} /></React.StrictMode>);
    await screen.findByTestId(`sidebar-agent:${identity}`);
    await settle();
    fireEvent.click(screen.getAllByRole("button", { name: /^Pin Storage agent/ })[0]);
    await settle(100);
    expect(JSON.parse(window.localStorage.getItem(key) ?? "[]")).toEqual([identity]);
    expect(screen.queryAllByRole("button", { name: /^Unpin Storage agent/ }).length).toBeGreaterThan(0);
  });

  it("an_idle_tab_does_not_rewrite_the_dock_layout", async () => {
    const fake = transport({ caller: "a" });
    let receive: ((frame: never) => void) | undefined;
    fake.subscribeTimeline = (_input, next) => { receive = next as never; return () => {}; };
    render(<ConsoleApp baseUrl="" transport={fake} />);
    await screen.findByTestId(`chat-composer:${identity}`);
    await waitFor(() => expect(receive).toBeTypeOf("function"));
    await settle();
    const key = storageKeys(window.localStorage).find((candidate) => candidate.startsWith("meta-console-dock-state:"))!;
    expect(key).toBeTruthy();
    // Another tab of the same person arranges the dock and saves it.
    const arranged = JSON.stringify({ arranged: "in another tab" });
    window.localStorage.setItem(key, arranged);
    // This tab only reloads its experience; its layout did not change.
    const reads = vi.mocked(fake.loadExperience).mock.calls.length;
    await act(async () => { receive?.({ id: "refresh", event: "interaction_started", identity, interactionId: "refresh", timestampMs: 2, data: {} } as never); });
    await waitFor(() => expect(vi.mocked(fake.loadExperience).mock.calls.length).toBeGreaterThan(reads));
    await settle();
    expect(window.localStorage.getItem(key)).toBe(arranged);
  });
});
