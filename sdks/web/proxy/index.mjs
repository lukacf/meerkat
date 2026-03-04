/**
 * @rkat/web provider proxy — auth-injecting reverse proxy for LLM providers.
 *
 * Sits between browser WASM clients and LLM provider APIs. Injects real API
 * keys server-side so secrets never reach the browser. Streams request and
 * response bodies without buffering.
 *
 * Usage (standalone):
 *   ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
 *
 * Usage (compose into existing server):
 *   import { createProxyHandler, corsHeaders } from '@rkat/web/proxy';
 *   app.all('/anthropic/*', createProxyHandler('anthropic'));
 */

import { createServer } from 'node:http';

// ─── Provider configuration ─────────────────────────────────────

const PROVIDERS = {
  anthropic: {
    target: 'https://api.anthropic.com',
    envKey: 'ANTHROPIC_API_KEY',
    injectAuth(headers, apiKey) {
      headers.set('x-api-key', apiKey);
      // Remove browser-only header — the proxy is server-side
      headers.delete('anthropic-dangerous-direct-browser-access');
    },
    rewriteUrl(url, _apiKey) { return url; },
  },
  openai: {
    target: 'https://api.openai.com',
    envKey: 'OPENAI_API_KEY',
    injectAuth(headers, apiKey) {
      headers.set('authorization', `Bearer ${apiKey}`);
    },
    rewriteUrl(url, _apiKey) { return url; },
  },
  gemini: {
    target: 'https://generativelanguage.googleapis.com',
    envKey: 'GEMINI_API_KEY',
    injectAuth(headers, apiKey) {
      headers.set('x-goog-api-key', apiKey);
      // Remove any client-side dummy key from query string
    },
    rewriteUrl(url, _apiKey) {
      // Strip any ?key= query param (legacy client-side auth)
      const u = new URL(url);
      u.searchParams.delete('key');
      return u.toString();
    },
  },
};

// ─── CORS headers ───────────────────────────────────────────────

/**
 * Default CORS headers. Override `allowOrigin` for production.
 * @param {string} [allowOrigin='*']
 */
export function corsHeaders(allowOrigin = '*') {
  return {
    'access-control-allow-origin': allowOrigin,
    'access-control-allow-methods': 'GET, POST, PUT, DELETE, OPTIONS',
    'access-control-allow-headers': 'content-type, authorization, x-api-key, anthropic-version, anthropic-beta, anthropic-dangerous-direct-browser-access',
    'access-control-expose-headers': '*',
    'access-control-max-age': '86400',
  };
}

// ─── Proxy handler ──────────────────────────────────────────────

/**
 * Create a proxy handler for a specific provider.
 *
 * Returns a function `(req: Request, pathPrefix: string) => Promise<Response>`
 * compatible with any framework that uses the Web Fetch API Request/Response.
 *
 * @param {keyof PROVIDERS} provider - Provider name ('anthropic', 'openai', 'gemini')
 * @param {{ apiKey?: string, allowOrigin?: string }} [opts]
 */
export function createProxyHandler(provider, opts = {}) {
  const config = PROVIDERS[provider];
  if (!config) throw new Error(`Unknown provider: ${provider}. Use: ${Object.keys(PROVIDERS).join(', ')}`);

  const apiKey = opts.apiKey || process.env[config.envKey];
  if (!apiKey) throw new Error(`Missing API key for ${provider}: set ${config.envKey} env var or pass opts.apiKey`);

  const allowOrigin = opts.allowOrigin || '*';
  const cors = corsHeaders(allowOrigin);

  /**
   * @param {Request} req - Incoming request
   * @param {string} strippedPath - URL path with provider prefix removed (e.g. '/v1/messages')
   */
  return async function proxyHandler(req, strippedPath) {
    // Build target URL
    let targetUrl = `${config.target}${strippedPath}`;
    // Preserve query string
    const qIdx = req.url.indexOf('?');
    if (qIdx !== -1) {
      const qs = req.url.slice(qIdx);
      targetUrl += qs;
    }
    targetUrl = config.rewriteUrl(targetUrl, apiKey);

    // Copy headers, inject auth
    const headers = new Headers();
    for (const [key, value] of req.headers) {
      // Skip hop-by-hop, host, encoding, and browser-origin headers
      if (['host', 'connection', 'keep-alive', 'transfer-encoding', 'accept-encoding', 'origin', 'referer'].includes(key.toLowerCase())) continue;
      headers.set(key, value);
    }
    config.injectAuth(headers, apiKey);

    // Forward request
    const fetchOpts = {
      method: req.method,
      headers,
    };
    // Only include body for methods that have one
    if (req.method !== 'GET' && req.method !== 'HEAD' && req.body) {
      fetchOpts.body = req.body;
      fetchOpts.duplex = 'half';
    }

    const upstream = await fetch(targetUrl, fetchOpts);

    // Build response headers: upstream headers + CORS
    const respHeaders = new Headers();
    for (const [key, value] of upstream.headers) {
      // Strip encoding headers — we don't forward compression negotiation
      if (['content-encoding', 'transfer-encoding'].includes(key.toLowerCase())) continue;
      respHeaders.set(key, value);
    }
    for (const [key, value] of Object.entries(cors)) {
      respHeaders.set(key, value);
    }

    return new Response(upstream.body, {
      status: upstream.status,
      statusText: upstream.statusText,
      headers: respHeaders,
    });
  };
}

// ─── Node.js HTTP server adapter ────────────────────────────────

/**
 * Start a standalone proxy server.
 *
 * @param {{ port?: number, allowOrigin?: string }} [opts]
 */
export function startProxy(opts = {}) {
  const port = opts.port || parseInt(process.env.PORT || '3100', 10);
  const allowOrigin = opts.allowOrigin || '*';
  const cors = corsHeaders(allowOrigin);

  // Build handlers for each provider that has a key configured
  const handlers = {};
  for (const [name, config] of Object.entries(PROVIDERS)) {
    const apiKey = process.env[config.envKey];
    if (apiKey) {
      handlers[name] = createProxyHandler(name, { apiKey, allowOrigin });
    }
  }

  if (Object.keys(handlers).length === 0) {
    console.error('No API keys found. Set at least one of: ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY');
    process.exit(1);
  }

  console.log(`Proxy providers: ${Object.keys(handlers).join(', ')}`);

  const server = createServer(async (nodeReq, nodeRes) => {
    try {
      // CORS preflight
      if (nodeReq.method === 'OPTIONS') {
        nodeRes.writeHead(204, cors);
        nodeRes.end();
        return;
      }

      // Match provider from path: /anthropic/v1/messages → provider='anthropic', rest='/v1/messages'
      const url = new URL(nodeReq.url, `http://localhost:${port}`);
      const match = url.pathname.match(/^\/([^/]+)(\/.*)?$/);
      if (!match || !handlers[match[1]]) {
        nodeRes.writeHead(404, { 'content-type': 'application/json', ...cors });
        nodeRes.end(JSON.stringify({
          error: 'not_found',
          message: `Unknown path. Use: /${Object.keys(handlers).join('/, /')}/`,
        }));
        return;
      }

      const provider = match[1];
      const strippedPath = match[2] || '/';

      // Convert Node.js request to Web Request
      const reqHeaders = new Headers();
      for (let i = 0; i < nodeReq.rawHeaders.length; i += 2) {
        reqHeaders.append(nodeReq.rawHeaders[i], nodeReq.rawHeaders[i + 1]);
      }

      const webReq = new Request(url.toString(), {
        method: nodeReq.method,
        headers: reqHeaders,
        body: nodeReq.method !== 'GET' && nodeReq.method !== 'HEAD'
          ? nodeReq
          : undefined,
        duplex: 'half',
      });

      // Proxy
      const resp = await handlers[provider](webReq, strippedPath);

      // Write response back to Node.js
      const respHeaders = {};
      for (const [key, value] of resp.headers) {
        respHeaders[key] = value;
      }
      nodeRes.writeHead(resp.status, respHeaders);

      if (resp.body) {
        // Stream the response body
        const reader = resp.body.getReader();
        try {
          while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            nodeRes.write(value);
          }
        } finally {
          reader.releaseLock();
        }
      }
      nodeRes.end();
    } catch (err) {
      console.error('Proxy error:', err);
      if (!nodeRes.headersSent) {
        nodeRes.writeHead(502, { 'content-type': 'application/json', ...cors });
        nodeRes.end(JSON.stringify({ error: 'proxy_error', message: String(err.message || err) }));
      } else {
        nodeRes.end();
      }
    }
  });

  server.listen(port, () => {
    console.log(`@rkat/web proxy listening on http://localhost:${port}`);
  });

  return server;
}
