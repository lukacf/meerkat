#!/usr/bin/env node
// Local fixture preview, deliberately independent of provider credentials.
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const agent = "identity:example";
const frames = [
  { id: "request", kind: "user_input", identity: agent, interaction_id: "example-turn", timestamp_ms: 1791540000000,
    payload: { content: "Find the matching records.", origin: "console:example" } },
  { id: "complete", kind: "run_completed", identity: agent, interaction_id: "example-turn", timestamp_ms: 1791540002000,
    payload: { result: "The matching records are ready to review." } },
];
frames.forEach((frame, index) => { frame.cursor = `console:${index + 1}`; });
const experience = {
  contract_version: "0.5.0", runtime_id: "extension-preview", storage_scope: "extension-preview-subject",
  console_config: { title: "Extension example", panel_modules: ["/extensions/example.js"],
    layout: { initial_agent: agent }, environment: { label: "fixture" }, rail: { visible: false } },
  console_policy: { read_only: true },
  agent_sidebar: { live_snapshot: { agents: [{ identity: agent, member_id: agent, agent_id: agent,
    label: "Example agent", kind: "member", role: "worker", state: "idle", addressable: true, affordances: {} }] } },
  activity_feed: { filter_presets: [], active_preset_id: "all" },
};

function createPreviewServer() {
  return http.createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    const json = body => { res.writeHead(200, { "Content-Type": "application/json" }); res.end(JSON.stringify(body)); };
    if (url.pathname === "/console/experience") return json(experience);
    if (url.pathname === "/console/modules") return json({ modules: [] });
    if (url.pathname === "/console/timeline") return json({ frames, exhausted: true, available: true });
    if (url.pathname.endsWith("/stream")) {
      res.writeHead(200, { "Content-Type": "text/event-stream", "Cache-Control": "no-cache" });
      res.write('event: snapshot_complete\ndata: {"cursor":"console:3"}\n\n');
      return;
    }
    if (url.pathname === "/console/rpc") {
      let body = "";
      for await (const chunk of req) body += chunk;
      const request = JSON.parse(body);
      const result = request.method === "meta/console/query_timeline"
        ? { frames: request.params?.after ? [] : frames, exhausted: true, available: true, latest_cursor: "console:3" }
        : request.method.endsWith("capabilities") ? { version: "0.5.0", methods: [] } : {};
      return json({ jsonrpc: "2.0", id: request.id, result });
    }
    const files = {
      "/": ["console/dist/index.html", "text/html"],
      "/console": ["console/dist/index.html", "text/html"],
      "/console/assets/console-app.js": ["console/dist/console-app.js", "text/javascript"],
      "/console/assets/console-app.css": ["console/dist/console-app.css", "text/css"],
      "/extensions/example.js": ["examples/console-extensions/extension.js", "text/javascript"],
    };
    const file = files[url.pathname];
    if (!file) { res.writeHead(404); return res.end(); }
    try {
      res.writeHead(200, { "Content-Type": file[1] });
      res.end(fs.readFileSync(path.join(root, file[0])));
    } catch { res.end("Build the console first: npm --prefix console run build"); }
  });
}

module.exports = { createPreviewServer };
if (require.main === module) {
  const server = createPreviewServer();
  server.listen(Number(process.env.CONSOLE_EXTENSION_PREVIEW_PORT ?? 0), "127.0.0.1", () => console.log(`Fixture preview: http://127.0.0.1:${server.address().port}/console`));
}
