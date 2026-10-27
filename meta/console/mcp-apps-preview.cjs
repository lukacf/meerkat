// Deterministic browser fixture for the MCP Apps bridge and sandbox. No provider calls.
const http = require('node:http');
const path = require('node:path');
const { build } = require('esbuild');
const { createSandboxServer } = require('./mcp-apps-sandbox.cjs');
async function main() {
  const options = { bundle: true, write: false, platform: 'browser', format: 'esm', target: 'es2022', nodePaths: [path.join(__dirname, 'node_modules')], jsx: 'automatic' };
  const [host, app] = await Promise.all(['preview-host.tsx', 'preview-app.ts'].map(name => build({ ...options, entryPoints: [path.join(__dirname, 'src/mcp-apps', name)] })));
  const html = '<!doctype html><html><body><script type="module">' + app.outputFiles[0].text.replaceAll('</script', '<\\/script') + '</script></body></html>';
  const result = count => ({ content: [{ type: 'text', text: `${count} matching records` }], structuredContent: { count }, _meta: { viewDetail: 'View-only detail delivered through standard MCP metadata.' } });
  let count = 7, actions = 0, resolves = 0, reads = 0, sandboxUrl;
  const server = http.createServer(async (req, res) => {
    const json = value => { res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); res.end(JSON.stringify(value)); };
    if (req.url === '/') { res.writeHead(200, { 'Content-Type': 'text/html' }); res.end(`<!doctype html><meta name="sandbox-url" content="${sandboxUrl}"><div id="root"></div><script type="module" src="/host.js"></script>`); return; }
    if (req.url === '/host.js') { res.writeHead(200, { 'Content-Type': 'text/javascript' }); res.end(host.outputFiles[0].text); return; }
    if (req.url === '/counts') { json({ originalToolCalls: 1, actions, resolves, reads }); return; }
    let body = ''; for await (const chunk of req) body += chunk;
    const params = JSON.parse(body || '{}');
    if (req.url === '/resolve' && params.toolCallId === 'original-call' && params.identity === 'member:example' && params.sessionId === 'fixture-session') {
      resolves++; json({ tool: { name: 'find', inputSchema: { type: 'object' }, _meta: { ui: { resourceUri: 'ui://example/result' } } }, arguments: { query: 'records' }, result: result(7) }); return;
    }
    if (req.url === '/resources/read' && params.uri === 'ui://example/result') {
      reads++; json({ contents: [{ uri: params.uri, mimeType: 'text/html;profile=mcp-app', text: html }] }); return;
    }
    if (req.url === '/tools/call' && params.name === 'refresh') { actions++; json(result(++count)); return; }
    res.writeHead(403); res.end();
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const hostOrigin = `http://127.0.0.1:${server.address().port}`;
  const sandbox = createSandboxServer({ allowedHostOrigins: [hostOrigin] });
  await new Promise(resolve => sandbox.listen(0, '127.0.0.1', resolve));
  sandboxUrl = `http://127.0.0.1:${sandbox.address().port}/sandbox.html`;
  console.log(JSON.stringify({ hostOrigin, sandboxUrl }));
  process.on('SIGTERM', () => { server.close(); sandbox.close(); process.exit(0); });
}
main().catch(error => { console.error(error); process.exit(1); });
