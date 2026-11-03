// Test-only DOM client for the real loopback companion. No external requests.
import { readFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { JSDOM } from "../../typescript/node_modules/jsdom/lib/api.js";
import { createWorkbench } from "../static/app.js";

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
let app, dom, origin, calls = [], pending = new Set();
async function drain() {
  for (let n = 0; n < 12; n++) {
    await Promise.all([...pending]);
    await new Promise(resolve => setImmediate(resolve));
  }
}
function snapshot() {
  const doc = dom.window.document, el = id => doc.getElementById(id);
  return { connection: el("connection-status").textContent, problem: el("problem").textContent,
    job: el("job-status").textContent, context: el("job-context").textContent,
    result: el("result").textContent, result_hidden: el("result").hidden,
    stop_disabled: el("stop-job").disabled, stop_hidden: el("stop-job").hidden,
    stop: el("stop-status").textContent, reconcile_disabled: el("reconcile").disabled,
    reconcile: el("reconciliation-status").textContent, history: el("job-choice").textContent,
    calls };
}
try {
  for await (const line of lines) {
    const command = JSON.parse(line);
    if (!app) {
      origin = new URL(command.origin).origin;
      if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(origin)) throw Error("loopback required");
      dom = new JSDOM(await readFile(new URL("../static/index.html", import.meta.url), "utf8"), { url: origin });
      app = createWorkbench(dom.window.document, { session: command.capability,
        setTimeout: () => 0, clearTimeout: () => {},
        fetch: (path, options) => {
          if (!/^\/api\/[a-z_]+$/.test(path)) throw Error("unexpected route");
          const call = { path, payload: JSON.parse(options.body) }; calls.push(call);
          const operation = fetch(origin + path, { ...options, headers: { ...options.headers, Origin: origin } })
            .then(async response => { call.status = response.status; const value = await response.json();
              return { ok: response.ok, status: response.status, json: async () => value }; });
          pending.add(operation); operation.finally(() => pending.delete(operation)); return operation;
        },
      });
      await app.start();
    } else if (command.action === "close") {
      app.dispose(); dom.window.close(); process.stdout.write(JSON.stringify({ closed: true }) + "\n"); break;
    } else if (command.action === "select") {
      const input = dom.window.document.getElementById("job-choice"); input.value = command.id;
      input.dispatchEvent(new dom.window.Event("change"));
    } else {
      if (!["refresh", "stop-job", "reconcile"].includes(command.action)) throw Error("unexpected action");
      dom.window.document.getElementById(command.action).click();
    }
    await drain(); process.stdout.write(JSON.stringify(snapshot()) + "\n");
  }
} finally { app?.dispose(); dom?.window.close(); lines.close(); }
