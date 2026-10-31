// Host-provided attention items for the "Needs you" list.
//
// A host configures same-origin JSON sources (console config
// `[[attention.sources]]`). Each answers `{"items": [...]}`; the console reads
// them with the operator's session and shows every item as a link card beside
// the gating approvals. The console never decides an item: the card's only
// action opens the host's own page (top level), which owns the decision.
// A source that fails is reported by its label and never hides approvals or
// another source's items.

export interface HostAttentionSourceConfig {
  id: string;
  label: string;
  url: string;
}

export interface HostAttentionDetail {
  label: string;
  value: string;
}

export interface HostAttentionItem {
  sourceId: string;
  sourceLabel: string;
  id: string;
  title: string;
  details: HostAttentionDetail[];
  href: string;
  tone?: string;
}

export interface HostAttentionSnapshot {
  status: "idle" | "loading" | "ready";
  items: HostAttentionItem[];
  /** Labels of the sources that could not be read on the last load. */
  failedSources: string[];
}

export const HOST_ATTENTION_LIMITS = {
  itemsPerSource: 50,
  detailsPerItem: 12,
  textLength: 300,
  /** A source answer larger than this fails that source. */
  responseBytes: 256 * 1024,
  /** A source that has not answered within this fails alone. */
  timeoutMs: 10_000,
} as const;

const TONES = new Set(["attention", "info", "warning", "critical"]);

/** The operator panel path rules: one leading "/", no scheme or authority, no
 *  whitespace, control characters or backslashes, no "." or ".." segment in
 *  any spelling, at most 2048 UTF-8 bytes. */
export function isSameOriginPath(value: unknown): value is string {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//")) return false;
  if (new TextEncoder().encode(value).length > 2048) return false;
  // eslint-disable-next-line no-control-regex
  if (/[\s\u0000-\u001f\u007f-\u009f\\\uFEFF]/.test(value)) return false;
  const path = value.split(/[?#]/, 1)[0];
  return !path.split("/").some((segment) => {
    const decoded = segment.toLowerCase().replace(/%2e/g, ".");
    return decoded === "." || decoded === "..";
  });
}

function text(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (!trimmed) return null;
  return trimmed.length > HOST_ATTENTION_LIMITS.textLength
    ? `${trimmed.slice(0, HOST_ATTENTION_LIMITS.textLength - 1)}…`
    : trimmed;
}

/** The valid items of one source's answer, bounded; anything else is dropped. */
export function normalizeHostAttention(source: HostAttentionSourceConfig, body: unknown): HostAttentionItem[] | null {
  if (!body || typeof body !== "object" || !Array.isArray((body as { items?: unknown }).items)) return null;
  const items: HostAttentionItem[] = [];
  const seen = new Set<string>();
  for (const raw of (body as { items: unknown[] }).items) {
    if (items.length >= HOST_ATTENTION_LIMITS.itemsPerSource) break;
    if (!raw || typeof raw !== "object") continue;
    const record = raw as Record<string, unknown>;
    const id = text(record.id);
    const title = text(record.title);
    if (!id || !title || seen.has(id) || !isSameOriginPath(record.href)) continue;
    const details: HostAttentionDetail[] = [];
    if (Array.isArray(record.details)) {
      for (const detail of record.details) {
        if (details.length >= HOST_ATTENTION_LIMITS.detailsPerItem) break;
        if (!detail || typeof detail !== "object") continue;
        const label = text((detail as Record<string, unknown>).label);
        const value = text((detail as Record<string, unknown>).value);
        if (label && value) details.push({ label, value });
      }
    }
    const tone = typeof record.tone === "string" && TONES.has(record.tone) ? record.tone : undefined;
    seen.add(id);
    items.push({ sourceId: source.id, sourceLabel: source.label, id, title, details, href: record.href, tone });
  }
  return items;
}

/** The body as text, or null when it is larger than `limit` bytes. Stops
 *  reading (and cancels the stream) as soon as the limit is passed. */
async function readCapped(response: Response, limit: number): Promise<string | null> {
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > limit) {
    // Never left unread on a shared connection.
    void response.body?.cancel().catch(() => {});
    return null;
  }
  const reader = response.body?.getReader?.();
  if (!reader) {
    const text = await response.text();
    return new TextEncoder().encode(text).length > limit ? null : text;
  }
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) {
      void reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return new TextDecoder().decode(bytes);
}

async function loadSource(
  source: HostAttentionSourceConfig,
  fetchImpl: typeof fetch,
  signal: AbortSignal | undefined,
  timeoutMs: number,
): Promise<HostAttentionItem[] | null> {
  // Checked again here, before any request: only a same-origin path is fetched.
  if (!isSameOriginPath(source.url)) return null;
  const controller = new AbortController();
  const abort = () => controller.abort();
  if (signal?.aborted) return null;
  signal?.addEventListener("abort", abort, { once: true });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<null>((resolve) => {
    timer = setTimeout(() => { controller.abort(); resolve(null); }, timeoutMs);
  });
  const read = (async () => {
    try {
      const response = await fetchImpl(source.url, {
        credentials: "same-origin",
        headers: { accept: "application/json" },
        redirect: "error",
        signal: controller.signal,
      });
      if (!response.ok) {
        void response.body?.cancel().catch(() => {});
        return null;
      }
      const text = await readCapped(response, HOST_ATTENTION_LIMITS.responseBytes);
      return text === null ? null : normalizeHostAttention(source, JSON.parse(text));
    } catch {
      return null;
    }
  })();
  try {
    // A source that never answers fails at its timeout, even when its fetch
    // ignores the abort; it delays the other sources' items by at most that.
    return await Promise.race([read, timedOut]);
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
    // Whatever was not read is released (an abort after a complete read is a no-op).
    controller.abort();
  }
}

/** Read every source once. Each source fails alone: an error, a non-2xx or
 *  unparsable answer, an oversized answer or no answer within the timeout. */
export async function loadHostAttention(
  sources: readonly HostAttentionSourceConfig[],
  fetchImpl: typeof fetch,
  signal?: AbortSignal,
  timeoutMs: number = HOST_ATTENTION_LIMITS.timeoutMs,
): Promise<HostAttentionSnapshot> {
  const results = await Promise.all(sources.map(async (source) => ({
    source,
    items: await loadSource(source, fetchImpl, signal, timeoutMs),
  })));
  return {
    status: "ready",
    items: results.flatMap((result) => result.items || []),
    failedSources: results.filter((result) => result.items === null).map((result) => result.source.label),
  };
}

export interface HostAttentionPollingEnvironment {
  fetch: typeof fetch;
  visible(): boolean;
  onVisible(callback: () => void): () => void;
  setInterval(callback: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
}

/** Read the sources now, every `intervalMs` while visible and when the page
 *  becomes visible again, publishing each completed load. A load that a newer
 *  one superseded is never published: its aborted sources are not failures.
 *  Returns the stop function. */
export function startHostAttentionPolling(
  sources: readonly HostAttentionSourceConfig[],
  publish: (snapshot: HostAttentionSnapshot) => void,
  environment: HostAttentionPollingEnvironment,
  intervalMs = 30_000,
): () => void {
  let stopped = false;
  let current: AbortController | null = null;
  const load = async () => {
    if (stopped || !environment.visible()) return;
    current?.abort();
    const mine = new AbortController();
    current = mine;
    const snapshot = await loadHostAttention(sources, environment.fetch, mine.signal);
    if (!stopped && current === mine) publish(snapshot);
  };
  void load();
  const timer = environment.setInterval(() => void load(), intervalMs);
  const stopVisible = environment.onVisible(() => void load());
  return () => {
    stopped = true;
    current?.abort();
    environment.clearInterval(timer);
    stopVisible();
  };
}
