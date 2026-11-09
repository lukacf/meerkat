const assert = require('node:assert/strict');
const { test } = require('node:test');
const { appCsp, createSandboxServer } = require('./mcp-apps-sandbox.cjs');
test('CSP defaults deny network and undeclared nested frames', () => {
  const policy = appCsp();
  assert.match(policy, /connect-src 'none'/);
  assert.match(policy, /frame-src 'none'/);
  assert.match(policy, /object-src 'none'/);
  assert.match(policy, /form-action 'none'/);
  assert.match(policy, /webrtc 'block'/);
  assert.match(appCsp({ connectDomains: ['https://api.example.test'] }), /connect-src https:\/\/api.example.test/);
  assert.match(appCsp({ connectDomains: ['wss://live.example.test', 'ws://localhost:8080'] }), /connect-src wss:\/\/live.example.test ws:\/\/localhost:8080/);
  assert.match(appCsp({ resourceDomains: ['https://*.example.test'] }), /https:\/\/\*\.example.test/);
  assert.throws(() => appCsp({ resourceDomains: ['wss://live.example.test'] }));
  for (const domain of ['*', 'https://example.test; script-src *', 'https://user@example.test', 'javascript:alert(1)', 'https://example.test/path']) {
    assert.throws(() => appCsp({ connectDomains: [domain] }));
  }
});
test('declared CSP cannot cover any configured Console hostname', () => {
  const hosts = ['https://console.example.test', 'http://admin.example.test:8000'];
  for (const field of ['connectDomains', 'resourceDomains', 'frameDomains', 'baseUriDomains']) {
    for (const domain of ['https://console.example.test', 'http://console.example.test:5000', 'https://console.example.test.', 'https://*.example.test', 'https://*', 'https://admin.example.test']) {
      assert.throws(() => appCsp({ [field]: [domain] }, false, hosts), /Console hosts/);
    }
  }
  for (const domain of ['ws://console.example.test', 'wss://console.example.test:5000']) {
    assert.throws(() => appCsp({ connectDomains: [domain] }, true, hosts), /Console hosts/);
  }
  assert.match(appCsp({ resourceDomains: ['https://assets.example.test'] }, false, hosts), /assets.example.test/);
});
test('sandbox response applies policy in HTTP headers and rejects unapproved parents', async () => {
  const server = createSandboxServer({ allowedHostOrigins: ['https://console.example.test'] });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    const root = `http://127.0.0.1:${server.address().port}/sandbox.html`;
    const valid = await fetch(root + '?hostOrigin=' + encodeURIComponent('https://console.example.test'));
    assert.equal(valid.status, 200);
    assert.match(valid.headers.get('content-security-policy'), /frame-ancestors https:\/\/console.example.test/);
    assert.match(valid.headers.get('content-security-policy'), /connect-src 'none'/);
    assert.equal(valid.headers.get('cache-control'), 'no-store');
    assert.equal((await fetch(root + '?hostOrigin=https://other.test')).status, 400);
    assert.equal((await fetch(root.replace('/sandbox.html', '/console/mcp-apps/resolve'))).status, 404);
    const bad = new URL(root); bad.searchParams.set('hostOrigin', 'https://console.example.test'); bad.searchParams.set('csp', JSON.stringify({ resourceDomains: ["https://x.test; default-src *"] }));
    assert.equal((await fetch(bad)).status, 400);
    bad.searchParams.set('csp', JSON.stringify({ connectDomains: ['wss://console.example.test'] }));
    assert.equal((await fetch(bad)).status, 400);
  } finally { await new Promise(resolve => server.close(resolve)); }
});
test('sandbox configuration accepts only exact HTTP Console origins', () => {
  for (const allowedHostOrigins of [[], ['*'], ['https://*.example.test'], ['file:///tmp'], ['https://console.test/private']]) {
    assert.throws(() => createSandboxServer({ allowedHostOrigins }));
  }
});
