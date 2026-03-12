import http from 'node:http';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { createProxyHandler } from '../../../../sdks/web/proxy/index.mjs';
import { buildFixtureMobpack } from './mobpack-builder.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..');
const APP_DIR = path.join(ROOT, 'app');
const RUNTIME_DIR = path.resolve(ROOT, '..', '..', '..', 'sdks', 'web', 'wasm');
const FIXTURE_DIR = path.join(ROOT, 'fixtures', 'browser_safe_mobpack');

const MIME_TYPES = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.mjs', 'text/javascript; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.md', 'text/markdown; charset=utf-8'],
  ['.toml', 'text/plain; charset=utf-8'],
  ['.wasm', 'application/wasm'],
]);

async function serveFile(response, filePath) {
  const file = await readFile(filePath);
  const ext = path.extname(filePath);
  response.writeHead(200, {
    'content-type': MIME_TYPES.get(ext) ?? 'application/octet-stream',
    'cache-control': 'no-store',
  });
  response.end(file);
}

function resolveWithin(rootDir, relativePath) {
  const normalized = path.normalize(relativePath).replace(/^(\.\.(\/|\\|$))+/, '');
  const resolved = path.resolve(rootDir, normalized);
  if (!resolved.startsWith(rootDir)) {
    throw new Error(`refusing to serve path outside root: ${relativePath}`);
  }
  return resolved;
}

export async function startBrowserSmokeServer() {
  const anthropicKey = process.env.ANTHROPIC_API_KEY || process.env.RKAT_ANTHROPIC_API_KEY;
  if (!anthropicKey) {
    throw new Error('browser live smoke requires ANTHROPIC_API_KEY');
  }

  const mobpackBytes = await buildFixtureMobpack(FIXTURE_DIR);
  const anthropicProxy = createProxyHandler('anthropic', {
    apiKey: anthropicKey,
    allowOrigin: '*',
  });

  const server = http.createServer(async (request, response) => {
    try {
      if (request.method === 'GET' && request.url === '/health') {
        response.writeHead(200, { 'content-type': 'application/json; charset=utf-8' });
        response.end(JSON.stringify({ ok: true }));
        return;
      }

      if (request.method === 'GET' && request.url === '/fixtures/browser-safe.mobpack') {
        response.writeHead(200, {
          'content-type': 'application/gzip',
          'cache-control': 'no-store',
        });
        response.end(mobpackBytes);
        return;
      }

      const url = new URL(request.url ?? '/', 'http://127.0.0.1');
      if (url.pathname.startsWith('/anthropic/')) {
        const reqHeaders = new Headers();
        for (let index = 0; index < request.rawHeaders.length; index += 2) {
          reqHeaders.append(request.rawHeaders[index], request.rawHeaders[index + 1]);
        }
        const webReq = new Request(url.toString(), {
          method: request.method,
          headers: reqHeaders,
          body:
            request.method !== 'GET' && request.method !== 'HEAD'
              ? request
              : undefined,
          duplex: 'half',
        });
        const proxied = await anthropicProxy(webReq, url.pathname.slice('/anthropic'.length));
        const headers = {};
        for (const [name, value] of proxied.headers) {
          headers[name] = value;
        }
        response.writeHead(proxied.status, headers);
        if (proxied.body) {
          const reader = proxied.body.getReader();
          try {
            while (true) {
              const { done, value } = await reader.read();
              if (done) {
                break;
              }
              response.write(value);
            }
          } finally {
            reader.releaseLock();
          }
        }
        response.end();
        return;
      }

      if (request.method !== 'GET' && request.method !== 'HEAD') {
        response.writeHead(405, { 'content-type': 'text/plain; charset=utf-8' });
        response.end('method not allowed');
        return;
      }

      const pathname = url.pathname === '/' ? '/index.html' : url.pathname;
      if (pathname.startsWith('/runtime/')) {
        const relative = pathname.slice('/runtime/'.length);
        await serveFile(response, resolveWithin(RUNTIME_DIR, relative));
        return;
      }

      const relative = pathname.replace(/^\/+/, '');
      await serveFile(response, resolveWithin(APP_DIR, relative));
    } catch (error) {
      response.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' });
      response.end(error instanceof Error ? error.stack ?? error.message : String(error));
    }
  });

  await new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', (error) => {
      if (error) {
        reject(error);
      } else {
        resolve();
      }
    });
  });

  const address = server.address();
  if (!address || typeof address === 'string') {
    throw new Error('failed to determine live-smoke server address');
  }

  return {
    origin: `http://127.0.0.1:${address.port}`,
    async close() {
      await new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      });
    },
  };
}
