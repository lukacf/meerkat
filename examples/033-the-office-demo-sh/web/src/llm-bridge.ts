/**
 * Network Bridge for meerkat-web-runtime WASM.
 *
 * WASM cannot make HTTP requests directly. This bridge is a transparent
 * network proxy — it receives a fully-formed HTTP request from the WASM
 * runtime and executes it via browser fetch. Zero provider knowledge.
 *
 * The WASM runtime (which contains meerkat's provider logic) builds the
 * correct URL, headers, and body for each provider. This bridge just
 * forwards the request and returns the raw response.
 *
 * Bridge contract v1:
 *   js_llm_call(url, method, headers_json, body, timeout_ms) → response JSON string
 *   Response: { "status": N, "headers": {...}, "body": "..." }
 *   Error: rejects with string: { "code": "...", "message": "..." }
 */

export interface BridgeConfig {
  /** Optional: inject extra headers on every request (e.g. auth tokens from app layer) */
  extraHeaders?: () => Record<string, string>;
}

let config: BridgeConfig = {};

export function setupBridge(cfg?: BridgeConfig): void {
  config = cfg ?? {};

  (globalThis as Record<string, unknown>).js_http_fetch = async (
    url: string,
    method: string,
    headersJson: string,
    body: string,
    timeoutMs: number,
  ): Promise<string> => {
    try {
      const headers: Record<string, string> = JSON.parse(headersJson || "{}");

      // Merge app-layer headers (e.g. API keys injected at runtime)
      const extra = config.extraHeaders?.() ?? {};
      for (const [k, v] of Object.entries(extra)) {
        if (!(k in headers)) headers[k] = v;
      }

      const controller = new AbortController();
      const timer = timeoutMs > 0 ? setTimeout(() => controller.abort(), timeoutMs) : null;

      const res = await fetch(url, {
        method,
        headers,
        body: method !== "GET" && body ? body : undefined,
        signal: controller.signal,
      });

      if (timer) clearTimeout(timer);

      const responseBody = await res.text();
      const responseHeaders: Record<string, string> = {};
      res.headers.forEach((v, k) => { responseHeaders[k] = v; });

      return JSON.stringify({
        status: res.status,
        headers: responseHeaders,
        body: responseBody,
      });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      const code = msg.includes("abort") ? "timeout" : "network_error";
      throw JSON.stringify({ code, message: msg });
    }
  };
}
