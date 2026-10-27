const assert = require('node:assert/strict');
const http = require('node:http');
const path = require('node:path');
const { test } = require('node:test');
const { build } = require('esbuild');
const { chromium } = require('playwright');
const { createSandboxServer } = require('./mcp-apps-sandbox.cjs');

async function listen(server) {
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  return `http://127.0.0.1:${server.address().port}`;
}
async function close(server) {
  server?.closeAllConnections();
  if (server?.listening) await new Promise(resolve => server.close(resolve));
}

async function checkNavigationRetirement(shadowParent) {
  let browser, host, sandbox, replacement;
  try {
    replacement = http.createServer((_req, res) => {
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end(`<script>parent.postMessage({jsonrpc:'2.0',method:'tools/call',id:9,params:{name:'replacement'}}, '*')</script>`);
    });
    const replacementOrigin = await listen(replacement);
    const appHtml = `<!doctype html><button id="navigate">Navigate</button><script>
      document.getElementById('navigate').onclick = () => location.href = ${JSON.stringify(replacementOrigin)};
      parent.postMessage({jsonrpc:'2.0',method:'ping',id:1}, '*');
      ${shadowParent ? "Object.defineProperty(window, 'parent', { value: {postMessage(){}} });" : ''}
    </script>`;
    let script, sandboxUrl;
    host = http.createServer((req, res) => {
      if (req.url === '/host.js') { res.writeHead(200, { 'content-type': 'text/javascript' }); res.end(script); return; }
      res.writeHead(200, { 'content-type': 'text/html' });
      res.end('<!doctype html><iframe sandbox="allow-scripts allow-same-origin"></iframe><script type="module" src="/host.js"></script>');
    });
    // Different hostnames let the test allow navigation without granting any
    // CSP access to the Console hostname itself.
    const hostOrigin = (await listen(host)).replace('127.0.0.1', 'localhost');
    sandbox = createSandboxServer({ allowedHostOrigins: [hostOrigin] });
    sandboxUrl = new URL('/sandbox.html', await listen(sandbox));
    sandboxUrl.searchParams.set('hostOrigin', hostOrigin);
    sandboxUrl.searchParams.set('csp', JSON.stringify({ frameDomains: [replacementOrigin] }));
    const bundle = await build({
      stdin: { contents: `
        import { McpAppTransport } from '../packages/console-components/src/mcp-apps.tsx';
        const frame = document.querySelector('iframe');
        window.evidence = { retirements: 0, ready: false, calls: [], callsAtRetirement: null, lateProbeReceived: false };
        const transport = new McpAppTransport(frame.contentWindow, ${JSON.stringify(sandboxUrl.origin)}, () => {
          ++window.evidence.retirements;
          window.evidence.callsAtRetirement = [...window.evidence.calls];
          transport.beginTeardown();
        });
        // Independently prove the late probe reached the browser from the same
        // source and origin; the transport must refuse it during teardown.
        window.addEventListener('message', event => {
          if (event.source === frame.contentWindow && event.origin === ${JSON.stringify(sandboxUrl.origin)} && event.data?.id === 90) {
            window.evidence.lateProbeReceived = true;
          }
        });
        transport.onmessage = message => {
          if (message.method === 'ui/notifications/sandbox-proxy-ready') {
            void transport.send({jsonrpc:'2.0', method:'ui/notifications/sandbox-resource-ready', params:{html:${JSON.stringify(appHtml)}}});
          }
          if (message.method === 'ping') window.evidence.ready = true;
          if (message.method === 'tools/call') window.evidence.calls.push(message.params.name);
        };
        void transport.start();
        frame.src = ${JSON.stringify(sandboxUrl.href)};
      `, resolveDir: __dirname, loader: 'tsx' },
      bundle: true, write: false, format: 'esm', platform: 'browser', jsx: 'automatic',
      nodePaths: [path.join(__dirname, 'node_modules')],
    });
    script = bundle.outputFiles[0].text;
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.goto(hostOrigin);
    await page.waitForFunction(() => window.evidence?.ready);
    const proxy = page.frames().find(frame => frame.url() === sandboxUrl.href);
    assert.ok(proxy);
    const app = page.frameLocator('iframe').frameLocator('iframe');
    await app.locator('#navigate').click();
    await page.waitForFunction(() => window.evidence?.retirements === 1, undefined, { timeout: 5_000 });
    await page.frameLocator('iframe').locator('iframe').waitFor({ state: 'detached' });
    // Navigation events and postMessage delivery are asynchronous. A replacement
    // script can race the early notice, so do not claim that no pre-retirement
    // message can arrive. Once retired, even the exact proxy cannot reopen work.
    await proxy.evaluate(origin => {
      parent.postMessage({ jsonrpc: '2.0', method: 'tools/call', id: 90, params: { name: 'late' } }, origin);
    }, hostOrigin);
    await page.waitForFunction(() => window.evidence.lateProbeReceived, undefined, { timeout: 5_000 });
    const evidence = await page.evaluate(() => window.evidence);
    assert.equal(evidence.retirements, 1);
    assert.deepEqual(evidence.calls, evidence.callsAtRetirement);
    assert.ok(evidence.calls.every(name => name === 'replacement'));
  } finally {
    await browser?.close();
    await close(host);
    await close(sandbox);
    await close(replacement);
  }
}

test('navigation retires the real sandbox and rejects later bridge messages', { timeout: 15_000 }, async () => {
  await checkNavigationRetirement(false);
});

test('parent shadowing cannot prevent bounded retirement or reopen the bridge', { timeout: 15_000 }, async () => {
  await checkNavigationRetirement(true);
});
