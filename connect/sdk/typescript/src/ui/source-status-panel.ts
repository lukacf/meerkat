import type { ToolkitSnapshot } from "./model.js";
import { validateProjection } from "./projection.js";
import { mountToolkitView } from "./view.js";
import type { ToolkitView } from "./view.js";

export const SOURCE_STATUS_DATA_PATH = "/host/toolkit/setup/data";
type StatusPage = { page_id: number; first: boolean; has_next: boolean };
type PageSelection = { page_id: number; direction: "refresh" | "next" };
type StatusEnvelope = { projection: ToolkitSnapshot; page?: StatusPage };
function checkedPage(value: unknown): StatusPage | undefined {
  if (value === undefined) return undefined; // Earlier whole-view envelopes.
  if (!value || typeof value !== "object") throw new TypeError("Invalid status page");
  const page = value as Record<string, unknown>;
  if (Object.keys(page).some(key => !["page_id", "first", "has_next"].includes(key))
    || typeof page.page_id !== "number" || !Number.isSafeInteger(page.page_id) || page.page_id < 1
    || typeof page.first !== "boolean" || typeof page.has_next !== "boolean") {
    throw new TypeError("Invalid status page");
  }
  return { page_id: page.page_id, first: page.first, has_next: page.has_next };
}

/** Read-only task pages. The host authenticates every read and owns its opaque
 * page position. This compiled helper has no protected review or access actions.
 */
export function mountSourceStatusPanel(
  container: HTMLElement,
  read: (selection?: PageSelection) => Promise<StatusEnvelope>,
): { refresh(): Promise<void>; first(): Promise<void>; next(): Promise<void>; dispose(): void } {
  const doc = container.ownerDocument;
  const status = doc.createElement("p");
  status.setAttribute("role", "status"); status.setAttribute("aria-live", "polite");
  const content = doc.createElement("div");
  const refresh = doc.createElement("button"); refresh.type = "button"; refresh.textContent = "Refresh status";
  const first = doc.createElement("button"); first.type = "button"; first.textContent = "First page";
  const next = doc.createElement("button"); next.type = "button"; next.textContent = "Next page";
  const navigation = doc.createElement("div"); navigation.append(first, next);
  const root = doc.createElement("section"); root.append(status, refresh, content); container.append(root);
  let request = 0; let disposed = false; let view: ToolkitView | undefined;
  let page: StatusPage | undefined; let pending: Promise<void> | undefined; let needsFirst = false;
  const clear = () => { view?.dispose(); view = undefined; content.replaceChildren(); };
  const buttons = (busy: boolean) => {
    refresh.disabled = busy || needsFirst; first.disabled = busy;
    next.disabled = busy || needsFirst || page?.has_next !== true;
    if (page || needsFirst) root.insertBefore(navigation, content); else navigation.remove();
  };
  function update(direction: "first" | "refresh" | "next" = "refresh"): Promise<void> {
    if (disposed) return Promise.resolve();
    if (pending) return pending;
    if (needsFirst && direction !== "first") return Promise.resolve();
    if (direction === "next" && page?.has_next !== true) return Promise.resolve();
    const selection = direction === "first" || !page ? undefined : { page_id: page.page_id, direction };
    const current = ++request;
    clear(); status.textContent = "Reading current task setup status..."; buttons(true);
    pending = Promise.resolve().then(async () => {
      try {
        if (disposed || current !== request) return;
        const envelope = await read(selection);
        if (disposed || current !== request) return;
        const snapshot = envelope.projection; const selected = checkedPage(envelope.page);
        validateProjection(snapshot);
        if (snapshot.tasks.some(task => task.controls.length || task.userActions.length || task.roles.length)
          || snapshot.connections.length || snapshot.frontends.length || snapshot.packages.length
          || snapshot.catalog !== undefined) throw new TypeError("The source status panel only accepts read-only task status");
        // Replace this page, including same-revision observations. Never append
        // old permissions or claim that separate reads form an atomic inventory.
        view = mountToolkitView(content, { scope: snapshot.scope, taskOnly: true, taskPage: selected !== undefined,
          dispatch: async () => { throw new Error("This panel has no setup actions"); } });
        view.update(snapshot); page = selected; needsFirst = false;
        status.textContent = selected ? selected.has_next
          ? "Showing one page. More history remains to inspect; later pages may have no matching entries."
          : "End of the history observed in this read. Use First page to check for new entries." : "";
      } catch {
        if (disposed || current !== request) return;
        clear(); page = undefined; needsFirst = true;
        status.textContent = "Task setup status is unavailable. Use First page to read current status. One status view is current per sign-in; another tab may have replaced this page. Your agent reports the task result separately.";
      }
    }).finally(() => { pending = undefined; if (!disposed) buttons(false); });
    return pending;
  }
  refresh.addEventListener("click", () => { void update(); });
  first.addEventListener("click", () => { void update("first"); });
  next.addEventListener("click", () => { void update("next"); });
  return { refresh: () => update(), first: () => update("first"), next: () => update("next"),
    dispose() { disposed = true; ++request; page = undefined; clear(); root.remove(); } };
}

/** Read the actual host envelope, retaining only its read-only projection and
 * page metadata. Neither review payloads nor access actions gain a UI here.
 */
export async function readSourceStatus(fetcher: typeof fetch = fetch, selection?: PageSelection): Promise<StatusEnvelope> {
  if (selection !== undefined && (!selection || Object.keys(selection).some(key => !["page_id", "direction"].includes(key))
    || !Number.isSafeInteger(selection.page_id) || selection.page_id < 1
    || !["refresh", "next"].includes(selection.direction))) throw new TypeError("Invalid page selection");
  const response = await fetcher(SOURCE_STATUS_DATA_PATH, {
    method: selection ? "POST" : "GET", credentials: "same-origin", cache: "no-store", redirect: "error",
    headers: selection ? { Accept: "application/json", "Content-Type": "application/json" } : { Accept: "application/json" },
    ...(selection ? { body: JSON.stringify(selection) } : {}),
  });
  if (!response.ok || !response.headers.get("content-type")?.startsWith("application/json")) {
    throw new Error("Task setup status read was not authorized or available");
  }
  const envelope: unknown = await response.json();
  if (!envelope || typeof envelope !== "object" || !("projection" in envelope)) throw new TypeError("Invalid status envelope");
  const projection = envelope.projection as ToolkitSnapshot;
  validateProjection(projection);
  const page = checkedPage("page" in envelope ? envelope.page : undefined);
  return { projection, ...(page ? { page } : {}) };
}

if (typeof document !== "undefined") {
  const container = document.getElementById("toolkit-source-status");
  if (container) {
    const panel = mountSourceStatusPanel(container, selection => readSourceStatus(fetch, selection));
    void panel.refresh();
    window.addEventListener("pagehide", () => panel.dispose(), { once: true });
  }
}
