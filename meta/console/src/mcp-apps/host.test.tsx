import React from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { ConsoleMcpAppView, ConsoleMcpAppsProvider, McpAppTransport, mcpSandboxUrl, type ConsoleMcpAppSession } from '../../../packages/console-components/src/mcp-apps';

const locator = { identity: 'member:one', sessionId: 'session:one', toolCallId: 'call:one' };
const session = (): ConsoleMcpAppSession => ({
  tool: { name: 'find', inputSchema: { type: 'object' }, _meta: { ui: { resourceUri: 'ui://example/result' } } },
  arguments: { query: 'test' },
  result: { content: [{ type: 'text', text: 'Seven results' }], structuredContent: { count: 7 }, _meta: { privateViewField: 'detail' } },
  readResource: vi.fn(async () => ({ contents: [{ uri: 'ui://example/result', mimeType: 'text/html;profile=mcp-app', text: '<!doctype html><html><body>App</body></html>' }] })),
  callTool: vi.fn(async () => ({ content: [{ type: 'text', text: 'Updated' }] })),
  dispose: vi.fn(),
});
function show(resolve, readOnly = false, authority = 'scope:one') {
  return <ConsoleMcpAppsProvider value={{ host: { sandboxProxyUrl: 'https://sandbox.test/sandbox.html', resolve }, readOnly, authority }}>
    <ConsoleMcpAppView locator={locator} fallback="Seven results" />
  </ConsoleMcpAppsProvider>;
}
function message(frame: HTMLIFrameElement, method: string, params: unknown, id?: number) {
  window.dispatchEvent(new MessageEvent('message', { source: frame.contentWindow, origin: 'https://sandbox.test', data: { jsonrpc: '2.0', method, params, ...(id === undefined ? {} : { id }) } }));
}
async function initialize(frame: HTMLIFrameElement, sent) {
  await act(async () => message(frame, 'ui/notifications/sandbox-proxy-ready', {}));
  await waitFor(() => expect(sent.mock.calls.some(([m]) => m.method === 'ui/notifications/sandbox-resource-ready')).toBe(true));
  await act(async () => message(frame, 'ui/initialize', { protocolVersion: '2026-01-26', appInfo: { name: 'Example', version: '1.0.0' }, appCapabilities: { availableDisplayModes: ['inline'] } }, 1));
  await waitFor(() => expect(sent.mock.calls.some(([m]) => m.id === 1 && m.result)).toBe(true));
  await act(async () => message(frame, 'ui/notifications/initialized', {}));
}

describe('standard MCP Apps host', () => {
  it('replays the original input and full result after the real SDK handshake, then proxies actions once', async () => {
    const binding = session();
    const resolve = vi.fn(async () => binding);
    const view = render(show(resolve));
    const frame = screen.getByTitle('Interactive tool result') as HTMLIFrameElement;
    await waitFor(() => expect(frame.src).toContain('sandbox.test'));
    const sent = vi.spyOn(frame.contentWindow!, 'postMessage');
    await initialize(frame, sent);
    await waitFor(() => expect(sent.mock.calls.some(([m]) => m.method === 'ui/notifications/tool-result')).toBe(true));
    const notifications = sent.mock.calls.map(([m]) => m).filter(m => m.method?.startsWith('ui/notifications/tool-'));
    expect(notifications.map(m => m.method)).toEqual(['ui/notifications/tool-input', 'ui/notifications/tool-result']);
    expect(notifications[1].params).toEqual(binding.result);
    expect(resolve).toHaveBeenCalledWith(locator, expect.any(AbortSignal));
    expect(binding.callTool).not.toHaveBeenCalled();
    await act(async () => message(frame, 'tools/call', { name: 'refresh', arguments: { page: 2 } }, 2));
    await waitFor(() => expect(sent.mock.calls.find(([m]) => m.id === 2)?.[0]).toMatchObject({ result: { content: [{ type: 'text', text: 'Updated' }] } }));
    expect(binding.callTool).toHaveBeenCalledTimes(1);
    expect(binding.callTool).toHaveBeenCalledWith('refresh', { page: 2 }, expect.any(AbortSignal));
    view.unmount();
    expect(binding.dispose).toHaveBeenCalledTimes(1);
  });

  it('keeps text fallback for missing UI resources without invoking a tool', async () => {
    const binding = session(); binding.readResource = vi.fn(async () => ({ contents: [{ uri: 'ui://example/result', mimeType: 'text/plain', text: 'Wrong type' }] }));
    render(show(async () => binding));
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Seven results'));
    await waitFor(() => expect(binding.dispose).toHaveBeenCalledTimes(1));
    expect(screen.queryByTitle('Interactive tool result')).not.toBeInTheDocument();
    expect(binding.callTool).not.toHaveBeenCalled();
  });

  it('rejects app actions when the console is view-only', async () => {
    const binding = session();
    render(show(async () => binding, true));
    const frame = screen.getByTitle('Interactive tool result') as HTMLIFrameElement;
    await waitFor(() => expect(frame.src).toContain('sandbox.test'));
    const sent = vi.spyOn(frame.contentWindow!, 'postMessage');
    await initialize(frame, sent);
    await act(async () => message(frame, 'tools/call', { name: 'refresh', arguments: {} }, 2));
    await waitFor(() => expect(sent.mock.calls.some(([m]) => m.id === 2 && m.error)).toBe(true));
    expect(binding.callTool).not.toHaveBeenCalled();
  });

  it('disposes late resolved bindings after unmount', async () => {
    let finish: (value: ConsoleMcpAppSession) => void;
    const resolve = vi.fn(() => new Promise<ConsoleMcpAppSession>(done => { finish = done; }));
    const view = render(show(resolve));
    view.unmount();
    const binding = session();
    await act(async () => finish(binding));
    expect(binding.dispose).toHaveBeenCalledTimes(1);
    expect(binding.readResource).not.toHaveBeenCalled();
    expect(resolve.mock.calls[0][1].aborted).toBe(true);
  });

  it('retires a navigated app and rejects its replacement document requests', async () => {
    const binding = session();
    const view = render(show(async () => binding));
    const frame = screen.getByTitle('Interactive tool result') as HTMLIFrameElement;
    await waitFor(() => expect(frame.src).toContain('sandbox.test'));
    const sent = vi.spyOn(frame.contentWindow!, 'postMessage');
    await initialize(frame, sent);
    await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
    const data = { type: 'console-mcp-app-retired' };
    await act(async () => {
      window.dispatchEvent(new MessageEvent('message', { source: window, origin: 'https://sandbox.test', data }));
      window.dispatchEvent(new MessageEvent('message', { source: frame.contentWindow, origin: 'https://other.test', data }));
    });
    expect(binding.dispose).not.toHaveBeenCalled();
    await act(async () => {
      window.dispatchEvent(new MessageEvent('message', { source: frame.contentWindow, origin: 'https://sandbox.test', data }));
      message(frame, 'tools/call', { name: 'refresh' }, 5);
    });
    expect(binding.dispose).toHaveBeenCalledTimes(1);
    expect(binding.callTool).not.toHaveBeenCalled();
    expect(screen.getByRole('status')).toHaveTextContent('Seven results');
    expect(frame).not.toBeVisible();
    view.unmount();
  });

  it('aborts an in-flight widget action on unmount and never replays it', async () => {
    const binding = session();
    let finish: (value: any) => void;
    let actionSignal: AbortSignal;
    binding.callTool = vi.fn((_name, _args, signal) => {
      actionSignal = signal;
      return new Promise(done => { finish = done; });
    });
    const view = render(show(async () => binding));
    const frame = screen.getByTitle('Interactive tool result') as HTMLIFrameElement;
    await waitFor(() => expect(frame.src).toContain('sandbox.test'));
    const sent = vi.spyOn(frame.contentWindow!, 'postMessage');
    await initialize(frame, sent);
    await act(async () => message(frame, 'tools/call', { name: 'refresh', arguments: {} }, 2));
    await waitFor(() => expect(binding.callTool).toHaveBeenCalledTimes(1));
    view.unmount();
    expect(actionSignal!.aborted).toBe(true);
    await act(async () => finish!({ content: [{ type: 'text', text: 'Late private value' }] }));
    expect(binding.callTool).toHaveBeenCalledTimes(1);
    expect(sent.mock.calls.some(([m]) => m.id === 2 && m.result)).toBe(false);
  });

  it('isolates a new authority from the retiring app until teardown is acknowledged', async () => {
    const first = session();
    const second = session();
    second.result = { content: [{ type: 'text', text: 'Only the second app may receive this' }] };
    second.readResource = vi.fn(async () => ({ contents: [{ uri: 'ui://example/result', mimeType: 'text/html;profile=mcp-app', text: '<html>Second app</html>' }] }));
    const view = render(show(async () => first));
    const oldFrame = screen.getByTitle('Interactive tool result') as HTMLIFrameElement;
    await waitFor(() => expect(oldFrame.src).toContain('sandbox.test'));
    const oldSent = vi.spyOn(oldFrame.contentWindow!, 'postMessage');
    await initialize(oldFrame, oldSent);
    await waitFor(() => expect(oldSent.mock.calls.some(([m]) => m.method === 'ui/notifications/tool-result')).toBe(true));
    oldSent.mockClear();

    view.rerender(show(async () => second, false, 'scope:two'));
    const newFrame = screen.getByTitle('Interactive tool result') as HTMLIFrameElement;
    expect(newFrame).not.toBe(oldFrame);
    expect(newFrame.contentWindow).not.toBe(oldFrame.contentWindow);
    expect(oldFrame.isConnected).toBe(true);
    expect(oldFrame.src).toContain('sandbox.test');
    expect(oldFrame).not.toBeVisible();
    const teardown = oldSent.mock.calls.find(([m]) => m.method === 'ui/resource-teardown')?.[0];
    expect(teardown).toBeDefined();

    // Retired callbacks cannot supply old HTML, start work, or alter the live frame.
    await act(async () => {
      message(oldFrame, 'ui/notifications/sandbox-proxy-ready', {});
      message(oldFrame, 'tools/call', { name: 'refresh', arguments: {} }, 2);
      message(oldFrame, 'resources/read', { uri: 'private://old' }, 3);
      message(oldFrame, 'ui/notifications/size-changed', { width: 900, height: 1200 });
    });
    expect(oldSent.mock.calls.filter(([m]) => m.method === 'ui/notifications/sandbox-resource-ready')).toEqual([]);
    expect(first.callTool).not.toHaveBeenCalled();
    expect(first.readResource).toHaveBeenCalledTimes(1);
    expect(newFrame.style.height).toBe('240px');

    await waitFor(() => expect(newFrame.src).toContain('sandbox.test'));
    const newSent = vi.spyOn(newFrame.contentWindow!, 'postMessage');
    await initialize(newFrame, newSent);
    await waitFor(() => expect(newSent.mock.calls.find(([m]) => m.method === 'ui/notifications/tool-result')?.[0].params).toEqual(second.result));
    expect(newSent.mock.calls.filter(([m]) => m.method === 'ui/notifications/sandbox-resource-ready').map(([m]) => m.params.html)).toEqual(['<html>Second app</html>']);

    await act(async () => {
      window.dispatchEvent(new MessageEvent('message', { source: oldFrame.contentWindow, origin: 'https://sandbox.test', data: { jsonrpc: '2.0', id: teardown.id, result: {} } }));
    });
    expect(oldFrame.isConnected).toBe(false);
    expect(first.dispose).toHaveBeenCalledTimes(1);
    view.unmount();
  });

  it.each(['resolve', 'readResource'])('times out a stalled %s and aborts its lifetime', async (stage) => {
    vi.useFakeTimers();
    let signal: AbortSignal;
    const binding = session();
    const pending = new Promise<never>(() => {});
    const resolve = stage === 'resolve'
      ? vi.fn((_locator, lifetime) => { signal = lifetime; return pending; })
      : vi.fn(async () => binding);
    if (stage === 'readResource') binding.readResource = vi.fn((_uri, lifetime) => { signal = lifetime; return pending; });
    const view = render(show(resolve));
    try {
      await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
      expect(signal!.aborted).toBe(true);
      expect(screen.getByRole('status')).toHaveTextContent(/^Seven results$/);
      expect(screen.queryByTitle('Interactive tool result')).not.toBeInTheDocument();
      if (stage === 'readResource') expect(binding.dispose).toHaveBeenCalledTimes(1);
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('requires an isolated HTTP sandbox and checks both source and origin', async () => {
    expect(() => mcpSandboxUrl('https://console.test/sandbox', 'https://console.test')).toThrow(/separate/);
    expect(() => mcpSandboxUrl('data:text/html,hi', 'https://console.test')).toThrow();
    const target = document.createElement('iframe'); document.body.append(target);
    const transport = new McpAppTransport(target.contentWindow!, 'https://sandbox.test');
    transport.onmessage = vi.fn(); await transport.start();
    const data = { jsonrpc: '2.0', method: 'ping', id: 1 };
    for (const event of [
      { source: window, origin: 'https://sandbox.test' },
      { source: target.contentWindow, origin: 'https://other.test' },
    ]) window.dispatchEvent(new MessageEvent('message', { ...event, data }));
    expect(transport.onmessage).not.toHaveBeenCalled();
    window.dispatchEvent(new MessageEvent('message', { source: target.contentWindow, origin: 'https://sandbox.test', data }));
    expect(transport.onmessage).toHaveBeenCalledTimes(1);
    const sent = vi.spyOn(target.contentWindow!, 'postMessage');
    transport.beginTeardown();
    window.dispatchEvent(new MessageEvent('message', { source: target.contentWindow, origin: 'https://sandbox.test', data }));
    expect(transport.onmessage).toHaveBeenCalledTimes(1);
    await transport.send(data);
    expect(sent).not.toHaveBeenCalled();
    await transport.send({ jsonrpc: '2.0', method: 'ui/resource-teardown', id: 2, params: {} });
    expect(sent).toHaveBeenCalledTimes(1);
    window.dispatchEvent(new MessageEvent('message', { source: target.contentWindow, origin: 'https://sandbox.test', data: { jsonrpc: '2.0', id: 2, result: {} } }));
    expect(transport.onmessage).toHaveBeenCalledTimes(2);
    await transport.close(); target.remove();
    await transport.send(data);
    expect(sent).toHaveBeenCalledTimes(1);
  });
});
