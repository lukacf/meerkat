import React from "react";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "./headless";

// A page that was opened before its gateway was upgraded, and an application
// that mounts an older console than its gateway serves, keep talking to the
// newer gateway with older code, and nothing said so. The gateway now names
// the console bundle it serves in the experience document, the page knows
// which bundle it runs, and a page that runs another bundle than the gateway
// names shows one notice that says to reload. The gateway's own page says
// it in words that hold after an upgrade and after a rollback; an
// application that mounts the console's library is told as well what to do
// when a reload does not help.

const namespace = "runtime/realm/stale-bundle";
const identity = "identity:stale-bundle-agent";
const RUNNING = "9a34aa516728";
const NEWER = "0f1e2d3c4b5a";
const PAGE_WORDS = "This page is of another Console build than its gateway's. Reload the page to use the gateway's version.";
const LIBRARY_WORDS = "This Console is of another build than its gateway's. Reload the page; "
  + "if this notice stays, the application needs the Console of its gateway's release.";

interface Gateway {
  transport: MetaConsoleTransport;
  /** What the gateway names from now on (undefined: an older gateway, which names nothing). */
  serve(bundle: string | undefined): void;
  /** A frame of the stream that makes the page read its experience again. */
  frame(): void;
  /** Whether the page holds its stream open. */
  subscribed(): boolean;
  reads(): number;
}

function gateway(bundle: string | undefined): Gateway {
  let served = bundle;
  let reads = 0;
  let deliver: ((frame: never) => void) | null = null;
  const transport: MetaConsoleTransport = {
    loadExperience: async () => {
      reads += 1;
      return {
        runtime_id: "stale-bundle-test",
        ...(served === undefined ? {} : { console_bundle: served }),
        agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Stale bundle agent", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true } }] } },
        activity_feed: { filter_presets: [], active_preset_id: "all" },
      } as never;
    },
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/timeline"] }) as never,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: (_input, onFrame) => {
      deliver = onFrame as (frame: never) => void;
      return () => { deliver = null; };
    },
    send: vi.fn(),
    executeCommand: async () => ({ result: {} }) as never,
    upload: async () => ({ blob_id: "blob" }) as never,
    blobUrl: (id) => `/blobs/${id}`,
  };
  let seq = 0;
  return {
    transport,
    serve(next) { served = next; },
    frame() {
      seq += 1;
      deliver?.({ id: `refresh-${seq}`, event: "topology_updated", timestampMs: seq + 1, data: {} } as never);
    },
    subscribed: () => deliver !== null,
    reads: () => reads,
  };
}

/** The page as the gateway serves it: its script names the bundle it loaded. */
function pageRuns(bundle: string) {
  const script = document.createElement("script");
  script.setAttribute("src", `/console/assets/console-app.js?v=${bundle}`);
  script.setAttribute("type", "text/plain");
  script.setAttribute("data-test-bundle", "");
  document.head.appendChild(script);
}

const notice = () => screen.queryByTestId("console-bundle-notice");
const settle = (ms = 150) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

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
  for (const script of Array.from(document.head.querySelectorAll("script[data-test-bundle]"))) script.remove();
});

describe("a console whose gateway names the bundle it serves", () => {
  it("shows no notice while the gateway names the bundle the page runs", async () => {
    pageRuns(RUNNING);
    const served = gateway(RUNNING);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    await screen.findByTestId("timeline-panel");
    await settle();
    expect(notice()).toBeNull();
  });

  it("shows one notice that says to reload when the gateway names another bundle", async () => {
    pageRuns(RUNNING);
    const served = gateway(NEWER);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    const shown = await screen.findByTestId("console-bundle-notice");
    expect(shown.querySelector("span")?.textContent).toBe(PAGE_WORDS);
    expect(shown.querySelector("button")?.textContent).toBe("Reload");
    expect(screen.getAllByTestId("console-bundle-notice")).toHaveLength(1);
    // The console goes on working under the notice.
    expect(await screen.findByTestId("timeline-panel")).toBeTruthy();
  });

  it("shows the notice in a page that was open when its gateway was replaced by a newer one", async () => {
    pageRuns(RUNNING);
    const served = gateway(RUNNING);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    await screen.findByTestId("timeline-panel");
    await waitFor(() => expect(served.subscribed()).toBe(true));
    await settle();
    expect(notice()).toBeNull();
    // The gateway is replaced: its next answer names the bundle it serves now.
    served.serve(NEWER);
    const before = served.reads();
    await act(async () => { served.frame(); });
    await waitFor(() => expect(served.reads()).toBeGreaterThan(before), { timeout: 5_000 });
    const shown = await screen.findByTestId("console-bundle-notice", undefined, { timeout: 5_000 });
    // The words hold whichever way the gateway's build moved.
    expect(shown.querySelector("span")?.textContent).toBe(PAGE_WORDS);
  }, 20_000);

  it("shows no notice on an older gateway, which names no bundle", async () => {
    pageRuns(RUNNING);
    const served = gateway(undefined);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    await screen.findByTestId("timeline-panel");
    await settle();
    expect(notice()).toBeNull();
  });

  it("shows no notice where the page cannot tell which bundle it runs", async () => {
    const served = gateway(NEWER);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    await screen.findByTestId("timeline-panel");
    await settle();
    expect(notice()).toBeNull();
  });
});

describe("an application that mounts the console itself", () => {
  // The console's own build names, inside the library an application
  // mounts, the bundle that library was built beside. There a reload helps
  // only when the application itself was updated, so the notice also says
  // what is needed when it stays.
  it("gets a notice in words of its own when its console is of another build than the gateway's", async () => {
    vi.stubGlobal("__META_CONSOLE_BUNDLE__", RUNNING);
    const served = gateway(NEWER);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    const shown = await screen.findByTestId("console-bundle-notice");
    expect(shown.querySelector("span")?.textContent).toBe(LIBRARY_WORDS);
    expect(shown.querySelector("button")?.textContent).toBe("Reload");
    expect(screen.getAllByTestId("console-bundle-notice")).toHaveLength(1);
  });

  it("is told in the library's words also on a page whose script names a bundle", async () => {
    // The library's build decides: an application's own page may load a
    // script of that name too.
    pageRuns(NEWER);
    vi.stubGlobal("__META_CONSOLE_BUNDLE__", RUNNING);
    const served = gateway(NEWER);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    const shown = await screen.findByTestId("console-bundle-notice");
    expect(shown.querySelector("span")?.textContent).toBe(LIBRARY_WORDS);
  });

  it("gets none when its console is of the gateway's build", async () => {
    vi.stubGlobal("__META_CONSOLE_BUNDLE__", RUNNING);
    const served = gateway(RUNNING);
    render(<ConsoleApp baseUrl="" storageNamespace={namespace} transport={served.transport} />);
    await screen.findByTestId("timeline-panel");
    await settle();
    expect(notice()).toBeNull();
  });
});
