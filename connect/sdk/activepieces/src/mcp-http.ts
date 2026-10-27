import { Agent, request, type Dispatcher } from "undici";
import type { Readable } from "node:stream";
import { finished } from "node:stream/promises";
import type { HostTransport } from "./index.js";

export interface OwnedHttpTransport {
  readonly send: HostTransport;
  /** Called after all reader invocations have settled and released their bodies. */
  close(): Promise<void>;
}
function responseStream(body: Readable): ReadableStream<Uint8Array> {
  const iterator = body[Symbol.asyncIterator]();
  const settled = finished(body, { cleanup: true }).catch(() => {});
  let cancelled = false;
  let pulling: Promise<void> | undefined;
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      return pulling = (async () => {
        try {
          const chunk = await iterator.next();
          if (cancelled) return;
          if (chunk.done) { await settled; if (!cancelled) controller.close(); }
          else if (chunk.value instanceof Uint8Array) controller.enqueue(chunk.value);
          else throw new Error("HTTP response failed");
        } catch {
          body.destroy();
          await settled;
          if (!cancelled) controller.error(new Error("HTTP response failed"));
        }
      })();
    },
    async cancel() {
      cancelled = true;
      // Destroy first to unblock any pending iterator read. Never enqueue after
      // cancellation, and retain ownership until the Node stream has closed.
      body.destroy();
      await settled;
      await pulling;
    },
  });
}
/** The optional dispatcher is transferred to this owner; intended for fixture/host injection. */
export function createOwnedHttpTransport(dispatcher: Dispatcher = new Agent({
  connections: 4, pipelining: 0, allowH2: false, maxOrigins: 1,
  connect: { rejectUnauthorized: true },
})): OwnedHttpTransport {
  let closing: Promise<void> | undefined;
  return {
    async send(url, init) {
      const method = init.method;
      if (closing || (method !== "GET" && method !== "POST") || init.redirect !== "error" || !init.signal ||
        (method === "GET" ? init.body !== undefined : typeof init.body !== "string" || new TextEncoder().encode(init.body).byteLength > 131072 || !["application/json", "application/x-www-form-urlencoded"].includes(new Headers(init.headers).get("content-type") ?? ""))) throw new Error("HTTP transport refused");
      const response = await request(url, {
        dispatcher, method, headers: Object.fromEntries(new Headers(init.headers)),
        ...(typeof init.body === "string" ? { body: init.body } : {}),
        // Raw request has no redirect/retry interceptor. Reader rejects 3xx.
        signal: init.signal, idempotent: false,
        headersTimeout: 60_000, bodyTimeout: 60_000,
      });
      try {
        const headers = new Headers();
        for (const [name, value] of Object.entries(response.headers)) {
          for (const item of Array.isArray(value) ? value : value === undefined ? [] : [value]) headers.append(name, item);
        }
        if ([204, 205, 304].includes(response.statusCode)) {
          response.body.resume();
          await finished(response.body);
          return new Response(null, { status: response.statusCode, headers });
        }
        return new Response(responseStream(response.body), { status: response.statusCode, headers });
      } catch {
        response.body.destroy();
        await finished(response.body).catch(() => {});
        throw new Error("HTTP response failed");
      }
    },
    close() { return closing ??= dispatcher.close(); },
  };
}
