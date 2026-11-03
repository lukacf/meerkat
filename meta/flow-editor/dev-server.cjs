#!/usr/bin/env node

const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");

const root = path.join(__dirname, "dist");
const port = Number(process.env.PORT || 4190);
const rpcProxyUrl = (process.env.META_FLOW_EDITOR_RPC_URL || "").trim();

const types = {
  ".css": "text/css; charset=utf-8",
  ".html": "text/html; charset=utf-8",
  ".js": "application/javascript; charset=utf-8",
};

function send(res, status, type, body) {
  res.writeHead(status, { "content-type": type, "cache-control": "no-store" });
  res.end(body);
}

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
  });
}

function jsonRpc(id, result) {
  return JSON.stringify({ jsonrpc: "2.0", id, result });
}

function jsonRpcError(id, message, code = -32603) {
  return JSON.stringify({ jsonrpc: "2.0", id, error: { code, message } });
}

async function proxyRpc(payload) {
  const response = await fetch(rpcProxyUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(payload),
  });
  const text = await response.text();
  if (!response.ok) {
    throw new Error(`Meta RPC proxy ${response.status}: ${text.slice(0, 240)}`);
  }
  JSON.parse(text);
  return text;
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url || "/", `http://127.0.0.1:${port}`);
  if (url.pathname === "/flow-editor/rpc" && req.method === "POST") {
    try {
      const payload = JSON.parse(await readBody(req));
      if (rpcProxyUrl) {
        send(res, 200, "application/json; charset=utf-8", await proxyRpc(payload));
        return;
      }
      send(res, 200, "application/json; charset=utf-8", jsonRpcError(
        payload.id ?? null,
        "flow editor dev server requires META_FLOW_EDITOR_RPC_URL=<real /flow-editor/rpc endpoint>; fixture RPC is not available",
        -32000,
      ));
      return;
    } catch (error) {
      send(res, 200, "application/json; charset=utf-8", jsonRpcError(null, error.message || String(error)));
      return;
    }
  }
  if (url.pathname === "/" || url.pathname === "/flow-editor" || url.pathname === "/flow-editor/") {
    send(res, 200, types[".html"], fs.readFileSync(path.join(root, "index.html")));
    return;
  }
  if (url.pathname.startsWith("/flow-editor/assets/")) {
    const name = path.basename(url.pathname);
    const file = path.join(root, name);
    if (fs.existsSync(file)) {
      send(res, 200, types[path.extname(name)] || "application/octet-stream", fs.readFileSync(file));
      return;
    }
  }
  if (url.pathname === "/healthz") {
    send(res, 200, "text/plain; charset=utf-8", "ok");
    return;
  }
  send(res, 404, "text/plain; charset=utf-8", "not found");
});

server.listen(port, "127.0.0.1", () => {
  const rpcMode = rpcProxyUrl
    ? `proxying RPC to ${rpcProxyUrl}`
    : "RPC disabled until META_FLOW_EDITOR_RPC_URL is set";
  process.stdout.write(`flow editor listening on http://127.0.0.1:${port}/flow-editor (${rpcMode})\n`);
});

process.on("SIGTERM", () => server.close(() => process.exit(0)));
