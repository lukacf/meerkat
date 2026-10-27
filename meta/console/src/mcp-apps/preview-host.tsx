import React from 'react';
import { createRoot } from 'react-dom/client';
import { ConsoleMcpAppView, ConsoleMcpAppsProvider, type ConsoleMcpAppsHost } from '../../../packages/console-components/src/mcp-apps';
const rpc = async (path: string, body: unknown, signal: AbortSignal) => {
  const response = await fetch(path, { method: 'POST', body: JSON.stringify(body), headers: { 'Content-Type': 'application/json' }, signal });
  if (!response.ok) throw Error(`Fixture request failed: ${response.status}`);
  return response.json();
};
const host: ConsoleMcpAppsHost = {
  sandboxProxyUrl: document.querySelector<HTMLMetaElement>('meta[name="sandbox-url"]')!.content,
  async resolve(locator, signal) {
    const original = await rpc('/resolve', locator, signal);
    return { ...original, readResource: (uri, signal) => rpc('/resources/read', { uri }, signal),
      callTool: (name, args, signal) => rpc('/tools/call', { name, arguments: args }, signal) };
  },
};
createRoot(document.getElementById('root')!).render(<main style={{ maxWidth: 640, margin: '32px auto', font: '15px system-ui' }}>
  <h1>MCP Apps host fixture</h1>
  <p>Operator: Find matching records.</p>
  <ConsoleMcpAppsProvider value={{ host, authority: 'fixture:viewer', readOnly: false }}>
    <ConsoleMcpAppView locator={{ identity: 'member:example', sessionId: 'fixture-session', toolCallId: 'original-call' }} fallback="Seven matching records" />
  </ConsoleMcpAppsProvider>
  <p style={{ color: '#666', fontSize: 12 }}>Protocol fixture. Native member admission and persisted-result integration are tested separately.</p>
</main>);
