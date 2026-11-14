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
 * Usage (compose into your own server): the handler takes a Fetch API
 * Request and the path with the provider prefix removed, and returns a
 * Response. It compares the request's Origin with `allowOrigin` and does not
 * authenticate callers; see `createProxyHandler` below.
 *   import { createProxyHandler } from '@rkat/web/proxy';
 *   const anthropic = createProxyHandler('anthropic', { allowOrigin: 'https://app.example.com' });
 *   const response = await anthropic(request, '/v1/messages');
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
 * CORS response headers that allow one origin value (`*` allows any).
 * @param {string} [allowOrigin='*']
 */
export function corsHeaders(allowOrigin = '*') {
  return {
    'access-control-allow-origin': allowOrigin,
    'access-control-allow-methods': 'GET, POST, PUT, DELETE, OPTIONS',
    'access-control-allow-headers': 'content-type, authorization, x-api-key, x-goog-api-key, anthropic-version, anthropic-beta, anthropic-dangerous-direct-browser-access',
    'access-control-expose-headers': '*',
    'access-control-max-age': '86400',
  };
}

// `localhost`, an address of 127.0.0.0/8, or `[::1]`.
const LOOPBACK_NAME = String.raw`(localhost|127(\.\d{1,3}){3}|\[::1\])`;
const LOOPBACK_ORIGIN = new RegExp(`^https?://${LOOPBACK_NAME}(:\\d+)?$`);
const LOOPBACK_HOST = new RegExp(`^${LOOPBACK_NAME}(:\\d+)?$`, 'i');

/**
 * Decide a request by its Origin header.
 *
 * `allowOrigin` names the one origin a request may come from, or is `*` for
 * any origin. Without it only loopback origins are allowed: `localhost`, a
 * 127.0.0.0/8 address or `[::1]`, http or https, any port. An Origin of
 * `null` is allowed by `*` only. A request without an Origin header is
 * allowed whatever the setting, except one a browser marks
 * `Sec-Fetch-Site: cross-site` (a browser sends no Origin on some GETs),
 * which only `*` allows. A caller that is not a browser sends neither header.
 *
 * @param {string | undefined} allowOrigin
 * @param {string | null | undefined} origin - The request's Origin header
 * @param {string | null | undefined} fetchSite - The request's Sec-Fetch-Site header
 * @returns {{ allowed: boolean, headers: Record<string, string> }} The CORS
 *   headers of the answer; a refused request gets no allow header.
 */
function checkOrigin(allowOrigin, origin, fetchSite) {
  if (allowOrigin === '*') return { allowed: true, headers: corsHeaders('*') };
  if (!origin && typeof fetchSite === 'string' && fetchSite.trim().toLowerCase() === 'cross-site') {
    return { allowed: false, headers: {} };
  }
  if (allowOrigin) {
    const allowed = !origin || (origin === allowOrigin && origin !== 'null');
    return { allowed, headers: allowed ? corsHeaders(allowOrigin) : {} };
  }
  if (!origin) return { allowed: true, headers: { vary: 'origin' } };
  if (!LOOPBACK_ORIGIN.test(origin)) return { allowed: false, headers: { vary: 'origin' } };
  return { allowed: true, headers: { ...corsHeaders(origin), vary: 'origin' } };
}

function isLoopbackHost(host) {
  return typeof host === 'string' && LOOPBACK_HOST.test(host);
}

const REFUSED = {
  origin: JSON.stringify({ error: 'origin_not_allowed', message: 'This origin is not allowed to call the proxy. The allowed origin is set with --cors-origin (allowOrigin).' }),
  host: JSON.stringify({ error: 'host_not_allowed', message: 'The proxy answers loopback host names only.' }),
  hosts: JSON.stringify({ error: 'bad_request', message: 'A request carries exactly one Host header.' }),
};

// ─── Proxy handler ──────────────────────────────────────────────

/**
 * Create a proxy handler for a specific provider.
 *
 * Returns a function `(req: Request, pathPrefix: string) => Promise<Response>`
 * compatible with any framework that uses the Web Fetch API Request/Response.
 *
 * The handler compares a request's Origin header with `allowOrigin` before it
 * calls the provider, and answers 403 when the origin is not allowed (see
 * `checkOrigin` above: loopback origins only when `allowOrigin` is not given,
 * one named origin, or `*` for any). `allowOrigin` is one origin as a string:
 * a list, a pattern or a function allows no origin. A page of the embedding
 * site itself sends an Origin on its POSTs, so name the site's origin. The
 * handler answers an allowed preflight itself.
 * It does not check who calls it in any other way: a request without an
 * Origin header is forwarded unless a browser marks it cross-site, so a
 * server that embeds the handler puts its own authentication in front of it.
 *
 * `loopbackHostOnly` makes the handler refuse (403) a request whose Host
 * header is not a loopback name or address. It is off by default, since the
 * embedding server owns its bind address and host names; turn it on for a
 * server that listens on a loopback address.
 *
 * @param {keyof PROVIDERS} provider - Provider name ('anthropic', 'openai', 'gemini')
 * @param {{ apiKey?: string, allowOrigin?: string, loopbackHostOnly?: boolean }} [opts]
 */
export function createProxyHandler(provider, opts = {}) {
  const config = PROVIDERS[provider];
  if (!config) throw new Error(`Unknown provider: ${provider}. Use: ${Object.keys(PROVIDERS).join(', ')}`);

  const apiKey = opts.apiKey || process.env[config.envKey];
  if (!apiKey) throw new Error(`Missing API key for ${provider}: set ${config.envKey} env var or pass opts.apiKey`);

  const { allowOrigin, loopbackHostOnly = false } = opts;

  /**
   * @param {Request} req - Incoming request
   * @param {string} strippedPath - URL path with provider prefix removed (e.g. '/v1/messages')
   */
  return async function proxyHandler(req, strippedPath) {
    // Refuse before anything is sent to the provider.
    const json = { 'content-type': 'application/json' };
    if (loopbackHostOnly && !isLoopbackHost(req.headers.get('host') ?? new URL(req.url).host)) {
      return new Response(REFUSED.host, { status: 403, headers: json });
    }
    const origin = checkOrigin(allowOrigin, req.headers.get('origin'), req.headers.get('sec-fetch-site'));
    if (!origin.allowed) {
      return new Response(REFUSED.origin, { status: 403, headers: { ...json, ...origin.headers } });
    }
    if (req.method === 'OPTIONS') {
      return new Response(null, { status: 204, headers: origin.headers });
    }

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
      const name = key.toLowerCase();
      // Strip encoding headers — we don't forward compression negotiation
      if (['content-encoding', 'transfer-encoding'].includes(name)) continue;
      // The proxy answers for its own allowed origins, not the provider's
      if (name.startsWith('access-control-')) continue;
      respHeaders.set(key, value);
    }
    for (const [key, value] of Object.entries(origin.headers)) {
      if (key === 'vary') respHeaders.append(key, value);
      else respHeaders.set(key, value);
    }

    return new Response(upstream.body, {
      status: upstream.status,
      statusText: upstream.statusText,
      headers: respHeaders,
    });
  };
}

// ─── Node.js HTTP server adapter ────────────────────────────────

const DEFAULT_HOST = '127.0.0.1';

function isLoopbackAddress(address) {
  return address === '::1' || /^(::ffff:)?127\./i.test(address);
}

// A port given as an integer or as a string of digits; 0 asks the system for
// a free port. Anything else leaves the choice to PORT or the default.
function portOption(value) {
  if (Number.isInteger(value)) return value;
  if (typeof value === 'string' && /^\d+$/.test(value.trim())) return Number(value.trim());
  return undefined;
}

/**
 * Start a standalone proxy server.
 *
 * Listens on the loopback interface (127.0.0.1) unless `host` names another
 * address. Every user and process of the machine reaches a loopback address,
 * and the proxy does not check who calls it: it only compares a request's
 * Origin header with `allowOrigin` (loopback origins when not given, see
 * `createProxyHandler`) and, while it is bound to a loopback address, refuses
 * a request whose Host header is not a loopback name or address. Bound to
 * another address it does not check the Host, and forwards any caller on the
 * network that sends no Origin header: put it behind your own authentication.
 *
 * @param {{ port?: number | string, host?: string, allowOrigin?: string }} [opts]
 */
export function startProxy(opts = {}) {
  const port = portOption(opts.port) ?? parseInt(process.env.PORT || '3100', 10);
  const host = opts.host || DEFAULT_HOST;
  const allowOrigin = opts.allowOrigin;

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
    let cors = {};
    try {
      // Refuse by Host and Origin first, whatever the method and path
      const json = { 'content-type': 'application/json' };
      const hostHeaders = nodeReq.rawHeaders.filter((name, i) => i % 2 === 0 && name.toLowerCase() === 'host');
      if (hostHeaders.length > 1) {
        nodeRes.writeHead(400, json);
        nodeRes.end(REFUSED.hosts);
        return;
      }
      if (isLoopbackAddress(server.address().address) && !isLoopbackHost(nodeReq.headers.host)) {
        nodeRes.writeHead(403, json);
        nodeRes.end(REFUSED.host);
        return;
      }
      const origin = checkOrigin(allowOrigin, nodeReq.headers.origin, nodeReq.headers['sec-fetch-site']);
      if (!origin.allowed) {
        nodeRes.writeHead(403, { ...json, ...origin.headers });
        nodeRes.end(REFUSED.origin);
        return;
      }
      cors = origin.headers;

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

  server.listen(port, host, () => {
    const bound = server.address();
    const shown = bound.address.includes(':') ? `[${bound.address}]` : bound.address;
    console.log(`@rkat/web proxy listening on http://${shown}:${bound.port}`);
    if (!isLoopbackAddress(bound.address)) {
      console.warn(`Warning: ${bound.address} is not a loopback address. The proxy does not check who calls it: put it behind your own authentication.`);
    }
  });

  return server;
}
