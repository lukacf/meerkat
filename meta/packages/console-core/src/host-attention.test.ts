import assert from "node:assert/strict";
import test from "node:test";
import { HOST_ATTENTION_LIMITS, isSameOriginPath, loadHostAttention, normalizeHostAttention, startHostAttentionPolling, type HostAttentionSnapshot } from "./host-attention";

const source = { id: "host", label: "Host", url: "/host/attention" };
const item = (id: string, extra: Record<string, unknown> = {}) => ({
  id, title: `Confirm ${id}`, href: `/host/items/${id}`, details: [{ label: "For", value: "plugin:x" }], ...extra,
});
const requested: string[] = [];
function respond(map: Record<string, () => Response | Promise<Response>>): typeof fetch {
  requested.length = 0;
  return (async (url: string | URL | Request, init?: RequestInit) => {
    requested.push(String(url));
    assert.equal(init?.credentials, "same-origin");
    assert.equal(init?.redirect, "error");
    const handler = map[String(url)];
    if (!handler) throw new Error(`unexpected ${String(url)}`);
    return handler();
  }) as typeof fetch;
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

test("same-origin paths follow the operator panel path rules", () => {
  for (const ok of ["/host/attention", "/a/b?next=/../", "/a.b/..c"]) assert.equal(isSameOriginPath(ok), true, ok);
  for (const bad of ["https://evil.example/x", "//evil.example/x", "x/y", "/a b", "/a\\b", "/a/../b", "/%2e%2E/x", "/\uFEFFx", "/a\u0085b", 7, null]) {
    assert.equal(isSameOriginPath(bad), false, String(bad));
  }
});

test("items are validated, bounded and keep their source", () => {
  const items = normalizeHostAttention(source, {
    items: [
      item("a", { tone: "attention" }),
      item("b", { href: "https://evil.example/b" }),
      item("c", { href: "javascript:alert(1)" }),
      { id: "d", href: "/host/d" },
      item("a"),
      item("e", { tone: "loud", details: [{ label: "x" }, { label: "y", value: "z".repeat(400) }, "nope"] }),
    ],
  });
  assert.deepEqual(items?.map((entry) => entry.id), ["a", "e"]);
  assert.equal(items?.[0].sourceLabel, "Host");
  assert.equal(items?.[0].tone, "attention");
  assert.equal(items?.[1].tone, undefined);
  assert.equal(items?.[1].details.length, 1);
  assert.equal(items?.[1].details[0].value.length, HOST_ATTENTION_LIMITS.textLength);
  const many = normalizeHostAttention(source, { items: Array.from({ length: 80 }, (_, i) => item(`i${i}`)) });
  assert.equal(many?.length, HOST_ATTENTION_LIMITS.itemsPerSource);
  assert.equal(normalizeHostAttention(source, { nope: [] }), null);
  assert.equal(normalizeHostAttention(source, "items"), null);
});

test("a failing source is reported alone and never hides another's items", async () => {
  const other = { id: "other", label: "Other", url: "/other/attention" };
  const broken = { id: "broken", label: "Broken", url: "/broken/attention" };
  const down = { id: "down", label: "Down", url: "/down/attention" };
  const snapshot = await loadHostAttention([source, broken, other, down], respond({
    "/host/attention": () => json({ items: [item("a")] }),
    "/broken/attention": () => json({ items: "no" }),
    "/other/attention": () => json({ items: [item("b")] }),
    "/down/attention": () => json({ error: "forbidden" }, 403),
  }));
  assert.equal(snapshot.status, "ready");
  assert.deepEqual(snapshot.items.map((entry) => `${entry.sourceId}:${entry.id}`), ["host:a", "other:b"]);
  assert.deepEqual(snapshot.failedSources, ["Broken", "Down"]);
});

test("a network error or a non-path url fails only that source, and a non-path url is never requested", async () => {
  const snapshot = await loadHostAttention(
    [source, { id: "abs", label: "Absolute", url: "https://evil.example/a" }, { id: "rel", label: "Protocol", url: "//evil.example/a" }],
    respond({ "/host/attention": () => { throw new TypeError("network"); } }),
  );
  assert.deepEqual(snapshot.items, []);
  assert.deepEqual(snapshot.failedSources, ["Host", "Absolute", "Protocol"]);
  assert.deepEqual(requested, ["/host/attention"]);
});

test("a hung source fails at its timeout and delays other sources' items by at most that", async () => {
  const hung = { id: "hung", label: "Hung", url: "/hung/attention" };
  const started = Date.now();
  const snapshot = await loadHostAttention([hung, source], respond({
    // Never answers, and ignores the abort.
    "/hung/attention": () => new Promise<Response>(() => {}),
    "/host/attention": () => json({ items: [item("a")] }),
  }), undefined, 60);
  assert.ok(Date.now() - started < 2000);
  assert.deepEqual(snapshot.items.map((entry) => entry.id), ["a"]);
  assert.deepEqual(snapshot.failedSources, ["Hung"]);
});

test("a 200 answer that is not JSON fails only that source", async () => {
  const other = { id: "other", label: "Other", url: "/other/attention" };
  const snapshot = await loadHostAttention([source, other], respond({
    "/host/attention": () => new Response("<html>sign in</html>", { status: 200, headers: { "content-type": "text/html" } }),
    "/other/attention": () => json({ items: [item("b")] }),
  }));
  assert.deepEqual(snapshot.items.map((entry) => entry.id), ["b"]);
  assert.deepEqual(snapshot.failedSources, ["Host"]);
});

test("an answer over the size cap fails that source, declared or streamed", async () => {
  const big = JSON.stringify({ items: [item("a", { pad: "x".repeat(HOST_ATTENTION_LIMITS.responseBytes) })] });
  const declared = { id: "declared", label: "Declared", url: "/declared/attention" };
  const snapshot = await loadHostAttention([source, declared], respond({
    "/host/attention": () => new Response(big, { status: 200 }),
    // A valid answer: only the declared length can fail it.
    "/declared/attention": () => new Response(JSON.stringify({ items: [] }), { status: 200, headers: { "content-length": String(HOST_ATTENTION_LIMITS.responseBytes + 1) } }),
  }));
  assert.deepEqual(snapshot.items, []);
  assert.deepEqual(snapshot.failedSources, ["Host", "Declared"]);
});

test("an aborted load requests nothing more and fails its sources", async () => {
  const controller = new AbortController();
  controller.abort();
  const snapshot = await loadHostAttention([source], respond({ "/host/attention": () => json({ items: [item("a")] }) }), controller.signal);
  assert.deepEqual(requested, []);
  assert.deepEqual(snapshot.failedSources, ["Host"]);
});

test("an unread body is cancelled: a non-2xx answer and an over-declared one", async () => {
  const cancelled: string[] = [];
  const streamed = (name: string) => new ReadableStream({ cancel() { cancelled.push(name); } });
  const declared = { id: "declared", label: "Declared", url: "/declared/attention" };
  await loadHostAttention([source, declared], respond({
    "/host/attention": () => new Response(streamed("non-2xx"), { status: 503 }),
    "/declared/attention": () => new Response(streamed("declared"), { status: 200, headers: { "content-length": String(HOST_ATTENTION_LIMITS.responseBytes + 1) } }),
  }));
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(cancelled.sort(), ["declared", "non-2xx"]);
});

test("a superseded load is never published", async () => {
  let calls = 0;
  const fetchImpl = ((_url: string, init?: RequestInit) => {
    calls += 1;
    if (calls === 1) {
      // The first load hangs until it is aborted by the newer one.
      return new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      });
    }
    return Promise.resolve(json({ items: [item("a")] }));
  }) as unknown as typeof fetch;
  let visible: () => void = () => {};
  const published: HostAttentionSnapshot[] = [];
  const stop = startHostAttentionPolling([source], (snapshot) => published.push(snapshot), {
    fetch: fetchImpl,
    visible: () => true,
    onVisible: (callback) => { visible = callback; return () => {}; },
    setInterval: () => 0,
    clearInterval: () => {},
  });
  visible();
  await new Promise((resolve) => setTimeout(resolve, 20));
  stop();
  assert.equal(calls, 2);
  assert.deepEqual(published.map((snapshot) => [snapshot.items.map((entry) => entry.id), snapshot.failedSources]), [[["a"], []]]);
});
