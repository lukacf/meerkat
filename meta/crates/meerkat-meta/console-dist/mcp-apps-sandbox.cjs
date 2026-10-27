// Serve this proxy on a dedicated origin. Never mount it on the Console origin.
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

function domains(value, schemes = ['https:', 'http:']) {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 100) throw Error('Invalid CSP domains');
  return value.map(item => {
    if (typeof item !== 'string' || /[\s;'"\\]/.test(item)) throw Error('Invalid CSP domain');
    const url = new URL(item);
    if (!schemes.includes(url.protocol) || url.username || url.password || url.search || url.hash || !['', '/'].includes(url.pathname)) throw Error('CSP requires allowed origins');
    return url.origin;
  });
}

function appCsp(csp = {}, proxy = false, hostOrigins = []) {
  const safeDomains = (value, schemes) => domains(value, schemes).map(origin => {
    const source = new URL(origin);
    for (const hostOrigin of hostOrigins) {
      const host = new URL(hostOrigin);
      // A CSP HTTP source can also match its HTTPS upgrade. Conservatively
      // reserve every port and transport of Console hostnames for host APIs.
      const sourceName = source.hostname.replace(/\.$/, '');
      const hostName = host.hostname.replace(/\.$/, '');
      const wildcard = sourceName === '*' || (sourceName.startsWith('*.') && hostName.endsWith(sourceName.slice(1)));
      if (sourceName === hostName || wildcard) throw Error('CSP must not grant access to Console hosts');
    }
    return origin;
  });
  const resource = safeDomains(csp.resourceDomains).join(' ');
  const connect = safeDomains(csp.connectDomains, ['https:', 'http:', 'wss:', 'ws:']).join(' ') || "'none'";
  const frames = safeDomains(csp.frameDomains).join(' ');
  const base = safeDomains(csp.baseUriDomains).join(' ') || "'self'";
  return ["default-src 'none'", `script-src 'self' 'unsafe-inline' ${resource}`,
    `style-src 'self' 'unsafe-inline' ${resource}`, `img-src 'self' data: ${resource}`,
    `media-src 'self' data: ${resource}`, `font-src 'self' ${resource}`, `connect-src ${connect}`,
    `frame-src ${proxy ? "'self' " : ''}${frames || (proxy ? '' : "'none'")}`,
    `base-uri ${base}`, "object-src 'none'", "form-action 'none'", "webrtc 'block'"].join('; ');
}

function createSandboxServer({ allowedHostOrigins }) {
  const allowed = new Set(domains(allowedHostOrigins));
  if (!allowed.size || [...allowed].some(value => new URL(value).hostname.includes('*'))) throw Error('Configure exact Console origins');
  return http.createServer((req, res) => {
    try {
      const url = new URL(req.url, 'http://sandbox.invalid');
      if (url.pathname !== '/sandbox.html') { res.writeHead(404); res.end(); return; }
      const hostOrigin = url.searchParams.get('hostOrigin');
      if (!allowed.has(hostOrigin)) throw Error('Host origin is not allowed');
      const csp = JSON.parse(url.searchParams.get('csp') || '{}');
      const innerCsp = appCsp(csp, false, [...allowed]);
      const script = fs.readFileSync(path.join(__dirname, 'src/mcp-apps/sandbox.js'), 'utf8');
      const bootstrap = JSON.stringify({ hostOrigin, innerCsp }).replaceAll('<', '\\u003c');
      res.writeHead(200, {
        'Content-Type': 'text/html; charset=utf-8',
        'Content-Security-Policy': `${appCsp(csp, true, [...allowed])}; frame-ancestors ${hostOrigin}`,
        'Permissions-Policy': 'camera=(), microphone=(), geolocation=(), clipboard-write=()',
        'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff',
        'Referrer-Policy': 'no-referrer',
      });
      res.end(`<!doctype html><html><head><meta charset="utf-8"><style>html,body,iframe{margin:0;width:100%;height:100%;border:0}</style></head><body><script>const config=${bootstrap};${script}</script></body></html>`);
    } catch { res.writeHead(400); res.end('Invalid sandbox request'); }
  });
}
module.exports = { appCsp, createSandboxServer };
if (require.main === module) {
  const origins = process.env.MCP_APPS_HOST_ORIGINS || process.env.MCP_APPS_HOST_ORIGIN;
  if (!origins) throw Error('Set MCP_APPS_HOST_ORIGIN to the Console origin');
  const allowedHostOrigins = origins.split(',').map(value => value.trim());
  const server = createSandboxServer({ allowedHostOrigins });
  const port = Number(process.env.MCP_APPS_SANDBOX_PORT || 8081);
  const bind = process.env.MCP_APPS_SANDBOX_HOST || '127.0.0.1';
  server.listen(port, bind, () => console.log(JSON.stringify({ sandbox: `http://${bind}:${server.address().port}/sandbox.html`, allowedHostOrigins })));
  for (const event of ['SIGINT', 'SIGTERM']) process.on(event, () => server.close());
}
