import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { JSDOM } from "../../typescript/node_modules/jsdom/lib/api.js";
import { createWorkbench } from "../../consumer-setup/static/app.js";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const python = process.env.SERVICES_TEST_PYTHON || "python3";
// Exercise the actual cold owner and existing Controller projection, not a
// second hand-maintained JavaScript copy of permission guidance.
const code = `
import importlib.abc, importlib.util, json, pathlib, sys
class ColdOnly(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname.split('.')[0] in {'mcp', 'httpx', 'httpcore', 'anyio'} or fullname.startswith('meerkat_toolkit.connectors'):
            raise AssertionError('runtime import during help projection')
sys.meta_path.insert(0, ColdOnly())
from meerkat_toolkit.adapters.services_config import configuration_help
spec = importlib.util.spec_from_file_location('setup_backend', pathlib.Path(sys.argv[1]) / 'sdk/consumer-setup/backend.py')
backend = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = backend
spec.loader.exec_module(backend)
result = {}
for profile in ('slack', 'gmail'):
    source = configuration_help(profile)
    projected = backend.normalize_help('services', profile, source, '')
    assert projected['prerequisites'] == source['prerequisites']
    assert projected['unresolved_details'] == source['unresolved_details']
    result[profile] = projected
print(json.dumps(result))
`;
const helps = JSON.parse(execFileSync(python, ["-B", "-c", code, root], {
  encoding: "utf8", timeout: 10000,
  env: { PATH: "/usr/bin:/bin", PYTHONPATH: `${root}/sdk/python/src`, PYTHONDONTWRITEBYTECODE: "1", PYTHONNOUSERSITE: "1" },
}));
const html = readFileSync(new URL("../../consumer-setup/static/index.html", import.meta.url), "utf8");
const settle = async () => { for (let index = 0; index < 6; index++) await new Promise(resolve => setImmediate(resolve)); };

for (const profile of ["slack", "gmail"]) {
  test(`actual ${profile} help survives Controller and form with zero installation effects`, async () => {
    const dom = new JSDOM(html, { url: "http://127.0.0.1:49231/#session=test-capability" });
    const document = dom.window.document;
    const calls = [];
    const state = { families: [{ id: "services", status: "prepared", profiles: Object.entries(helps).map(([id, help]) => ({ id, operations: help.operations })) }], job: null, scope: "local_installation_only" };
    const app = createWorkbench(document, { fetch: async (path, init) => {
      calls.push({ path, payload: JSON.parse(init.body) });
      assert.ok(["/api/state", "/api/help"].includes(path), "browsing triggered an effect");
      return { ok: true, status: 200, json: async () => structuredClone(path === "/api/state" ? state : helps[profile]) };
    } });
    const change = (id, value) => { const element = document.getElementById(id); element.value = value; element.dispatchEvent(new dom.window.Event("change", { bubbles: true })); };
    try {
      await app.start(); change("family", "services"); change("profile", profile); await settle();
      assert.deepEqual(calls.map(call => call.path), ["/api/state", "/api/help"]);
      assert.deepEqual(calls[1].payload, { family: "services", profile });
      for (const row of helps[profile].prerequisites) assert.ok(document.getElementById("prerequisites").textContent.includes(row.instruction));
      for (const row of helps[profile].unresolved_details) assert.ok(document.getElementById("unresolved").textContent.includes(row));
      assert.equal(document.querySelectorAll("#operations input:checked").length, 0);
      assert.equal(document.querySelector('#fields [name="token_env"]').value, "");
      assert.equal(document.querySelectorAll("#prerequisites a, #prerequisites script, #unresolved script").length, 0);
    } finally { app.dispose(); dom.window.close(); }
  });
}
