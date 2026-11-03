import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { assistantPresentationEntries } from "../../../console-core/src/assistant-presentation";
import type { ConversationMessageEntry } from "../../../console-core/src/conversation";
import type { ConsoleFrame } from "../../../console-core/src/runtime-types";
import { buildConversationViewState, mapFramesToTimelineEntries as shared } from "../../../console-core/src/adapters";
import { mapFramesToTimelineEntries as stock } from "../../../../console/src/lib/adapters";
import { ChatPane } from "../../../../console/src/panels/ChatPane";
import { ConversationTranscript } from "./conversation-transcript";
import { ConversationBlobReaderProvider, type ConversationBlobReader } from "./blob-image";
import { createHttpConsoleTransport as stockTransport } from "../../../../console/src/lib/headless";
import { createHttpConsoleTransport as sharedTransport } from "../../../console-core/src/headless";

const agent = { agent_id: "worker", member_id: "worker", identity: "worker", label: "Worker", kind: "mob_agent" };
const callId = "poll-empty";
const resultBytes = '{"events":[],"note":"A\u030a, å and 🚀"}';
const noop = () => {};
function frame(id: string, event: string, data: Record<string, unknown>, scope: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data, runtimeKey: "runtime", identity: "worker", sessionId: "session", runId: "run",
    interactionId: "interaction", sourceKind: "console_event", timestampMs: 1000, ...scope };
}
function liveTools(ids: string[], scope: Partial<ConsoleFrame> = {}) {
  return ids.flatMap(id => [
    frame(`request-${id}`, "tool_call_requested", { id, name: "workgraph_events", args: { after_seq: 1 } }, scope),
    frame(`result-${id}`, "tool_execution_completed", { id, name: "workgraph_events", result: resultBytes, is_error: false }, scope),
  ]);
}
function savedTools(ids: string[], text = "", scope: Partial<ConsoleFrame> = {}) {
  return frame("canonical-tool-frame", "assistant_message", {
    assistant_message_id: "assistant-tool",
    message: { role: "block_assistant", blocks: [
      ...ids.map(id => ({ block_type: "tool_use", data: { id, name: "workgraph_events", args: { after_seq: 1 } } })),
      ...(text ? [{ block_type: "text", data: { text } }] : []),
    ], stop_reason: text ? "end_turn" : "tool_use" }, text, result: text,
  }, { sourceKind: "session_history", ...scope });
}
function toolNodes(container: HTMLElement) {
  const tool = container.querySelector<HTMLElement>(".cc-tool-call")!;
  expect(tool).toBeTruthy();
  const header = tool.querySelector<HTMLElement>(".cc-tool-call__header")!;
  const row = tool.closest<HTMLElement>("[data-conversation-row-id]")!;
  const turn = row.closest<HTMLElement>("[data-conversation-turn-id]")!;
  return { tool, header, row, turn };
}
function openResult(container: HTMLElement, expectedBytes = resultBytes) {
  const nodes = toolNodes(container);
  for (const header of container.querySelectorAll<HTMLElement>(".cc-tool-call__header")) {
    if (header.getAttribute("aria-expanded") !== "true") fireEvent.click(header);
  }
  const results = [...container.querySelectorAll<HTMLElement>(".cc-tool-call__section")]
    .filter(node => node.querySelector(".cc-tool-call__section-label")?.textContent === "Result")
    .map(node => node.querySelector("pre")!);
  expect(results.length).toBeGreaterThan(0);
  results.forEach(node => expect(node.textContent).toBe(expectedBytes));
  return { ...nodes, results };
}

for (const [surface, map] of [["stock", stock], ["shared", shared]] as const) {
  function view(frames: ConsoleFrame[]) {
    const entries = map(agent, frames, { textMode: "markdown" });
    return surface === "stock"
      ? <ChatPane agent={agent} agentLabel="Worker" identity="worker" entries={entries} phase={null}
          draft="Unsent draft" sending={false} staged={[]} onDraftChange={noop} onStagedChange={noop} onSend={() => false} />
      : <ConversationTranscript viewState={buildConversationViewState({ memberId: "worker", agentLabel: "Worker", entries })} />;
  }
  describe(`${surface} tool presentation identity`, () => {
    it("opens a fresh pending call and respects manual close until new outcome evidence", () => {
      const id = "held-read";
      const args = { path: "late-A\u030a-å-🚀.txt" };
      const pending = [frame("held-request", "tool_call_requested", { id, name: "read_file", args })];
      const result = render(view(pending));
      const original = toolNodes(result.container);
      expect(original.tool.classList.contains("cc-tool-call--pending")).toBe(true);
      expect(original.header.getAttribute("aria-expanded")).toBe("true");
      const sections = [...original.tool.querySelectorAll<HTMLElement>(".cc-tool-call__section")];
      expect(sections.map(node => node.querySelector(".cc-tool-call__section-label")?.textContent)).toEqual(["Input"]);
      expect(sections[0].querySelector("pre")?.textContent).toBe(JSON.stringify(args));
      fireEvent.click(original.header);
      expect(original.header.getAttribute("aria-expanded")).toBe("false");
      result.rerender(view([...pending]));
      expect(toolNodes(result.container).header).toBe(original.header);
      expect(original.header.getAttribute("aria-expanded")).toBe("false");
      const started = frame("held-started", "tool_execution_started", { id, name: "read_file", args });
      result.rerender(view([...pending, started]));
      expect(toolNodes(result.container).header).toBe(original.header);
      expect(original.header.getAttribute("aria-expanded")).toBe("false");
      expect(original.tool.querySelector(".cc-tool-call__body")).toBeNull();
      const canonical = frame("canonical-held-call", "assistant_message", {
        assistant_message_id: "assistant-held-call",
        message: { role: "block_assistant", blocks: [{ block_type: "tool_use", data: { id, name: "read_file", args } }], stop_reason: "tool_use" },
      }, { sourceKind: "session_history" });
      result.rerender(view([...pending, started, canonical]));
      const hydrated = toolNodes(result.container);
      expect(hydrated.row).toBe(original.row);
      expect(hydrated.tool).toBe(original.tool);
      expect(hydrated.header).toBe(original.header);
      expect(hydrated.header.getAttribute("aria-expanded")).toBe("true");
      expect(hydrated.tool.querySelector(".cc-tool-call__status")?.textContent).toContain("Completion unknown");
      const reopened = [...hydrated.tool.querySelectorAll<HTMLElement>(".cc-tool-call__section")];
      expect(reopened.map(node => node.querySelector(".cc-tool-call__section-label")?.textContent)).toEqual(["Input"]);
      expect(reopened[0].querySelector("pre")?.textContent).toBe(JSON.stringify(args));
      fireEvent.click(hydrated.header);
      expect(hydrated.header.getAttribute("aria-expanded")).toBe("false");
      result.rerender(view([...pending, started, { ...canonical }]));
      expect(toolNodes(result.container).header).toBe(original.header);
      expect(original.header.getAttribute("aria-expanded")).toBe("false");
      expect(original.tool.querySelector(".cc-tool-call__body")).toBeNull();
    });
    for (const variant of ["tool-only", "with-user", "with-answer", "bundled-answer", "two-tools", "two-owners"] as const) {
      it(`keeps exact opened tool DOM across canonical hydration: ${variant}`, () => {
        const ids = variant === "two-tools" || variant === "two-owners" ? [callId, "poll-again"] : [callId];
        const user = variant === "with-user" ? [frame("user", "user_input", { content: "Review the events." }, { sourceKind: "send" })] : [];
        const text = "The events are checked.";
        const answer = variant === "with-answer" || variant === "bundled-answer"
          ? [frame("answer", "text_delta", { assistant_message_id: variant === "bundled-answer" ? "assistant-tool" : "assistant-answer", delta: text })] : [];
        const live = [...user, ...liveTools(ids), ...answer];
        const result = render(view(live));
        const expectedBytes = ids.length > 1 && surface === "stock" ? JSON.stringify(JSON.parse(resultBytes), null, 2) : resultBytes;
        const original = openResult(result.container, expectedBytes);
        const rowId = original.row.dataset.conversationRowId;
        const canonical = variant === "two-owners" ? ids.map((id, index) => {
          const saved = savedTools([id]);
          saved.id = index ? "canonical-second-tool" : "canonical-tool-frame";
          (saved.data as Record<string, unknown>).assistant_message_id = `assistant-tool-${index}`;
          return saved;
        }) : [savedTools(ids, variant === "bundled-answer" ? text : "")];
        const projected = map(agent, [...live, ...canonical], { textMode: "markdown" });
        expect(projected.some(entry => entry.id === "canonical-tool-frame")).toBe(true);
        result.rerender(view([...live, ...canonical]));
        const current = toolNodes(result.container);
        expect(current.row.dataset.conversationRowId).toBe(rowId);
        expect(current.turn).toBe(original.turn);
        expect(current.row).toBe(original.row);
        expect(current.tool).toBe(original.tool);
        expect(current.header).toBe(original.header);
        expect(current.header.getAttribute("aria-expanded")).toBe("true");
        const results = [...result.container.querySelectorAll<HTMLElement>(".cc-tool-call__section")]
          .filter(node => node.querySelector(".cc-tool-call__section-label")?.textContent === "Result")
          .map(node => node.querySelector("pre")!);
        expect(results).toHaveLength(original.results.length);
        results.forEach((node, index) => { expect(node).toBe(original.results[index]); expect(node.textContent).toBe(expectedBytes); });
        const rowIds = [...result.container.querySelectorAll<HTMLElement>("[data-conversation-row-id]")].map(node => node.dataset.conversationRowId);
        expect(new Set(rowIds).size).toBe(rowIds.length);
      });
    }
    for (const field of ["runtimeKey", "identity", "sessionId", "runId"] as const) {
      it(`does not reuse disclosure state for the same call ID in another ${field}`, () => {
        const result = render(view(liveTools([callId])));
        const original = openResult(result.container);
        result.rerender(view(liveTools([callId], { [field]: `other-${field}` })));
        const current = toolNodes(result.container);
        expect(current.row.dataset.conversationRowId).not.toBe(original.row.dataset.conversationRowId);
        expect(current.tool).not.toBe(original.tool);
        expect(current.header.getAttribute("aria-expanded")).toBe("false");
      });
    }
  });
}


describe("typed tool display authority", () => {
  function projected(source: ConsoleFrame, toolCallId = callId) {
    const entry: ConversationMessageEntry = { kind: "message", id: source.id, identity: { role: "assistant", label: "Worker" },
      variant: "rich", blocks: [{ type: "tool-call", toolCallId, name: "workgraph_events", arguments: "{}", status: "success" }] };
    return assistantPresentationEntries([entry], new Map(), new Map([[source.id, source]]))[0];
  }
  for (const field of ["runtimeKey", "identity", "sessionId", "runId"] as const) {
    for (const value of [undefined, " "]) {
      it(`keeps the source fallback when ${field} is ${value === undefined ? "absent" : "blank"}`, () => {
        const source = frame("original-source", "tool_call_requested", { id: callId }, { [field]: value });
        expect(projected(source).renderKey).toBeUndefined();
        expect(projected(source).id).toBe("original-source");
      });
    }
  }
  it("does not authorize an invented canonical tool ID from another live request", () => {
    const saved = savedTools([]);
    (saved.data as { message: { blocks: unknown[] } }).message.blocks = [{ block_type: "tool_use", data: { name: "workgraph_events" } }];
    const live = frame("live", "tool_call_requested", { id: "history-tool-1" });
    const entry: ConversationMessageEntry = { kind: "message", id: saved.id, identity: { role: "assistant", label: "Worker" },
      variant: "rich", blocks: [{ type: "tool-call", toolCallId: "history-tool-1", name: "workgraph_events", arguments: "{}", status: "success" }] };
    const [result] = assistantPresentationEntries([entry], new Map(), new Map([[saved.id, saved], [live.id, live]]));
    expect(result.renderKey).toBeUndefined();
    expect(result.id).toBe(saved.id);
  });
  for (const event of ["runtime_notice", "tool_call_requested"]) {
    it(`does not borrow a typed call for an unowned ${event} source`, () => {
      const source = frame("synthetic-source", event, { tool_call_id: "", id: callId });
      const live = frame("actual-call", "tool_call_requested", { id: callId });
      const entry: ConversationMessageEntry = { kind: "message", id: source.id, identity: { role: "assistant", label: "Worker" },
        variant: "rich", blocks: [{ type: "tool-call", toolCallId: callId, name: "workgraph_events", arguments: "{}", status: "success" }] };
      const [result] = assistantPresentationEntries([entry], new Map(), new Map([[source.id, source], [live.id, live]]));
      expect(result.renderKey).toBeUndefined();
      expect(result.id).toBe(source.id);
    });
  }
  it("keeps ambiguous canonical owners distinct instead of sharing a tool key", () => {
    const sources = [savedTools([callId]), savedTools([callId])];
    sources[1].id = "canonical-other-owner";
    (sources[1].data as Record<string, unknown>).assistant_message_id = "assistant-other-tool";
    const entries: ConversationMessageEntry[] = sources.map(source => ({ kind: "message", id: source.id,
      identity: { role: "assistant", label: "Worker" }, variant: "rich", blocks: [{ type: "tool-call",
        toolCallId: callId, name: "workgraph_events", arguments: "{}", status: "success" }] }));
    const rows = assistantPresentationEntries(entries, new Map(sources.map(source => [source.id, `assistant:${source.id}`])),
      new Map(sources.map(source => [source.id, source])));
    expect(new Set(rows.map(row => row.renderKey)).size).toBe(2);
    rows.forEach((row, index) => { expect(row.id).toBe(sources[index].id); expect(row.renderKey).not.toMatch(/^tool:/); });
  });
  it("preserves opaque scope and call ID bytes", () => {
    const source = frame("original-source", "tool_call_requested", { id: " call " }, { runId: " run " });
    const padded = projected(source, " call ");
    const plain = projected(frame("other-source", "tool_call_requested", { id: "call" }), "call");
    expect(padded.renderKey).toContain('" run "');
    expect(padded.renderKey).toContain('" call "');
    expect(padded.renderKey).not.toBe(plain.renderKey);
    expect(padded.id).toBe(source.id);
  });
});

// A stored tool image is read with the host transport's request authority,
// the credentials it gives every Console request. The gateway's /blobs route
// answers 401 to a request without them, as it does under require_app_auth.
const BLOB_BASE = "http://gateway.test";
const BLOB_ID = "sha256:abc";
const BLOB_URL = `${BLOB_BASE}/blobs/${encodeURIComponent(BLOB_ID)}`;
const HOST_CREDENTIAL = "Bearer host-session";
type BlobRequest = { url: string; authorization: string | null; credentials?: RequestCredentials };
function toolImageFrames(image: Record<string, unknown>): ConsoleFrame[] {
  return [
    frame("image-request", "tool_call_requested", { id: "shot", name: "computer_screenshot", args: {} }),
    frame("image-result", "tool_result_received", { id: "shot", name: "computer_screenshot", is_error: false,
      content: [{ type: "text", text: "Captured the screen." }, image] }),
  ];
}
const storedImage = { type: "image", media_type: "image/png", source: "blob", blob_id: BLOB_ID };
/// The protected /blobs route. `accept` decides which requests carry valid
/// credentials; `contentType` is what the stored blob declares.
function protectedBlobRoute(requests: BlobRequest[], accept: (request: BlobRequest) => boolean, contentType = "image/png") {
  return async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = { url: String(input), authorization: new Headers(init?.headers).get("authorization"), credentials: init?.credentials };
    requests.push(request);
    const status = request.url !== BLOB_URL ? 404 : accept(request) ? 200 : 401;
    return {
      ok: status === 200,
      status,
      headers: new Headers({ "content-type": contentType }),
      arrayBuffer: async () => new TextEncoder().encode("png-bytes").buffer,
    } as unknown as Response;
  };
}
function stubObjectUrls() {
  const created: Blob[] = [];
  const revoked: string[] = [];
  const target = URL as unknown as { createObjectURL?: (blob: Blob) => string; revokeObjectURL?: (url: string) => void };
  const original = { create: target.createObjectURL, revoke: target.revokeObjectURL };
  target.createObjectURL = (blob: Blob) => { created.push(blob); return `blob:console-image-${created.length}`; };
  target.revokeObjectURL = (url: string) => { revoked.push(url); };
  return { created, revoked, restore: () => { target.createObjectURL = original.create; target.revokeObjectURL = original.revoke; } };
}
// jsdom lacks AbortSignal.any, which every supported browser has and the
// default transport's fetch helper combines its timeout with.
function ensureAbortSignalAny(): () => void {
  const target = AbortSignal as unknown as { any?: (signals: AbortSignal[]) => AbortSignal };
  if (typeof target.any === "function") return () => {};
  target.any = (signals: AbortSignal[]) => {
    const controller = new AbortController();
    for (const signal of signals) {
      if (signal.aborted) { controller.abort(signal.reason); break; }
      signal.addEventListener("abort", () => controller.abort(signal.reason), { once: true });
    }
    return controller.signal;
  };
  return () => { delete target.any; };
}
function bareBlobLoads(container: HTMLElement) {
  return [...container.querySelectorAll<HTMLElement>("[src]")].filter(node => (node.getAttribute("src") ?? "").includes("/blobs/"));
}

for (const [surface, map, transport] of [["stock", stock, stockTransport], ["shared", shared, sharedTransport]] as const) {
  function imageView(frames: ConsoleFrame[], reader?: ConversationBlobReader) {
    const entries = map(agent, frames, { textMode: "markdown", blobBaseUrl: BLOB_BASE });
    const view = surface === "stock"
      ? <ChatPane agent={agent} agentLabel="Worker" identity="worker" entries={entries} phase={null} blobReader={reader}
          draft="" sending={false} staged={[]} onDraftChange={noop} onStagedChange={noop} onSend={() => false} />
      : <ConversationTranscript viewState={buildConversationViewState({ memberId: "worker", agentLabel: "Worker", entries })} />;
    return surface === "stock" ? view : <ConversationBlobReaderProvider reader={reader}>{view}</ConversationBlobReaderProvider>;
  }
  // The default HTTP transport's reader: what the Console uses without a host transport.
  const defaultReader: ConversationBlobReader = input => transport({ baseUrl: BLOB_BASE }).readBlob!(input);
  async function shownImage(container: HTMLElement) {
    return waitFor(() => {
      const node = container.querySelector<HTMLImageElement>(".cc-tool-call__image");
      expect(node).toBeTruthy();
      return node!;
    });
  }
  describe(`${surface} stored tool images behind app auth`, () => {
    let objectUrls: ReturnType<typeof stubObjectUrls> | undefined;
    let restoreAbortSignalAny = () => {};
    beforeEach(() => { restoreAbortSignalAny = ensureAbortSignalAny(); });
    afterEach(() => {
      // Unmount (revoking object URLs) while the URL stubs are still installed.
      cleanup();
      objectUrls?.restore();
      objectUrls = undefined;
      restoreAbortSignalAny();
      vi.unstubAllGlobals();
      vi.restoreAllMocks();
    });

    it("reads the image with the host transport's credential and opens that same object URL", async () => {
      const requests: BlobRequest[] = [];
      const route = protectedBlobRoute(requests, request => request.authorization === HOST_CREDENTIAL);
      // A host transport whose requests carry its bearer, as its other calls do.
      const hostReader: ConversationBlobReader = async ({ blobId, signal }) => {
        const response = await route(`${BLOB_BASE}/blobs/${encodeURIComponent(blobId)}`, { signal, headers: { authorization: HOST_CREDENTIAL } });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        return new Blob([await response.arrayBuffer()], { type: "image/png" });
      };
      objectUrls = stubObjectUrls();
      const open = vi.spyOn(window, "open").mockImplementation(() => null);
      const result = render(imageView(toolImageFrames(storedImage), hostReader));
      const image = await shownImage(result.container);
      expect(image.getAttribute("src")).toBe("blob:console-image-1");
      expect(requests.map(({ url, authorization }) => ({ url, authorization }))).toEqual([{ url: BLOB_URL, authorization: HOST_CREDENTIAL }]);
      expect(bareBlobLoads(result.container)).toEqual([]);
      fireEvent.click(image.closest("button")!);
      expect(open).toHaveBeenCalledWith("blob:console-image-1", "_blank", "noopener,noreferrer");
      expect(requests).toHaveLength(1);
      result.unmount();
      expect(objectUrls.revoked).toEqual(["blob:console-image-1"]);
    });

    it("a Bearer host's route refuses the image without the transport's credential and serves it through the transport", async () => {
      const requests: BlobRequest[] = [];
      const route = protectedBlobRoute(requests, request => request.authorization === HOST_CREDENTIAL);
      const statuses: number[] = [];
      vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
        const response = await route(input, init);
        statuses.push(response.status);
        return response;
      });
      // A host transport whose requests carry its Bearer, as its other calls do.
      const hostTransport = {
        async readBlob({ blobId, signal }: { blobId: string; signal?: AbortSignal }): Promise<Blob> {
          const response = await fetch(`${BLOB_BASE}/blobs/${encodeURIComponent(blobId)}`, { signal, headers: { authorization: HOST_CREDENTIAL } });
          if (!response.ok) throw new Error(`HTTP ${response.status}`);
          return new Blob([await response.arrayBuffer()], { type: response.headers.get("content-type") ?? "" });
        },
      };
      objectUrls = stubObjectUrls();
      // Without the host transport's credential: the plain same-origin read gets 401.
      const refused = render(imageView(toolImageFrames(storedImage), defaultReader));
      await waitFor(() => expect(refused.container.querySelector(".cc-tool-call__image-unavailable")).toBeTruthy());
      refused.unmount();
      // Through the host transport: 200, shown from an object URL.
      const served = render(imageView(toolImageFrames(storedImage), input => hostTransport.readBlob(input)));
      expect((await shownImage(served.container)).getAttribute("src")).toBe("blob:console-image-1");
      expect(statuses).toEqual([401, 200]);
      expect(requests.map(request => request.authorization)).toEqual([null, HOST_CREDENTIAL]);
      expect(requests.every(request => request.url === BLOB_URL)).toBe(true);
    });

    it("reads through the default transport same-origin with the browser's credentials and no token in the URL", async () => {
      const requests: BlobRequest[] = [];
      // A session cookie or an authenticating front for /blobs/{id}: the
      // request is accepted when it carries the browser's same-origin credentials.
      vi.stubGlobal("fetch", protectedBlobRoute(requests, request => request.credentials === "same-origin"));
      objectUrls = stubObjectUrls();
      const result = render(imageView(toolImageFrames(storedImage), defaultReader));
      const image = await shownImage(result.container);
      expect(image.getAttribute("src")).toBe("blob:console-image-1");
      expect(requests).toEqual([{ url: BLOB_URL, authorization: null, credentials: "same-origin" }]);
      expect(requests[0].url).not.toMatch(/[?&]auth_token=/);
      expect(objectUrls.created.map(blob => blob.type)).toEqual(["image/png"]);
    });

    it("shows a refused read as unavailable instead of a broken bare load", async () => {
      const requests: BlobRequest[] = [];
      vi.stubGlobal("fetch", protectedBlobRoute(requests, () => false));
      objectUrls = stubObjectUrls();
      const result = render(imageView(toolImageFrames(storedImage), defaultReader));
      await waitFor(() => expect(result.container.querySelector(".cc-tool-call__image-unavailable")?.textContent).toBe("Image unavailable"));
      expect(requests).toHaveLength(1);
      expect(result.container.querySelector(".cc-tool-call__image")).toBeNull();
      expect(bareBlobLoads(result.container)).toEqual([]);
      expect(objectUrls.created).toEqual([]);
    });

    it("refuses bytes that are not a displayable image before they become an object URL", async () => {
      for (const reader of [
        // The default transport checks the declared media type.
        (() => { vi.stubGlobal("fetch", protectedBlobRoute([], () => true, "text/html")); return defaultReader; })(),
        // A host transport that returns other bytes is refused by the view.
        (async () => new Blob(["<script>"], { type: "image/svg+xml" })) as ConversationBlobReader,
      ]) {
        objectUrls = stubObjectUrls();
        const result = render(imageView(toolImageFrames(storedImage), reader));
        await waitFor(() => expect(result.container.querySelector(".cc-tool-call__image-unavailable")).toBeTruthy());
        expect(objectUrls.created).toEqual([]);
        result.unmount();
        objectUrls.restore();
      }
    });

    it("aborts a read whose image unmounts and keeps no object URL", async () => {
      let release!: () => void;
      let seen: AbortSignal | undefined;
      const slowReader: ConversationBlobReader = ({ signal }) => {
        seen = signal;
        return new Promise(resolve => { release = () => resolve(new Blob(["png"], { type: "image/png" })); });
      };
      objectUrls = stubObjectUrls();
      const result = render(imageView(toolImageFrames(storedImage), slowReader));
      await waitFor(() => expect(seen).toBeDefined());
      result.unmount();
      expect(seen!.aborted).toBe(true);
      release();
      await new Promise(resolve => setTimeout(resolve, 0));
      expect(objectUrls.created).toEqual([]);
    });

    it("replacing the reader for the same image drops the old image until the new read settles", async () => {
      objectUrls = stubObjectUrls();
      const readerA: ConversationBlobReader = async () => new Blob(["png-a"], { type: "image/png" });
      let refuseB: ((error: Error) => void) | undefined;
      const readerB: ConversationBlobReader = () => new Promise<Blob>((_, reject) => { refuseB = reject; });
      const frames = toolImageFrames(storedImage);
      const result = render(imageView(frames, readerA));
      expect((await shownImage(result.container)).getAttribute("src")).toBe("blob:console-image-1");
      // Same blob URL, another reader: A's image is gone while B is pending.
      result.rerender(imageView(frames, readerB));
      await waitFor(() => expect(refuseB).toBeTypeOf("function"));
      expect(result.container.querySelector(".cc-tool-call__image")).toBeNull();
      expect(objectUrls.revoked).toEqual(["blob:console-image-1"]);
      // B refuses: the image is unavailable, never A's.
      refuseB!(new Error("HTTP 401"));
      await waitFor(() => expect(result.container.querySelector(".cc-tool-call__image-unavailable")?.textContent).toBe("Image unavailable"));
      expect(result.container.querySelector(".cc-tool-call__image")).toBeNull();
      expect(objectUrls.created).toHaveLength(1);
    });

    it("keeps the direct URL when the host transport has no reader", () => {
      const result = render(imageView(toolImageFrames(storedImage)));
      expect(result.container.querySelector(".cc-tool-call__image")?.getAttribute("src")).toBe(BLOB_URL);
    });

    it("shows an inline image as it is, without a read", () => {
      const reads: string[] = [];
      const reader: ConversationBlobReader = async ({ blobId }) => { reads.push(blobId); return new Blob([]); };
      const result = render(imageView(toolImageFrames({ type: "image", media_type: "image/png", source: "inline", data: "iVBORw0KGgo=" }), reader));
      expect(result.container.querySelector(".cc-tool-call__image")?.getAttribute("src")).toBe("data:image/png;base64,iVBORw0KGgo=");
      expect(reads).toEqual([]);
    });
  });
}
