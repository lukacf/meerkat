// A real stdio MCP server for native Console acceptance. No host projection mocks.
const fs = require('node:fs');
const readline = require('node:readline');
const uri = 'ui://fixture/results.html';
let count = 7;
const viewCounts = new Map();
const log = value => fs.appendFileSync(process.env.META_FIXTURE_APP_LOG, JSON.stringify({ pid: process.pid, atMs: Date.now(), ...value }) + '\n');
const result = (viewId, operation, requestId) => {
  const value = { count: viewId ? (viewCounts.get(viewId) ?? 7) : count };
  if (viewId) value.viewId = viewId;
  if (requestId) value.responseMarker = `APP_${operation.toUpperCase()}_RESULT_${requestId}`;
  return {
    content: [{ type: 'text', text: JSON.stringify(value) }],
    structuredContent: value,
    _meta: { viewDetail: 'PRIVATE_APP_DETAIL', ...(requestId ? { pollDetail: 'PRIVATE_POLL_DETAIL' } : {}) },
  };
};
const lines = readline.createInterface({ input: process.stdin });
lines.on('line', line => {
  const request = JSON.parse(line);
  if (request.id === undefined) return;
  log({ method: request.method, params: request.params });
  let value, error;
  switch (request.method) {
    case 'initialize':
      value = { protocolVersion: request.params.protocolVersion, serverInfo: { name: 'native-apps-fixture', version: '1' }, capabilities: { tools: {}, resources: {} } }; break;
    case 'ping': value = {}; break;
    case 'tools/list':
      value = { tools: [
        { name: 'display', description: 'Display matching records', inputSchema: { type: 'object', properties: { query: { type: 'string' }, viewId: { type: 'string' } } }, _meta: { ui: { resourceUri: uri, visibility: ['model'] } } },
        { name: 'refresh', description: 'Refresh records from the app', inputSchema: { type: 'object' }, _meta: { ui: { resourceUri: uri, visibility: ['app'] } } },
        { name: 'poll', description: 'Read the current records from the app', inputSchema: { type: 'object' }, annotations: { readOnlyHint: true }, _meta: { ui: { resourceUri: uri, visibility: ['app'] } } },
      ] }; break;
    case 'resources/list': value = { resources: [] }; break;
    case 'resources/read':
      if (request.params.uri === uri) value = { contents: [{ uri, mimeType: 'text/html;profile=mcp-app', text: fs.readFileSync(process.env.META_FIXTURE_APP_HTML, 'utf8') }] };
      else error = { code: -32602, message: 'Unknown resource' };
      break;
    case 'tools/call':
      {
        const { viewId, requestId } = request.params.arguments ?? {};
        if (request.params.name === 'refresh') {
          if (viewId) viewCounts.set(viewId, (viewCounts.get(viewId) ?? 7) + 1);
          else count++;
          value = result(viewId, 'refresh', requestId);
        } else if (request.params.name === 'display') value = result(viewId);
        else if (request.params.name === 'poll') value = result(viewId, 'poll', requestId);
        else error = { code: -32602, message: 'Unknown tool' };
      }
      break;
    default: error = { code: -32601, message: 'Unknown method' };
  }
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, ...(error ? { error } : { result: value }) }) + '\n');
});
