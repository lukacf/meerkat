/**
 * Structural typing-lag benchmark for the Meta console.
 *
 * Mounts the real ConsoleApp under jsdom with a fake transport: 160 agents in
 * the sidebar, one docked chat panel whose identity carries a transcript of
 * `N` markdown messages. Then it types 20 keystrokes into the composer and
 * pushes 20 SSE frames, recording wall time and component render counts.
 *
 * The assertions are ceilings that pin the structural fixes (composer draft
 * isolated from the app root, memoised siblings, memoised transcript
 * derivation, coalesced event frames). The timings are printed for the PR
 * body; only the render-count ceilings and a generous time bound are asserted
 * so the test stays deterministic on slow CI runners.
 *
 * This is not a latency measurement. jsdom does no style recalculation,
 * layout or paint, and these frames are not the gateway's, so it passed
 * while real typing lagged by a full transcript layout per keystroke and a
 * full transcript re-derivation per streamed token. Keystroke-to-next-paint
 * latency is gated by `npm run perf:typing:browser` (typing-lag-browser.cjs),
 * which drives the production build in Chromium.
 */
import React from "react";
import { act, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import type { MetaConsoleTransport } from "../lib/headless";
import {
  installRenderCounts,
  resetRenderCounts,
  uninstallRenderCounts,
  type RenderCounts,
} from "../lib/render-counts";
import { TRANSCRIPT_WINDOW_TURNS } from "../panels/ChatPane";
import type { ConsoleFrame } from "../types";

const AGENT_COUNT = 160;
const CHAT_IDENTITY = "identity:agent-000";
const KEYSTROKES = 20;
const SSE_FRAMES = 20;

const MARKDOWN = [
  "Here is a **status** update with `inline code` and a [link](https://example.invalid).",
  "- first point\n- second point with *emphasis*\n- third point",
  "```rust\nfn main() {\n    println!(\"hello\");\n}\n```",
  "A longer paragraph that keeps going for a while so the row has some height and the markdown parser has some work to do before the next message arrives in the transcript.",
];

function agentRow(index: number) {
  const id = `identity:agent-${String(index).padStart(3, "0")}`;
  return {
    identity: id,
    member_id: id,
    agent_id: id,
    label: `Agent ${index}`,
    kind: "member",
    role: index % 4 === 0 ? "lead" : "worker",
    state: "running",
    addressable: true,
    affordances: { can_respawn: true, can_retire: true },
    model_capabilities: { image_input: false },
  };
}

function experience() {
  return {
    contract_version: "bench",
    runtime_id: "bench-runtime",
    storage_scope: "bench-principal",
    console_config: {},
    console_policy: {},
    agent_sidebar: {
      live_snapshot: {
        agents: Array.from({ length: AGENT_COUNT }, (_, i) => agentRow(i)),
      },
    },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
  };
}

function transcript(identity: string, count: number): ConsoleFrame[] {
  const frames: ConsoleFrame[] = [];
  const base = 1_700_000_000_000;
  for (let i = 0; i < count; i += 1) {
    const iid = `turn-${i}`;
    const ts = base + i * 10_000;
    frames.push({
      id: `${identity}:${i}:start`,
      event: "interaction_started",
      identity,
      interactionId: iid,
      timestampMs: ts,
      cursor: `console:${i * 2 + 1}`,
      data: { content: `Question number ${i}, please summarise.` },
    });
    frames.push({
      id: `${identity}:${i}:done`,
      event: "interaction_complete",
      identity,
      interactionId: iid,
      timestampMs: ts + 5_000,
      cursor: `console:${i * 2 + 2}`,
      data: { text: MARKDOWN[i % MARKDOWN.length] },
    });
  }
  return frames;
}

interface FakeTransport extends MetaConsoleTransport {
  live?: (frame: ConsoleFrame) => void;
  /// Number of timeline HTTP queries issued so far (idle-rate assertion).
  timelineQueries: number;
}

function fakeTransport(frames: ConsoleFrame[]): FakeTransport {
  const fake: FakeTransport = {
    timelineQueries: 0,
    loadExperience: async () => experience() as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "bench", methods: [] }) as never,
    queryTimeline: async (input) => {
      fake.timelineQueries += 1;
      const own = input.identity === CHAT_IDENTITY ? frames : [];
      return { frames: own, available: true } as never;
    },
    subscribeTimeline: (_input, onFrame) => {
      fake.live = onFrame;
      return () => {
        fake.live = undefined;
      };
    },
    send: async (input) =>
      ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) =>
      ({ command: input.command, accepted: true, result: {} }) as never,
    upload: async () => ({ blob_id: "blob" }) as never,
    blobUrl: (blobId) => `/blobs/${blobId}`,
  };
  return fake;
}

function seedDockedChat(panelCount = 1): void {
  // Mirrors ConsoleApp's dock persistence key and ConsoleDockState shape so the
  // app restores focused chat panel(s) at mount: panel-1 for CHAT_IDENTITY,
  // further panels for the next agents in the roster, laid out as a grid.
  const key = `meta-console-dock-state:${JSON.stringify(["", "bench-principal"])}`;
  const panels = Array.from({ length: panelCount }, (_, i) => {
    const identity = i === 0 ? CHAT_IDENTITY : agentRow(i).identity;
    return {
      id: `panel-${i + 1}`,
      mode: "console",
      target: {
        id: `chat:${identity}`,
        kind: "agent-chat",
        title: `Agent ${i}`,
        identity,
        memberId: identity,
      },
    };
  });
  type Node = { kind: "panel"; panelId: string } | { kind: "split"; id: string; direction: "horizontal" | "vertical"; first: Node; second: Node };
  const leaf = (i: number): Node => ({ kind: "panel", panelId: panels[i].id });
  const layout: Node =
    panelCount === 1
      ? leaf(0)
      : {
          kind: "split",
          id: "root",
          direction: "horizontal",
          first: panelCount > 2 ? { kind: "split", id: "left", direction: "vertical", first: leaf(0), second: leaf(2) } : leaf(0),
          second: panelCount > 3 ? { kind: "split", id: "right", direction: "vertical", first: leaf(1), second: leaf(3) } : leaf(1),
        };
  const state = {
    tabs: [{ id: "tab-1", presetId: panelCount === 1 ? "single" : "grid", layout }],
    panels,
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  };
  window.localStorage.setItem(key, JSON.stringify(state));
}

async function flush(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

async function settle(predicate: () => boolean, label: string, ms = 5_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await flush();
  }
}

interface Measurement {
  keystrokeMs: number[];
  keystrokeRenders: RenderCounts;
  frameMs: number[];
  /// Wall time for the whole SSE burst including the coalesced render flush.
  burstMs: number;
  frameRenders: RenderCounts;
  /// Turn elements mounted in the transcript after initial load.
  mountedTurns: number;
  /// Highest turn index rendered (turn count minus one after any log trim).
  lastTurnIndex: number;
  /// Timeline queries issued during a 2.5 s idle window after load, with
  /// the document visible and then hidden.
  idleTimelineQueries: number;
  hiddenIdleTimelineQueries: number;
}

function snapshotCounts(counts: RenderCounts): RenderCounts {
  return { ...counts };
}

async function measure(
  transcriptLength: number,
  options: { idleMs?: number; panels?: number } = {},
): Promise<Measurement> {
  const frames = transcript(CHAT_IDENTITY, transcriptLength);
  const transport = fakeTransport(frames);
  seedDockedChat(options.panels ?? 1);
  const counts = installRenderCounts();
  const view = render(<ConsoleApp baseUrl="" transport={transport} />);
  await settle(
    () => view.container.querySelectorAll(".conv-turn").length >= Math.min(transcriptLength, 1),
    "transcript render",
  );
  const textarea = view.container.querySelector("textarea");
  if (!textarea) throw new Error("composer textarea not rendered");
  await flush();
  if (options.panels && options.panels > 1) {
    await settle(
      () => view.container.querySelectorAll("textarea").length >= (options.panels ?? 1),
      "all docked panels render",
    );
  }
  const turnNodes = view.container.querySelectorAll<HTMLElement>("[data-chat-turn-index]");
  const mountedTurns = turnNodes.length;
  const lastTurnIndex = Array.from(turnNodes).reduce(
    (max, node) => Math.max(max, Number(node.dataset.chatTurnIndex)),
    -1,
  );
  let idleTimelineQueries = 0;
  let hiddenIdleTimelineQueries = 0;
  if (options.idleMs) {
    const before = transport.timelineQueries;
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, options.idleMs));
    });
    idleTimelineQueries = transport.timelineQueries - before;
    // A hidden tab must not issue docked-identity refreshes at all.
    const visibility = Object.getOwnPropertyDescriptor(Document.prototype, "visibilityState");
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    document.dispatchEvent(new Event("visibilitychange"));
    const hiddenBefore = transport.timelineQueries;
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, options.idleMs));
    });
    hiddenIdleTimelineQueries = transport.timelineQueries - hiddenBefore;
    delete (document as unknown as Record<string, unknown>).visibilityState;
    if (visibility) Object.defineProperty(Document.prototype, "visibilityState", visibility);
  }

  resetRenderCounts();
  const keystrokeMs: number[] = [];
  let draft = "";
  for (let i = 0; i < KEYSTROKES; i += 1) {
    draft += String.fromCharCode(97 + (i % 26));
    const started = performance.now();
    await act(async () => {
      fireEvent.change(textarea, { target: { value: draft } });
    });
    keystrokeMs.push(performance.now() - started);
  }
  const keystrokeRenders = snapshotCounts(counts);

  const scheduledFrames = new Map<number, FrameRequestCallback>();
  const requestFrame = window.requestAnimationFrame;
  const cancelFrame = window.cancelAnimationFrame;
  let nextFrameId = -1;
  // Keep independent React commits per event, but hold the frame boundary so
  // CPU contention cannot split this structural coalescing check across frames.
  window.requestAnimationFrame = (callback) => {
    const id = nextFrameId--;
    scheduledFrames.set(id, callback);
    return id;
  };
  window.cancelAnimationFrame = (id) => {
    if (!scheduledFrames.delete(id)) cancelFrame.call(window, id);
  };
  resetRenderCounts();
  const frameMs: number[] = [];
  const base = 1_800_000_000_000;
  const burstStarted = performance.now();
  let burstMs: number;
  let frameRenders: RenderCounts;
  try {
    for (let i = 0; i < SSE_FRAMES; i += 1) {
      const frame: ConsoleFrame = {
        id: `live:${i}`,
        event: "text_delta",
        identity: CHAT_IDENTITY,
        interactionId: "live-turn",
        timestampMs: base + i,
        cursor: `console:${100_000 + i}`,
        data: `token ${i} `,
      };
      const started = performance.now();
      await act(async () => {
        transport.live?.(frame);
      });
      frameMs.push(performance.now() - started);
    }
    // Release one animation frame only after all independent event commits.
    await act(async () => {
      const callbacks = [...scheduledFrames.values()];
      scheduledFrames.clear();
      callbacks.forEach((callback) => callback(performance.now()));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    burstMs = performance.now() - burstStarted;
    frameRenders = snapshotCounts(counts);
  } finally {
    view.unmount();
    scheduledFrames.clear();
    window.requestAnimationFrame = requestFrame;
    window.cancelAnimationFrame = cancelFrame;
  }
  return {
    keystrokeMs,
    keystrokeRenders,
    frameMs,
    burstMs,
    frameRenders,
    mountedTurns,
    lastTurnIndex,
    idleTimelineQueries,
    hiddenIdleTimelineQueries,
  };
}

function stats(samples: number[]): string {
  const sorted = [...samples].sort((a, b) => a - b);
  const mean = samples.reduce((a, b) => a + b, 0) / samples.length;
  const p95 = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95))];
  return `mean ${mean.toFixed(2)} ms, p95 ${p95.toFixed(2)} ms`;
}

describe("console typing lag benchmark", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });
  afterEach(() => {
    uninstallRenderCounts();
    window.localStorage.clear();
  });

  it("repairs a docked identity from its cursor when the stream reports a replay gap", async () => {
    const frames = transcript(CHAT_IDENTITY, 5);
    const transport = fakeTransport(frames);
    const identityQueries: Array<{ mode?: string; after?: string }> = [];
    const baseQuery = transport.queryTimeline;
    transport.queryTimeline = async (input) => {
      if (input.identity === CHAT_IDENTITY) identityQueries.push({ mode: input.mode, after: input.after });
      return baseQuery(input);
    };
    seedDockedChat(1);
    installRenderCounts();
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    await settle(() => view.container.querySelectorAll(".conv-turn").length >= 5, "transcript render");
    const before = identityQueries.length;
    // The runtime emits a synthetic replay_unavailable frame when its
    // source log resets; the console must re-query the docked identity.
    await act(async () => {
      transport.live?.({ id: "gap-1", event: "replay_unavailable", data: { reason: "source gap" } });
    });
    await settle(() => identityQueries.length > before, "replay gap repair query");
    const repair = identityQueries[identityQueries.length - 1];
    expect(["since", "recent"]).toContain(repair.mode);
    view.unmount();
  }, 30_000);

  it("renders streamed text on every third animation frame, other frames on the next, completion at once", async () => {
    const frames = transcript(CHAT_IDENTITY, 5);
    const transport = fakeTransport(frames);
    seedDockedChat(1);
    const counts = installRenderCounts();
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    await settle(() => view.container.querySelectorAll(".conv-turn").length >= 5, "transcript render");
    await flush();
    const scheduled = new Map<number, FrameRequestCallback>();
    const raf = window.requestAnimationFrame;
    const caf = window.cancelAnimationFrame;
    let nextId = -1;
    window.requestAnimationFrame = (callback) => { const id = nextId--; scheduled.set(id, callback); return id; };
    window.cancelAnimationFrame = (id) => { scheduled.delete(id); };
    const animationFrame = () => act(async () => {
      const callbacks = [...scheduled.values()];
      scheduled.clear();
      callbacks.forEach((callback) => callback(performance.now()));
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
    let seq = 0;
    const live = (event: string, data?: unknown) => act(async () => {
      seq += 1;
      data ??= `token ${seq} `;
      transport.live?.({ id: `cadence:${seq}`, event, identity: CHAT_IDENTITY, interactionId: "cadence-turn", timestampMs: 1_950_000_000_000 + seq, cursor: `console:${950_000 + seq}`, data });
    });
    const flushes = () => counts["LiveRenderFlush"] ?? 0;
    try {
      // Let mount-time refreshes settle: no render for a quiet window.
      for (let attempt = 0; attempt < 30; attempt += 1) {
        resetRenderCounts();
        for (let frame = 0; frame < 3; frame += 1) await animationFrame();
        await act(async () => { await new Promise((resolve) => setTimeout(resolve, 100)); });
        if (flushes() === 0 && scheduled.size === 0) break;
      }
      resetRenderCounts();
      await live("text_delta");
      await live("text_delta");
      await animationFrame();
      await animationFrame();
      expect(flushes()).toBe(0);
      await animationFrame();
      expect(flushes()).toBe(1);
      expect(view.container.textContent).toContain(`token ${seq}`);
      // Any other frame ends the text cadence: it renders on the next frame.
      await live("text_delta");
      await animationFrame();
      await live("tool_call_requested", { id: "call-1", name: "read_file", args: {} });
      expect(flushes()).toBe(1);
      await animationFrame();
      expect(flushes()).toBe(2);
      // A stream's completion renders without waiting for a frame.
      await live("text_delta");
      await live("interaction_complete", { result: "done" });
      expect(flushes()).toBe(3);
      // It also replaced the paced text render that was waiting.
      for (let frame = 0; frame < 3; frame += 1) await animationFrame();
      expect(flushes()).toBe(3);
    } finally {
      window.requestAnimationFrame = raf;
      window.cancelAnimationFrame = caf;
      view.unmount();
    }
  }, 30_000);

  it("flushes coalesced frames on a bounded timer while the tab is hidden", async () => {
    const frames = transcript(CHAT_IDENTITY, 5);
    const transport = fakeTransport(frames);
    seedDockedChat(1);
    const counts = installRenderCounts();
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    await settle(() => view.container.querySelectorAll(".conv-turn").length >= 5, "transcript render");
    await flush();
    const visibility = Object.getOwnPropertyDescriptor(Document.prototype, "visibilityState");
    const raf = window.requestAnimationFrame;
    let rafRequests = 0;
    try {
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
      // A hidden tab never runs animation frames.
      window.requestAnimationFrame = () => {
        rafRequests += 1;
        return 0;
      };
      resetRenderCounts();
      await act(async () => {
        transport.live?.({
          id: "hidden:1",
          event: "text_delta",
          identity: CHAT_IDENTITY,
          interactionId: "hidden-turn",
          timestampMs: 1_900_000_000_000,
          cursor: "console:900000",
          data: "hidden token",
        });
      });
      expect(rafRequests).toBe(0);
      expect(counts["ConsoleApp"] ?? 0).toBe(0);
      await act(async () => {
        await new Promise((resolve) => setTimeout(resolve, 300));
      });
      expect(counts["ConsoleApp"] ?? 0).toBe(1);
      expect(view.container.textContent).toContain("hidden token");
      // Becoming visible brings a pending timer flush forward immediately.
      resetRenderCounts();
      await act(async () => {
        transport.live?.({
          id: "hidden:2",
          event: "text_delta",
          identity: CHAT_IDENTITY,
          interactionId: "hidden-turn",
          timestampMs: 1_900_000_000_001,
          cursor: "console:900001",
          data: " then visible",
        });
      });
      expect(counts["ConsoleApp"] ?? 0).toBe(0);
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
      await act(async () => {
        document.dispatchEvent(new Event("visibilitychange"));
      });
      expect(counts["ConsoleApp"] ?? 0).toBe(1);
      expect(view.container.textContent).toContain("then visible");
    } finally {
      window.requestAnimationFrame = raf;
      delete (document as unknown as Record<string, unknown>).visibilityState;
      if (visibility) Object.defineProperty(Document.prototype, "visibilityState", visibility);
      view.unmount();
    }
  }, 30_000);

  it("issues no timeline queries while idle with 4 docked chats, visible or hidden", async () => {
    const m = await measure(50, { idleMs: 2_500, panels: 4 });
    console.log(
      `[typing-lag] 4 docked chats: idle timeline queries visible ${m.idleTimelineQueries} hidden ${m.hiddenIdleTimelineQueries} per 2.5 s`,
    );
    expect(m.idleTimelineQueries).toBe(0);
    expect(m.hiddenIdleTimelineQueries).toBe(0);
  }, 60_000);

  // N is the number of user/agent message pairs, so E = 2N frames in the
  // identity log: N=2500 is the E=5000 point, N=3000 exceeds the log cap.
  for (const n of [50, 500, 2000, 2500, 3000]) {
    it(`N=${n}: keystrokes do not re-render the app root, sidebar or transcript`, async () => {
      const m = await measure(n, { idleMs: n === 50 ? 2_500 : 0 });
      // Printed for the PR body; the assertions below are the contract.
      console.log(
        `[typing-lag] N=${n} keystroke ${stats(m.keystrokeMs)} renders ${JSON.stringify(m.keystrokeRenders)}`,
      );
      console.log(
        `[typing-lag] N=${n} controlled-sse-dispatch ${stats(m.frameMs)} controlled-flush ${m.burstMs.toFixed(2)} ms renders ${JSON.stringify(m.frameRenders)}`,
      );
      console.log(
        `[typing-lag] N=${n} mounted turns ${m.mountedTurns} of ${m.lastTurnIndex + 1}; idle timeline queries visible ${m.idleTimelineQueries} hidden ${m.hiddenIdleTimelineQueries} per 2.5 s`,
      );
      // Only the tail window of turns is mounted.
      expect(m.mountedTurns).toBeLessThanOrEqual(TRANSCRIPT_WINDOW_TURNS);
      // The per-identity log is capped: trimming keeps between the ceiling
      // and ceiling plus slack, i.e. at most 2750 pairs at N=3000.
      expect(m.lastTurnIndex + 1).toBeLessThanOrEqual(Math.min(n, 2750));
      if (n > 2750) expect(m.lastTurnIndex + 1).toBeGreaterThanOrEqual(2500);
      // No polling: the live stream is the only source while connected,
      // whether the tab is visible or hidden.
      if (n === 50) {
        expect(m.idleTimelineQueries).toBe(0);
        expect(m.hiddenIdleTimelineQueries).toBe(0);
      }
      const perKeystroke = (name: string) => (m.keystrokeRenders[name] ?? 0) / KEYSTROKES;
      expect(perKeystroke("ConsoleApp")).toBe(0);
      expect(perKeystroke("Sidebar")).toBe(0);
      expect(perKeystroke("SignalsRail")).toBe(0);
      expect(perKeystroke("VoiceBar")).toBe(0);
      expect(perKeystroke("ChatPane")).toBe(0);
      expect(perKeystroke("TranscriptView")).toBe(0);
      // 20 independent events before one controlled animation frame coalesce
      // to one sidebar render; React event batching cannot satisfy this alone.
      expect(m.frameRenders["Sidebar"] ?? 0).toBeLessThanOrEqual(1);
      expect(m.frameRenders["SignalsRail"] ?? 0).toBeLessThanOrEqual(1);
      expect(m.frameRenders["VoiceBar"] ?? 0).toBeLessThanOrEqual(1);
      expect(m.frameRenders["ConsoleApp"] ?? 0).toBeLessThanOrEqual(2);
      // Only the streaming row re-renders; every other mounted row is
      // skipped by MessageRow's content comparator. A log trim (N=3000)
      // legitimately remounts the window once because entry ids shift.
      if (n * 2 <= 5000) expect(m.frameRenders["MessageRow"] ?? 0).toBeLessThanOrEqual(2);
      const meanKeystroke = m.keystrokeMs.reduce((a, b) => a + b, 0) / m.keystrokeMs.length;
      expect(meanKeystroke).toBeLessThan(25);
    }, 120_000);
  }
});
