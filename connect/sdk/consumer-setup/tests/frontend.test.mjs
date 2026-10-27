import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { JSDOM } from "../../typescript/node_modules/jsdom/lib/api.js";
import { createWorkbench } from "../static/app.js";
import { draftFromFields, FormError, normalizeHelp, takeSession } from "../static/form.js";

const html = await readFile(new URL("../static/index.html", import.meta.url), "utf8");
const clone = value => structuredClone(value);
const settle = async () => { for (let index = 0; index < 6; index += 1) await new Promise(resolve => setImmediate(resolve)); };
const deferred = () => { let resolve; const promise = new Promise(value => { resolve = value; }); return { promise, resolve }; };
const response = (value, status = 200) => ({ ok: status === 200, status, json: async () => clone(value) });

function diagnosticFixture(profile = "slack", handler = () => undefined) {
  const ops = profile === "slack" ? ["auth_test", "list_conversations_page"] : ["send_message", "send_raw"];
  const field = profile === "slack" ? "allowed_channel_ids" : "allowed_recipient_addresses";
  const value = profile === "slack" ? '["CSELECTED"]' : '["reader@example.invalid"]';
  const initial = { families: [{ id: "services", status: "prepared", profiles: [{ id: profile,
    operations: ops.map(id => ({ id, effect: "read" })) }] }], job: null,
    family_contents_sha256: { services: "c".repeat(64) }, scope: "local_installation_only" };
  const h = harness(initial, handler);
  h.fixture.help = { family: "services", profile, operations: initial.families[0].profiles[0].operations,
    selectable_operations: true, fields: [{ name: field, kind: "string_list", required: false, credential_reference: false }],
    prerequisites: [], unresolved_details: [], guide_text: "Configuration reference", scope: "configuration_only" };
  h.ready = async () => {
    await h.app.start(); h.change(h.byId("family"), "services"); h.change(h.byId("profile"), profile); h.byId("show-profile").click();
    await settle(); h.checked(0); h.checked(1); h.change(h.field(field), value, "input");
  };
  h.rejection = (code = profile === "slack" ? "channel_list_conflict" : "recipient_operation_conflict", path = ["operations"]) => ({
    error: "invalid_configuration", diagnostic: { version: 1, phase: "configuration_validation", code, field_path: path,
      message: "PRIVATE-UNTRUSTED-MESSAGE", family: "services", profile,
      configuration_sha256: "a".repeat(64), contents_sha256: "c".repeat(64) } });
  h.listField = field; h.listValue = value;
  return h;
}

test("service conflicts explain explicit correction without changing access restrictions", async () => {
  for (const profile of ["slack", "gmail"]) {
    let accept = false;
    const h = diagnosticFixture(profile, call => call.path === "/api/check" ?
      response(accept ? validationValue() : h.rejection(), accept ? 200 : 400) : undefined);
    try {
      await h.ready(); await h.submit();
      assert.match(h.byId("validation-status").textContent, /Keep the restriction/);
      assert.equal(h.document.activeElement.id, "operation-0");
      assert.equal(h.field(h.listField).value, h.listValue);
      assert.equal(h.byId("operations").querySelectorAll("input:checked").length, 2);
      assert.equal(h.byId("install").disabled, true);
      assert.equal(h.calls.filter(call => call.path === "/api/install").length, 0);
      assert.doesNotMatch(h.document.body.textContent, /PRIVATE-UNTRUSTED-MESSAGE/);
      accept = true; h.checked(1, false); await h.submit();
      assert.equal(h.byId("review").hidden, false);
      const calls = h.calls.filter(call => call.path === "/api/check");
      assert.deepEqual(calls[1].payload.values, calls[0].payload.values);
      assert.equal(calls[1].payload.operations.length, 1);
      assert.equal(h.byId("operation-0").hasAttribute("aria-invalid"), false);
    } finally { h.close(); }
  }
});

test("non-ASCII and astral invalid values receive field diagnostics without reflected values", async () => {
  const h = diagnosticFixture("slack", call => call.path === "/api/check" ?
    response(h.rejection("invalid_channel_ids", ["config", "allowed_channel_ids"]), 400) : undefined);
  try {
    await h.ready(); h.change(h.field(h.listField), '["caf\u00e9\ud83d\ude00"]', "input"); await h.submit();
    assert.equal(h.document.activeElement, h.field(h.listField));
    assert.equal(h.field(h.listField).getAttribute("aria-invalid"), "true");
    assert.match(h.byId("validation-status").textContent, /unique channel IDs/);
    assert.doesNotMatch(h.byId("problem").textContent, /caf|PRIVATE/);
    assert.equal(h.field(h.listField).value, '["caf\u00e9\ud83d\ude00"]');
  } finally { h.close(); }
});

test("Slack result-byte limit keeps the installed owner's field diagnostic", async () => {
  const h = diagnosticFixture("slack", call => call.path === "/api/check" ?
    response(h.rejection("integer_out_of_range", ["config", "max_interval_result_bytes"]), 400) : undefined);
  h.fixture.help.fields.push({ name: "max_interval_result_bytes", kind: "integer", required: false,
    credential_reference: false, minimum: 4096, maximum: 16777216 });
  try {
    await h.ready(); h.change(h.field("max_interval_result_bytes"), "8192", "input"); await h.submit();
    assert.equal(h.document.activeElement, h.field("max_interval_result_bytes"));
    assert.equal(h.field("max_interval_result_bytes").getAttribute("aria-invalid"), "true");
    assert.match(h.byId("validation-status").textContent, /whole number within the bounds/);
    assert.doesNotMatch(h.document.body.textContent, /PRIVATE-UNTRUSTED-MESSAGE/);
    assert.equal(h.field("max_interval_result_bytes").value, "8192");
    assert.equal(h.calls.filter(call => call.path === "/api/install").length, 0);
  } finally { h.close(); }
});

test("diagnostics with unsupported or mismatched bindings remain generic", async () => {
  const mutations = [
    v => { v.diagnostic.profile = "gmail"; }, v => { v.diagnostic.family = "agno"; },
    v => { v.diagnostic.contents_sha256 = "d".repeat(64); }, v => { v.diagnostic.configuration_sha256 = "invalid"; },
    v => { v.diagnostic.version = true; }, v => { v.diagnostic.code = "__proto__"; },
    v => { v.diagnostic.code = "invalid_mailbox"; v.diagnostic.field_path = ["config", "user_id"]; },
    v => { v.diagnostic.field_path = ["config", "SECRET-REFLECTED-PATH"]; },
    v => { v.diagnostic.extra = "PRIVATE-EXTRA"; }, v => { delete v.diagnostic; },
  ];
  for (const mutate of mutations) {
    const h = diagnosticFixture("slack", call => {
      if (call.path === "/api/check") { const v = h.rejection(); mutate(v); return response(v, 400); }
    });
    try {
      await h.ready(); await h.submit();
      assert.match(h.byId("validation-status").textContent, /did not accept/);
      assert.doesNotMatch(h.document.body.textContent, /PRIVATE|SECRET-REFLECTED|Keep the restriction/);
      assert.equal(h.byId("review").hidden, true);
    } finally { h.close(); }
  }
});

test("late service diagnostic cannot change focus or status after an edit", async () => {
  const waiting = deferred();
  const h = diagnosticFixture("slack", call => call.path === "/api/check" ? waiting.promise : undefined);
  try {
    await h.ready(); await h.submit();
    h.change(h.field(h.listField), '["CCHANGED"]', "input"); h.byId("configure-heading").focus();
    waiting.resolve(response(h.rejection(), 400)); await settle();
    assert.doesNotMatch(h.byId("validation-status").textContent, /Keep the restriction/);
    assert.equal(h.document.activeElement.id, "configure-heading");
    assert.equal(h.field(h.listField).value, '["CCHANGED"]');
  } finally { h.close(); }
});
const operations = [{ id: "search_web", effect: "unspecified" }, { id: "write_fixture", effect: "write" }];
const profile = "fixture-profile";
const helpValue = () => ({
  family: "agno", profile, operations: clone(operations), selectable_operations: true,
  fields: [
    { name: "host", kind: "string", required: true, credential_reference: false },
    { name: "timeout", kind: "integer", required: false, credential_reference: false },
    { name: "token_env", kind: "string", required: false, credential_reference: true },
    { name: "enable_writes", kind: "boolean", required: true, credential_reference: false, only_when_operation_selected: "write_fixture" },
  ],
  prerequisites: [{ kind: "client_authentication", instruction: "Supply an existing token through a host environment reference." }],
  unresolved_details: ["No provider account is checked."], scope: "configuration_only",
  guide_text: "# Configuration reference\nUse a valid host and a positive timeout.\n<script>throw Error('not executed')</script>",
});
const stateValue = (status = "prepared") => ({ families: [{ id: "agno", status, profiles: status === "prepared" ? [{ id: profile, operations: clone(operations) }] : [] }], job: null, scope: "local_installation_only" });
const validationValue = () => ({ validation_id: "fixture-validation", configuration_sha256: "a".repeat(64), scope: "configuration_only" });
const job = (status, kind = "install", id = "fixture-job") => ({ id, family: "agno", kind, status });
const completed = () => ({ ...job("succeeded"), result: {
  mcp_path: "/host/chosen/new/ready/mcp.json", receipt_path: "/host/chosen/new/ready/installed.json", scope: "local_installation_only",
  mcp_configuration: { mcpServers: { fixture: { command: "/host/chosen/python", args: ["-I", "-B", "-m", "fixture", "serve", "--config", "/host/chosen/nonsecret.json"] } } },
} });
const observation = (status = "matched", jobId = "fixture-job", revision = 1) => ({
  job_id: jobId, revision, status, scope: "retained_payload_and_launch_binding_only",
  excluded: ["Runtime integrity is not verified.", "No account access or native attachment was established."],
  ...(status === "matched" ? { result: { ...completed().result, family: "agno", profile,
    receipt_sha256: "b".repeat(64) } } : {}),
});
const selectedForms = [["python", false], ["node", false], ["mcpb", false], ["python", true], ["node", true]];
const selectedResult = (kind = "python", scope = "observed_software_installation_only", software = false) => ({
  ...completed().result, scope, selection_kind: kind, component: "selected-server",
  selection_sha256: "a".repeat(64),
  ...(software ? { document_kind: "local_software_declaration", declaration_digest: "b".repeat(64) } : { manifest_digest: "b".repeat(64) }),
  effect_sha256: "c".repeat(64), receipt_sha256: "d".repeat(64),
  ...(kind === "mcpb" ? { configuration_sha256: "e".repeat(64) } : {}),
});
const selectedState = (kind = "python", status = "running", software = false) => ({
  families: [], mode: "setup", scope: "local_installation_only",
  selected_installation: { scope: "standing_local_software_installation_only", profiles: [kind] },
  job: { id: "1".repeat(32), family: `selected-${kind}`, kind: "install", status,
    ...(status === "succeeded" ? { result: selectedResult(kind, "observed_software_installation_only", software) } : {}) },
  reconciliation: { available: true, observation: null },
});
const selectedObservation = (kind = "python", status = "matched", revision = 1, software = false) => ({
  ...observation(status, "1".repeat(32), revision),
  ...(status === "matched" ? { result: selectedResult(kind, "retained_payload_and_launch_binding_only", software) } : {}),
});

const acquisitionScopes = { mcpb: "trusted_public_mcpb_acquisition", python: "trusted_pypi_wheel_acquisition", node: "trusted_npm_registry_acquisition", compose: "acquire_agent_selected_container_images" };
const acquisitionState = (kind = "python", status = "running") => {
  const id = "5".repeat(32), request = "6".repeat(32), hash = "a".repeat(64);
  if (kind === "mcpb") return {
    families: [], mode: "setup", scope: "local_installation_only", stop: { available: true },
    selected_installation: { scope: "standing_local_software_installation_only", profiles: ["python", "mcpb"] },
    mcpb_acquisition: { available: true, scope: acquisitionScopes.mcpb, staging_root: "/host/staging", host_profile_sha256: hash },
    reconciliation: { available: false, observation: null },
    job: { id, kind: "acquire", family: "selected-mcpb", request_id: request, status,
      ...(status === "succeeded" ? { result: {
        schema_version: 1, scope: "retained_mcpb_artifact_bytes", staged_id: id,
        host_profile_sha256: hash, request: { request_id: request, url: "https://public.example/tool.mcpb", artifact_sha256: hash },
        primary: { url: "https://public.example/tool.mcpb", sha256: hash, byte_length: 123,
          final_origin: "https://assets.example", redirect_count: 1 },
        report_sha256: "b".repeat(64), files: { "artifact.mcpb": hash },
        receipt_path: `/host/staging/${id}/acquisition.json`, receipt_sha256: "c".repeat(64),
      } } : {}),
    },
  };
  const files = kind === "compose" ? Object.fromEntries([
    "host-profile.json", "selection.json", "backend-profile.json", "inputs.json", "request.json", "prepared.json", "image-acquisition.json",
  ].map(name => [name, hash])) : { [kind === "python" ? "artifact.whl" : "artifact.tgz"]: hash,
    ...(kind === "node" ? { "closure/package-json": hash, "closure/package-lock": hash } : {}) };
  const primary = { name: "chosen-package", version: "1.0.0", sha256: hash, url: "https://provider.example/retained-package" };
  return {
    families: [], mode: "setup", scope: "local_installation_only", stop: { available: true },
    selected_installation: { scope: "standing_local_software_installation_only", profiles: ["python", "node"] },
    [`${kind}_acquisition`]: { available: true, scope: acquisitionScopes[kind],
      staging_root: "/host/staging", host_profile_sha256: hash },
    reconciliation: { available: false, observation: null },
    job: { id, kind: "acquire", family: `selected-${kind}`, request_id: request, status,
      ...(status === "succeeded" ? { result: kind === "compose" ? {
        scope: "compose_plan_for_host_review", proposal_directory: `/host/workspace/acquisitions/${id}/proposal`,
        receipt_sha256: hash, proposal: { schema_version: 1, status: "proposed", scope: "compose_plan_for_host_review", plan_sha256: hash, files },
      } : { schema_version: 1, scope: kind === "python" ? "retained_python_wheel_inputs" : "retained_node_package_inputs",
        request: { request_id: request, [kind === "python" ? "distribution" : "package"]: primary.name, version: primary.version },
        host_profile_sha256: hash, installer_sha256: hash,
        ...(kind === "python" ? { pip_sha256: hash, report_sha256: hash } : { npm_sha256: hash, npm_version: "12.2.0", runtime: { platform: "darwin", arch: "arm64", version: "v24.19.0" } }),
        staged_id: id, primary, artifacts: [primary], files,
        receipt_path: `/host/staging/${id}/acquisition.json`, receipt_sha256: hash,
      } } : {}),
    },
  };
};

test("MCPB receipt projection refuses incoherent bundle facts without rendering private fields", async () => {
  const mutations = [
    r => { r.schema_version = true; }, r => { r.request.artifact_sha256 = "b".repeat(64); },
    r => { r.primary.sha256 = "b".repeat(64); }, r => { r.primary.url += "/other"; },
    r => { r.files["artifact.mcpb"] = "b".repeat(64); }, r => { r.files["other"] = "a".repeat(64); },
    r => { r.primary.byte_length = -1; }, r => { r.primary.byte_length = 64 * 1024 * 1024 + 1; },
    r => { r.primary.byte_length = 1.5; }, r => { r.primary.redirect_count = 6; },
    r => { r.primary.redirect_count = true; }, r => { r.report_sha256 = "invalid"; },
    r => { r.request.version = "invented"; }, r => { r.primary.final_origin = "https://assets.example?PRIVATE"; },
    r => { r.request.url += "?PRIVATE"; r.primary.url = r.request.url; },
    r => { r.request.url = "https://user:PRIVATE@public.example/tool.mcpb"; r.primary.url = r.request.url; },
  ];
  for (const mutate of mutations) {
    const initial = acquisitionState("mcpb", "succeeded"); mutate(initial.job.result);
    const h = harness(initial);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("stop-job").disabled, true);
      assert.doesNotMatch(h.document.documentElement.outerHTML, /PRIVATE|invented/);
      assert.equal(h.calls.some(c => /acquire|install|retry|stop/.test(c.path)), false);
    } finally { h.close(); }
  }
});

test("MCPB preserves owner URL spellings without browser URL recanonicalization", async () => {
  for (const origin of ["https://010.0.0.1", "https://127.1", "https://[2001:db8::1]:8443"]) {
    const initial = acquisitionState("mcpb", "succeeded");
    initial.job.result.request.url = origin + "/bundle.mcpb";
    initial.job.result.primary.url = initial.job.result.request.url;
    initial.job.result.primary.final_origin = origin;
    const h = harness(initial);
    try {
      await h.app.start(); assert.equal(h.byId("problem").textContent, "");
      assert.equal(h.byId("result").hidden, false);
      assert.ok(!h.document.documentElement.outerHTML.includes(origin));
    } finally { h.close(); }
  }
});

test("MCPB projects only bundle size and hashes, not URLs or registry package identity", async () => {
  for (const byteLength of [0, 64 * 1024 * 1024]) {
    const initial = acquisitionState("mcpb", "succeeded");
    initial.job.result.primary.byte_length = byteLength;
    initial.job.result.extra = { url: "https://secret.example/?PRIVATE", text: "PRIVATE" };
    const h = harness(initial);
    try {
      await h.app.start();
      assert.equal(h.byId("problem").textContent, "");
      assert.match(h.byId("result-paths").textContent, /Bundle bytes/);
      assert.ok(h.byId("result-paths").textContent.includes(String(byteLength)));
      assert.doesNotMatch(h.document.documentElement.outerHTML, /PRIVATE|public\.example|assets\.example|undefined/);
      assert.equal(h.byId("result-paths").querySelectorAll("a, img").length, 0);
    } finally { h.close(); }
  }
});

test("acquisition jobs on another surface remain visible and stop the exact running job", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const h = harness(acquisitionState(kind), (call, fixture) => {
      if (call.path === "/api/stop") {
        fixture.state.job.stop = { status: "requested", scope: "local_job_only" };
        return response({ job: fixture.state.job });
      }
    });
    try {
      await h.app.start();
      assert.equal(h.byId("problem").textContent, "");
      assert.match(h.byId("job-status").textContent, new RegExp(`${kind === "python" ? "Python" : kind === "node" ? "Node" : kind === "mcpb" ? "MCPB" : "Compose"}.*acquisition.*running`));
      assert.equal(h.byId("stop-job").disabled, false);
      assert.equal(h.byId("reconcile").hidden, true);
      assert.equal(h.byId("result").hidden, true);
      h.byId("stop-job").click(); h.byId("stop-job").click(); await settle();
      assert.deepEqual(h.calls.filter(call => call.path === "/api/stop").map(call => call.payload), [{ job_id: "5".repeat(32) }]);
      assert.match(h.byId("stop-status").textContent, /Stop requested/);
      assert.equal(h.byId("stop-job").disabled, true);
      h.fixture.state.job.status = "unconfirmed";
      h.fixture.state.job.stop.status = "settled";
      h.byId("refresh").click(); await settle();
      assert.match(h.byId("stop-status").textContent, /acquisition outcome remains unconfirmed/);
      assert.doesNotMatch(h.byId("stop-status").textContent, /installation outcome/);
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.calls.some(call => /retry|install|acquire/.test(call.path)), false);
    } finally { h.close(); }
  }
});

test("acquisition success shows only retained inputs or a reviewable proposal", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = acquisitionState(kind, "succeeded");
    initial.job.result.unexpected_private_value = "PRIVATE-CANARY";
    if (kind !== "compose") initial.job.result.primary.url += "/PRIVATE-CANARY";
    if (kind === "mcpb") initial.job.result.request.url = initial.job.result.primary.url;
    const h = harness(initial);
    try {
      await h.app.start();
      assert.equal(h.byId("problem").textContent, "");
      assert.equal(h.byId("result").hidden, false);
      assert.equal(h.byId("stop-job").disabled, true);
      assert.match(h.byId("result-heading").textContent, kind === "compose" ? /proposal/i : /inputs/i);
      assert.match(h.byId("result-limits").textContent, /not.*installed|does not install/);
      assert.doesNotMatch(h.byId("result-heading").textContent, /Installed MCP/);
      assert.ok(h.byId("result-paths").textContent.includes(initial.job.result.receipt_sha256));
      assert.ok(h.byId("mcp-configuration").textContent.includes(Object.keys(kind === "compose" ? initial.job.result.proposal.files : initial.job.result.files)[0]));
      assert.ok(!h.document.body.textContent.includes("PRIVATE-CANARY"));
      assert.equal(h.document.activeElement.id, "result-heading");
      assert.deepEqual(h.calls.map(call => call.path), ["/api/state"]);
    } finally { h.close(); }
  }
});

test("acquisition displays host paths literally without imposing another filesystem policy", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = acquisitionState(kind, "succeeded"), directory = "/host/a line\nbreak and \\ literal/<markup>";
    initial[`${kind}_acquisition`].staging_root = directory;
    const path = kind === "compose" ? `${directory}/${initial.job.id}/proposal` : `${directory}/${initial.job.id}/acquisition.json`;
    initial.job.result[kind === "compose" ? "proposal_directory" : "receipt_path"] = path;
    const h = harness(initial);
    try {
      await h.app.start();
      assert.equal(h.byId("problem").textContent, "");
      assert.equal(h.byId("result").hidden, false);
      assert.ok(h.byId("result-paths").textContent.includes(path));
      assert.equal(h.byId("result-paths").querySelector("markup"), null);
    } finally { h.close(); }
  }
});

test("acquisition history remains selectable after a later installation", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = acquisitionState(kind, "succeeded"), earlier = clone(initial.job);
    initial.job = selectedState("python", "succeeded").job;
    initial.job_history = { available: true, count: 2 };
    const h = harness(initial, call => {
      if (call.path === "/api/jobs") return response({ jobs: [jobSummary(initial.job), jobSummary(earlier)], next_before: null });
      if (call.path === "/api/job") return response({ job: earlier, reconciliation: { available: false, observation: null } });
    });
    try {
      await h.app.start();
      assert.equal(h.byId("problem").textContent, "");
      assert.match(h.byId("result-heading").textContent, /Installed MCP/);
      assert.match(h.byId("job-choice").textContent, /acquisition/);
      h.change(h.byId("job-choice"), earlier.id); await settle();
      assert.equal(h.byId("problem").textContent, "");
      assert.match(h.byId("result-heading").textContent, kind === "compose" ? /proposal/i : /inputs/i);
      assert.equal(h.byId("reconcile").hidden, true);
      assert.equal(h.byId("retry-selected").hidden, true);
      assert.equal(h.byId("stop-job").disabled, true);
      h.change(h.byId("job-choice"), ""); await settle();
      assert.match(h.byId("result-heading").textContent, /Installed MCP/);
      assert.equal(h.calls.some(call => /\/api\/(install|acquire|stop|retry)/.test(call.path)), false);
    } finally { h.close(); }
  }
});

test("acquisition refuses wrong result bindings or missing host capability", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const mutations = [
      s => { delete s[`${kind}_acquisition`]; },
      s => { s[`${kind}_acquisition`].available = false; },
      s => { s[`${kind}_acquisition`].scope = "other_scope"; },
      s => { s.job.family = "selected-unknown"; },
      s => { s.job.retry_of = "7".repeat(32); },
      s => { s.mode = "recovery"; },
      s => { s.reconciliation.available = true; },
      s => { s.reconciliation.observation = observation("checking", s.job.id); },
      s => { s.job.result.scope = "observed_software_installation_only"; },
      s => { (kind === "compose" ? s.job.result.proposal.files : s.job.result.files)["../escape"] = "a".repeat(64); },
      ...(kind === "compose" ? [
        s => { s.job.result.proposal_directory = "/host/other/proposal"; },
        s => { delete s.job.result.proposal.files["image-acquisition.json"]; },
        s => { s.job.result.proposal.status = "observed"; },
      ] : [
        s => { s.job.result.request.request_id = "7".repeat(32); },
        s => { s.job.result.staged_id = "7".repeat(32); },
        s => { s.job.result.host_profile_sha256 = "b".repeat(64); },
        s => { s.job.result.receipt_path = "/other/acquisition.json"; },
      ]),
    ];
    for (const mutate of mutations) {
      const initial = acquisitionState(kind, "succeeded"); mutate(initial);
      const h = harness(initial);
      try {
        await h.app.start();
        assert.match(h.byId("connection-status").textContent, /unconfirmed/);
        assert.equal(h.byId("result").hidden, true);
        assert.equal(h.byId("stop-job").disabled, true);
      } finally { h.close(); }
    }
  }
});

test("acquisition started after page load is discovered by polling and can complete", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = acquisitionState(kind); initial.job = null;
    const h = harness(initial);
    try {
      await h.app.start();
      assert.equal(h.byId("job-panel").hidden, true);
      h.fixture.state.job = acquisitionState(kind).job;
      const poll = [...h.timers.values()].at(-1); assert.equal(typeof poll, "function"); poll(); await settle();
      assert.match(h.byId("job-status").textContent, /acquisition.*running/);
      assert.equal(h.byId("stop-job").disabled, false);
      h.fixture.state.job = acquisitionState(kind, "succeeded").job;
      h.byId("refresh").click(); await settle();
      assert.match(h.byId("job-status").textContent, /acquisition completed/);
      assert.equal(h.byId("result").hidden, false);
      assert.equal(h.calls.some(call => /\/api\/(acquire|install|stop)/.test(call.path)), false);
    } finally { h.close(); }
  }
});

const recoveredAcquisition = kind => {
  const state = acquisitionState(kind, "unconfirmed");
  state.mode = "recovery";
  state.job_history = { available: true, count: 1 };
  state.stop.available = false;
  state.job.recovered = true;
  state.job.stop_supported = false;
  state.job.recovery_request = { id: state.job.id, sha256: "b".repeat(64) };
  state[`${kind}_acquisition`].workspace = "/host/workspace";
  state.reconciliation.available = true;
  return state;
};
const acquisitionObservation = (kind, status = "matched", revision = 1) => ({
  job_id: "5".repeat(32), revision, status, scope: "retained_acquisition_files_only",
  excluded: ["No current image availability, account access or native attachment is established."],
  ...(status === "matched" ? { result: acquisitionState(kind, "succeeded").job.result } : {}),
});

test("recovered acquisition checks files without restoring setup or rewriting history", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = recoveredAcquisition(kind);
    const h = harness(initial, (call, fixture) => {
      if (call.path === "/api/reconcile") {
        fixture.state.reconciliation.observation = acquisitionObservation(kind);
        return response({ observation: fixture.state.reconciliation.observation });
      }
    });
    try {
      await h.app.start();
      assert.equal(h.byId("problem").textContent, "");
      assert.match(h.byId("job-status").textContent, /historical acquisition outcome is unknown/);
      assert.equal(h.byId("reconcile").hidden, false);
      assert.equal(h.byId("reconcile").disabled, false);
      assert.equal(h.byId("reconcile").textContent, "Check acquired files");
      assert.equal(h.byId("stop-job").hidden, true);
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.calls.filter(call => call.path === "/api/reconcile").length, 0);
      h.byId("reconcile").click(); h.byId("reconcile").click(); await settle();
      assert.deepEqual(h.calls.filter(call => call.path === "/api/reconcile").map(call => call.payload), [{ job_id: initial.job.id }]);
      assert.equal(h.byId("result").hidden, false);
      assert.match(h.byId("reconciliation-status").textContent, /historical job outcome above is unchanged/);
      assert.match(h.byId("job-status").textContent, /unknown/);
      assert.match(h.byId("result-limits").textContent, /does not establish current image availability/);
      assert.equal(h.calls.some(call => /\/api\/(acquire|install|retry|stop)/.test(call.path)), false);
      h.fixture.state.reconciliation.observation = acquisitionObservation(kind, "unconfirmed", 2);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.match(h.byId("reconciliation-status").textContent, /No download, image pull, repair or retry/);
    } finally { h.close(); }
  }
});

test("acquisition recovery refuses crossed requests, host bindings and success claims", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const mutations = [
      state => { state.job.recovery_request.id = "9".repeat(32); },
      state => { delete state.job.recovery_request; },
      state => { state.job.status = "succeeded"; state.job.result = acquisitionState(kind, "succeeded").job.result; },
      state => { state.reconciliation.observation.job_id = "9".repeat(32); },
      state => { state.reconciliation.observation.scope = "observed_software_installation_only"; },
      state => { state.reconciliation.observation.status = "unconfirmed"; },
      ...(kind === "compose" ? [
        state => { delete state.compose_acquisition.workspace; },
        state => { state.reconciliation.observation.result.proposal_directory = "/foreign/proposal"; },
      ] : [
        state => { state.reconciliation.observation.result.host_profile_sha256 = "c".repeat(64); },
        state => { state.reconciliation.observation.result.request.request_id = "9".repeat(32); },
        state => { state.reconciliation.observation.result.receipt_path = `/foreign/${state.job.id}/acquisition.json`; },
      ]),
    ];
    for (const mutate of mutations) {
      const state = recoveredAcquisition(kind);
      state.reconciliation.observation = acquisitionObservation(kind);
      mutate(state);
      const h = harness(state);
      try {
        await h.app.start();
        assert.match(h.byId("connection-status").textContent, /unconfirmed/);
        assert.equal(h.byId("result").hidden, true);
        assert.equal(h.byId("reconcile").disabled, true);
      } finally { h.close(); }
    }
  }
});

test("direct acquisition check replies cannot cache a foreign host binding", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    for (const field of kind === "compose" ? ["proposal_directory"] : ["receipt_path", "host_profile_sha256"]) {
      const result = acquisitionObservation(kind);
      result.result[field] = field === "receipt_path" ? "/foreign/acquisition.json" : field === "proposal_directory" ? `/foreign/${result.job_id}/proposal` : "c".repeat(64);
      const initial = recoveredAcquisition(kind);
      initial[`${kind}_acquisition`].workspace = "/host/workspace";
      const h = harness(initial, call => call.path === "/api/reconcile" ? response({ observation: result }) : undefined);
      try {
        await h.app.start(); h.byId("reconcile").click(); await settle();
        assert.equal(h.byId("result").hidden, true);
        assert.match(h.byId("problem").textContent, /unconfirmed/);
        h.byId("refresh").click(); await settle();
        assert.equal(h.byId("result").hidden, true);
        assert.doesNotMatch(h.byId("reconciliation-status").textContent, /files match/);
      } finally { h.close(); }
    }
  }
});

test("invalid later acquisition state hides an earlier matched result", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = recoveredAcquisition(kind);
    initial[`${kind}_acquisition`].workspace = "/host/workspace";
    initial.reconciliation.observation = acquisitionObservation(kind);
    const h = harness(initial);
    try {
      await h.app.start(); assert.equal(h.byId("result").hidden, false);
      h.fixture.state.reconciliation.observation.result.receipt_sha256 = "invalid";
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.doesNotMatch(h.byId("reconciliation-status").textContent, /files match/);
    } finally { h.close(); }
  }
});

test("earlier acquisition history cannot regress from a newer unconfirmed check to stale success", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    const initial = acquisitionState(kind, "succeeded"), earlier = clone(initial.job);
    earlier.recovery_request = { id: earlier.id, sha256: "b".repeat(64) };
    initial[`${kind}_acquisition`].workspace = "/host/workspace";
    initial.job = selectedState("python", "succeeded").job;
    initial.job_history = { available: true, count: 2 };
    let observed = acquisitionObservation(kind, "unconfirmed", 2);
    const h = harness(initial, call => {
      if (call.path === "/api/jobs") return response({ jobs: [jobSummary(initial.job), jobSummary(earlier)], next_before: null });
      if (call.path === "/api/job") return response({ job: earlier, reconciliation: { available: true, observation: observed } });
    });
    try {
      await h.app.start(); h.change(h.byId("job-choice"), earlier.id); await settle();
      assert.equal(h.byId("problem").textContent, "");
      assert.equal(h.byId("result").hidden, true);
      observed = acquisitionObservation(kind, "matched", 1);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("problem").textContent, "");
      assert.equal(h.byId("result").hidden, true);
      assert.match(h.byId("reconciliation-status").textContent, /are unconfirmed/);
    } finally { h.close(); }
  }
});

test("cached acquisition observations never cross a changed request or host anchor", async () => {
  for (const kind of Object.keys(acquisitionScopes)) {
    for (const change of [
      state => { state.job.recovery_request.sha256 = "c".repeat(64); },
      state => { state.job.request_id = "9".repeat(32); },
      state => { state[`${kind}_acquisition`].workspace = "/other/workspace"; },
      state => { state[`${kind}_acquisition`].host_profile_sha256 = "c".repeat(64); },
    ]) {
      const initial = recoveredAcquisition(kind);
      initial.reconciliation.observation = acquisitionObservation(kind);
      const h = harness(initial);
      try {
        await h.app.start(); assert.equal(h.byId("result").hidden, false);
        change(h.fixture.state); h.fixture.state.reconciliation.observation = null;
        h.byId("refresh").click(); await settle();
        assert.equal(h.byId("result").hidden, true);
        assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      } finally { h.close(); }
    }
  }
});

test("acquisition stop reply loss is observed without automatically stopping again", async () => {
  const h = harness(acquisitionState("compose"), (call, fixture) => {
    if (call.path === "/api/stop") {
      fixture.state.job.stop = { status: "requested", scope: "local_job_only" };
      throw Error("PRIVATE-CANARY");
    }
  });
  try {
    await h.app.start(); h.byId("stop-job").click(); await settle();
    assert.match(h.byId("stop-status").textContent, /Could not confirm/);
    assert.equal(h.byId("stop-job").disabled, true);
    assert.ok(!h.document.body.textContent.includes("PRIVATE-CANARY"));
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("stop-status").textContent, /Stop requested/);
    assert.equal(h.calls.filter(call => call.path === "/api/stop").length, 1);
    assert.equal(h.byId("stop-job").disabled, true);
  } finally { h.close(); }
});

test("acquisition delayed stop acknowledgement cannot replace a newer current job", async () => {
  const initial = acquisitionState("python"), stopped = clone(initial.job), delayed = deferred();
  stopped.stop = { status: "requested", scope: "local_job_only" };
  const h = harness(initial, call => call.path === "/api/stop" ? delayed.promise : undefined);
  try {
    await h.app.start(); h.byId("stop-job").click(); await settle();
    h.fixture.state.job = selectedState("node", "succeeded").job;
    delayed.resolve(response({ job: stopped })); await settle();
    assert.match(h.byId("job-status").textContent, /Node package installation completed/);
    assert.match(h.byId("result-heading").textContent, /Installed MCP/);
    assert.equal(h.byId("stop-status").textContent, "");
    assert.deepEqual(h.calls.filter(call => call.path === "/api/stop").map(call => call.payload), [{ job_id: stopped.id }]);
  } finally { h.close(); }
});

test("acquisition history detail cannot switch the requested job or show an installation observation", async () => {
  for (const mutate of [
    detail => { detail.job.id = "7".repeat(32); },
    detail => { detail.job.family = "selected-node"; },
    detail => { detail.job = selectedState("python", "succeeded").job; },
    detail => { detail.reconciliation = { available: true, observation: null }; },
    detail => { detail.reconciliation.observation = observation("checking", detail.job.id); },
    detail => { detail.job.result.host_profile_sha256 = "b".repeat(64); },
  ]) {
    const initial = acquisitionState("python", "succeeded"), earlier = clone(initial.job);
    initial.job = selectedState("python", "succeeded").job; initial.job_history = { available: true, count: 2 };
    const detail = { job: clone(earlier), reconciliation: { available: false, observation: null } }; mutate(detail);
    const h = harness(initial, call => {
      if (call.path === "/api/jobs") return response({ jobs: [jobSummary(initial.job), jobSummary(earlier)], next_before: null });
      if (call.path === "/api/job") return response(detail);
    });
    try {
      await h.app.start(); h.change(h.byId("job-choice"), earlier.id); await settle();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("stop-job").disabled, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.doesNotMatch(h.byId("result-heading").textContent, /Retained package inputs/);
    } finally { h.close(); }
  }
});

const selectedRetryState = (kind = "python") => ({ ...selectedState(kind, "unconfirmed"), selected_retry: { available: true } });
const selectedRetryProposal = (kind = "python", software = false) => ({
  scope: "selected_software_inputs_only", retry_of: "1".repeat(32), job_id: null,
  validation_id: "2".repeat(32), effect_sha256: "f".repeat(64), profile: kind,
  package: { publisher: "fixture", name: "chosen", version: "1.0.0" }, component: "selected-server",
  selection_sha256: "a".repeat(64),
  ...(software ? { document_kind: "local_software_declaration", declaration_digest: "b".repeat(64) } : { manifest_digest: "b".repeat(64) }),
  destination: "/workspace/installations/" + "2".repeat(32),
  entrypoint: { executable: kind === "python" ? "chosen.server" : "server.js", arguments: ["--mode", "safe"] },
  mcp_name: "chosen",
  ...(kind === "mcpb" ? { configuration_sha256: "e".repeat(64) } : {}),
  files: { "package.json": "b".repeat(64), [software ? "declaration.json" : "activation.json"]: "a".repeat(64),
    ...(kind === "mcpb" ? { "configuration.json": "e".repeat(64), "artifact.mcpb": "d".repeat(64) } :
      { [kind === "python" ? "artifact.whl" : "artifact.tgz"]: "d".repeat(64), "closure/dependency": "c".repeat(64) }) },
});

test("selected retry reviews Python, Node and MCPB inputs before a separate install", async () => {
  for (const [kind, software] of selectedForms) {
    const proposal = selectedRetryProposal(kind, software);
    const h = harness(selectedRetryState(kind), (call, fixture) => {
      if (call.path === "/api/selected_retry") return response(proposal);
      if (call.path === "/api/selected_install") {
        fixture.state.job = { ...selectedState(kind).job, id: "3".repeat(32), retry_of: "1".repeat(32) };
        return response({ job: fixture.state.job });
      }
    });
    try {
      await h.app.start();
      assert.equal(h.byId("retry-selected").disabled, false);
      assert.equal(h.byId("selected-retry-review").hidden, true);
      h.byId("retry-selected").click(); h.byId("retry-selected").click(); await settle();
      assert.deepEqual(h.calls.filter(c => c.path === "/api/selected_retry").map(c => c.payload), [{ job_id: proposal.retry_of }]);
      assert.equal(h.calls.filter(c => c.path === "/api/selected_install").length, 0);
      assert.equal(h.byId("selected-retry-review").hidden, false);
      for (const literal of ["fixture/chosen@1.0.0", proposal.destination, JSON.stringify(proposal.entrypoint.arguments), proposal.effect_sha256])
        assert.ok(h.byId("selected-retry-values").textContent.includes(literal));
      assert.ok(h.byId("selected-retry-values").textContent.includes(`${software ? "Software declaration digest" : "Manifest digest"}${"b".repeat(64)}`));
      assert.doesNotMatch(h.byId("selected-retry-values").textContent, /undefined/);
      for (const name of Object.keys(proposal.files)) assert.ok(h.byId("selected-retry-files").textContent.includes(name));
      assert.equal(h.document.activeElement.id, "selected-retry-heading");
      assert.equal(h.byId("install-selected-retry").disabled, false);
      h.byId("install-selected-retry").click(); h.byId("install-selected-retry").click(); await settle();
      assert.deepEqual(h.calls.filter(c => c.path === "/api/selected_install").map(c => c.payload),
        [{ validation_id: proposal.validation_id, effect_sha256: proposal.effect_sha256 }]);
      assert.match(h.byId("job-context").textContent, /New installation after attempt 1{32}/);
      assert.equal(h.byId("selected-retry-review").hidden, true);
      assert.match(h.byId("job-status").textContent, /running/);
    } finally { h.close(); }
  }
});

test("selected retry refuses contradictory or malformed proposal fields", async () => {
  const mutations = [
    p => { p.retry_of = "4".repeat(32); }, p => { p.profile = "node"; },
    p => { p.files["activation.json"] = "f".repeat(64); }, p => { p.files["configuration.json"] = "f".repeat(64); },
    p => { delete p.files["artifact.mcpb"]; p.files["elsewhere"] = "c".repeat(64); },
    p => { p.files["closure/../escape"] = "c".repeat(64); }, p => { p.destination = "/wrong"; },
    p => { p.job_id = p.retry_of; }, p => { p.entrypoint.arguments = "unsafe"; },
  ];
  for (const mutate of mutations) {
    const proposal = selectedRetryProposal("mcpb"); mutate(proposal);
    const h = harness(selectedRetryState("mcpb"), c => c.path === "/api/selected_retry" ? response(proposal) : undefined);
    try {
      await h.app.start(); h.byId("retry-selected").click(); await settle();
      assert.equal(h.byId("selected-retry-review").hidden, true);
      assert.equal(h.byId("install-selected-retry").disabled, true);
      assert.match(h.byId("problem").textContent, /Could not confirm/);
      h.byId("install-selected-retry").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/selected_install").length, 0);
    } finally { h.close(); }
  }
});

test("selected retry binds the exclusive document form to its exact raw input file", async () => {
  for (const [kind, software] of selectedForms) {
    const mutations = [
      p => { p.document_kind = null; },
      p => { p.document_kind = "activation_manifest"; },
      p => { p[software ? "manifest_digest" : "declaration_digest"] = "b".repeat(64); },
      p => { delete p[software ? "declaration_digest" : "manifest_digest"]; },
      p => { p[software ? "declaration_digest" : "manifest_digest"] = "B".repeat(64); },
      p => { p.files[software ? "declaration.json" : "activation.json"] = "f".repeat(64); },
      p => { p.files[software ? "activation.json" : "declaration.json"] = p.selection_sha256; },
      p => {
        delete p.files[software ? "declaration.json" : "activation.json"];
        p.files[software ? "activation.json" : "declaration.json"] = p.selection_sha256;
      },
      ...(software ? [p => { delete p.document_kind; }] : []),
      ...(kind === "mcpb" ? [p => {
        delete p.manifest_digest; p.document_kind = "local_software_declaration"; p.declaration_digest = "b".repeat(64);
        delete p.files["activation.json"]; p.files["declaration.json"] = p.selection_sha256;
      }] : []),
    ];
    for (const mutate of mutations) {
      const proposal = selectedRetryProposal(kind, software); mutate(proposal);
      const h = harness(selectedRetryState(kind), call => call.path === "/api/selected_retry" ? response(proposal) : undefined);
      try {
        await h.app.start(); h.byId("retry-selected").click(); await settle();
        assert.equal(h.byId("selected-retry-review").hidden, true);
        assert.equal(h.byId("install-selected-retry").disabled, true);
        assert.match(h.byId("problem").textContent, /Could not confirm/);
        h.byId("install-selected-retry").click(); await settle();
        assert.equal(h.calls.filter(call => call.path === "/api/selected_install").length, 0);
      } finally { h.close(); }
    }
  }
});

test("selected retry requires capability and settled writable local job", async () => {
  for (const change of [s => { delete s.selected_retry; }, s => { s.selected_retry.available = false; },
    s => { s.job.status = "running"; }, s => { s.job = selectedState("python", "succeeded").job; },
    s => { s.job.recovered = true; }, s => { s.mode = "recovery"; }, s => { s.operation_in_progress = true; }]) {
    const initial = selectedRetryState(); change(initial);
    const h = harness(initial);
    try {
      await h.app.start(); assert.equal(h.byId("retry-selected").disabled, true);
      h.byId("retry-selected").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/selected_retry").length, 0);
    } finally { h.close(); }
  }
});

test("lost selected proposal reply permits only explicit proposal retrieval", async () => {
  let attempts = 0;
  const h = harness(selectedRetryState(), call => {
    if (call.path !== "/api/selected_retry") return;
    if (++attempts === 1) throw Error("private details");
    return response(selectedRetryProposal());
  });
  try {
    await h.app.start(); h.byId("retry-selected").click(); await settle();
    assert.equal(h.byId("selected-retry-review").hidden, true);
    assert.ok(!h.byId("problem").textContent.includes("private details"));
    h.byId("refresh").click(); await settle(); assert.equal(attempts, 1);
    h.byId("retry-selected").click(); await settle(); assert.equal(attempts, 2);
    assert.equal(h.byId("install-selected-retry").disabled, false);
    assert.equal(h.calls.filter(c => c.path === "/api/selected_install").length, 0);
  } finally { h.close(); }
});

test("lost selected installation reply is retrieved without reinstalling", async () => {
  for (const [kind, software] of selectedForms) {
    const proposal = selectedRetryProposal(kind, software);
    const h = harness(selectedRetryState(kind), call => {
      if (call.path === "/api/selected_retry") return response(proposal);
      if (call.path === "/api/selected_install") { proposal.job_id = "3".repeat(32); throw Error("lost reply"); }
    });
    try {
      await h.app.start(); h.byId("retry-selected").click(); await settle();
      h.byId("install-selected-retry").click(); await settle();
      assert.equal(h.byId("install-selected-retry").disabled, true);
      assert.equal(h.byId("selected-retry-review").hidden, true);
      h.byId("retry-selected").click(); await settle();
      assert.equal(h.byId("selected-retry-review").hidden, false);
      assert.equal(h.byId("install-selected-retry").disabled, true);
      assert.match(h.byId("retry-selected-status").textContent, /already has installation job 3{32}/);
      assert.equal(h.calls.filter(c => c.path === "/api/selected_install").length, 1);
    } finally { h.close(); }
  }
});

test("selected proposal becomes non-actionable while host status is unknown", async () => {
  let unavailable = false;
  const h = harness(selectedRetryState(), call => {
    if (call.path === "/api/selected_retry") return response(selectedRetryProposal());
    if (call.path === "/api/state" && unavailable) throw Error("offline");
  });
  try {
    await h.app.start(); unavailable = true;
    h.byId("retry-selected").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("install-selected-retry").disabled, true);
    assert.equal(h.calls.filter(c => c.path === "/api/selected_install").length, 0);
  } finally { h.close(); }
});

test("late selected proposal cannot replace a different history selection", async () => {
  const first = selectedRetryState();
  const other = { ...selectedState("node", "succeeded").job, id: "4".repeat(32) };
  first.selected_installation.profiles = ["python", "node"];
  first.job_history = { available: true, count: 2 };
  const delayed = deferred();
  const h = harness(first, call => {
    if (call.path === "/api/selected_retry") return delayed.promise;
    if (call.path === "/api/jobs") return response({ jobs: [jobSummary(first.job), jobSummary(other)], next_before: null });
    if (call.path === "/api/job") return response({ job: call.payload.job_id === other.id ? other : first.job,
      reconciliation: { available: true, observation: null } });
  });
  try {
    await h.app.start(); h.byId("retry-selected").click(); await settle();
    h.change(h.byId("job-choice"), other.id); await settle();
    delayed.resolve(response(selectedRetryProposal())); await settle();
    assert.equal(h.byId("job-choice").value, other.id);
    assert.equal(h.byId("selected-retry-review").hidden, true);
    assert.ok(!h.byId("job-status").textContent.includes("Python"));
    h.change(h.byId("job-choice"), first.job.id); await settle();
    assert.equal(h.byId("selected-retry-review").hidden, false);
    assert.equal(h.byId("install-selected-retry").disabled, false);
  } finally { h.close(); }
});

test("selected retry reply after disposal cannot publish a proposal", async () => {
  const delayed = deferred();
  const h = harness(selectedRetryState(), c => c.path === "/api/selected_retry" ? delayed.promise : undefined);
  try {
    await h.app.start(); h.byId("retry-selected").click(); await settle(); h.app.dispose();
    delayed.resolve(response(selectedRetryProposal())); await settle();
    assert.equal(h.byId("selected-retry-review").hidden, true);
    assert.equal(h.calls.filter(c => c.path === "/api/selected_install").length, 0);
  } finally { h.close(); }
});

function harness(initial = stateValue(), handler = () => undefined, hash = "#session=fixture-capability") {
  const dom = new JSDOM(html, { url: "http://127.0.0.1:49231/" + hash });
  const { document } = dom.window;
  const calls = [], timers = new Map(), timerDelays = new Map(); let nextTimer = 0;
  const fixture = { state: clone(initial), help: helpValue() };
  const fetcher = async (path, init) => {
    const call = { path, init, payload: JSON.parse(init.body) }; calls.push(call);
    const handled = await handler(call, fixture);
    if (handled !== undefined) return handled;
    if (path === "/api/state") return response(fixture.state);
    if (path === "/api/help") return response(fixture.help);
    if (path === "/api/check") return response(validationValue());
    if (path === "/api/install") { fixture.state.job = job("running"); return response({ job: fixture.state.job }); }
    if (path === "/api/prepare") { fixture.state.families[0].status = "preparing"; fixture.state.job = job("running", "prepare"); return response({ job: fixture.state.job }); }
    throw new Error("unexpected fixture request");
  };
  const app = createWorkbench(document, { fetch: fetcher,
    setTimeout: (callback, delay) => { const id = ++nextTimer; timers.set(id, callback); timerDelays.set(id, delay); return id; },
    clearTimeout: id => { timers.delete(id); timerDelays.delete(id); },
  });
  const byId = id => document.getElementById(id);
  const change = (element, value, event = "change") => { element.value = value; element.dispatchEvent(new dom.window.Event(event, { bubbles: true })); };
  const checked = (index, value = true) => { const input = byId("operations").querySelectorAll("input")[index]; input.checked = value; input.dispatchEvent(new dom.window.Event("change", { bubbles: true })); };
  const field = name => byId("fields").querySelector(`[name="${name}"]`);
  const submit = async () => { byId("configuration-form").dispatchEvent(new dom.window.Event("submit", { bubbles: true, cancelable: true })); await settle(); };
  async function choose() { change(byId("family"), "agno"); change(byId("profile"), profile); byId("show-profile").focus(); byId("show-profile").click(); await settle(); }
  async function fill() { await choose(); checked(0); change(field("host"), "https://127.0.0.1:49352", "input"); change(field("timeout"), "10", "input"); }
  const close = () => { app.dispose(); dom.window.close(); };
  return { app, byId, field, change, checked, submit, choose, fill, calls, fixture, timers, timerDelays, document, dom, close };
}

const responseDeadlines = h => [...h.timers].filter(([id]) => [30000, 180000].includes(h.timerDelays.get(id)));
async function expireResponses(h) {
  for (const [id, callback] of responseDeadlines(h)) {
    h.timers.delete(id); h.timerDelays.delete(id); callback();
  }
  await settle();
}

for (const phase of ["fetch", "body"]) {
  test(`response deadline releases held preparation ${phase} for exact owner refresh and Stop`, async () => {
    const gate = deferred(), admitted = job("running", "prepare", "a".repeat(32));
    const h = harness({ ...stateValue("unprepared"), stop: { available: true } }, (call, fixture) => {
      if (call.path === "/api/prepare") {
        fixture.state.job = clone(admitted); fixture.state.families[0].status = "preparing";
        return phase === "fetch" ? gate.promise : { ok: true, status: 200, json: () => gate.promise };
      }
      if (call.path === "/api/stop") {
        assert.deepEqual(call.payload, { job_id: admitted.id });
        fixture.state.job = { ...admitted, status: "unconfirmed", stop: { status: "settled", scope: "local_job_only" } };
        fixture.state.families[0].status = "unconfirmed";
        return response({ job: fixture.state.job });
      }
    });
    try {
      await h.app.start(); h.change(h.byId("family"), "agno"); h.byId("prepare").click(); await settle();
      assert.equal(h.timerDelays.get(responseDeadlines(h)[0]?.[0]), 30000);
      h.byId("refresh").click(); await settle();
      assert.match(h.byId("job-status").textContent, /running/);
      assert.equal(h.byId("stop-job").disabled, true);
      await expireResponses(h);
      assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 1);
      assert.equal(h.calls.filter(c => c.path === "/api/stop").length, 0);
      h.byId("refresh").click(); await settle();
      assert.match(h.byId("connection-status").textContent, /Connected/);
      assert.equal(h.byId("stop-job").disabled, false);
      h.byId("stop-job").click(); await settle();
      assert.match(h.byId("stop-status").textContent, /Local setup stopped/);
      const observed = h.document.body.textContent;
      gate.resolve(phase === "fetch" ? response({ job: admitted }) : { job: admitted }); await settle();
      assert.equal(h.document.body.textContent, observed);
      assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 1);
      assert.equal(h.calls.filter(c => c.path === "/api/stop").length, 1);
      assert.ok(h.calls.every(c => !Object.hasOwn(c.init, "signal")));
      assert.equal(responseDeadlines(h).length, 0);
    } finally { h.close(); }
  });
}

test("response deadline uses one observation budget across fetch and JSON body", async () => {
  const headers = deferred(), body = deferred();
  const h = harness(stateValue(), call => call.path === "/api/state" ? headers.promise : undefined);
  try {
    const started = h.app.start(); await settle();
    const initial = responseDeadlines(h); assert.equal(initial.length, 1);
    assert.equal(h.timerDelays.get(initial[0][0]), 30000);
    headers.resolve({ ok: true, status: 200, json: () => body.promise }); await settle();
    assert.deepEqual(responseDeadlines(h).map(([id]) => id), initial.map(([id]) => id));
    await expireResponses(h); await started;
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("family").disabled, true);
    assert.equal(h.timers.size, 0);
    body.resolve(stateValue()); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.calls.length, 1);
  } finally { h.close(); }
});

test("response deadline disposal clears waits without cancelling admitted work", async () => {
  const gate = deferred(); let rejectBody;
  const body = new Promise((_, reject) => { rejectBody = reject; });
  const h = harness({ ...stateValue("unprepared"), stop: { available: true } }, (call, fixture) => {
    if (call.path === "/api/prepare") {
      fixture.state.job = job("running", "prepare", "a".repeat(32));
      return { ok: true, status: 200, json: () => body };
    }
    if (call.path === "/api/state" && fixture.state.job) return gate.promise;
  });
  try {
    await h.app.start(); h.change(h.byId("family"), "agno"); h.byId("prepare").click(); await settle();
    h.byId("refresh").click(); await settle();
    assert.equal(responseDeadlines(h).length, 2);
    const count = h.calls.length, observed = h.document.body.textContent;
    h.app.dispose(); await settle();
    assert.equal(h.timers.size, 0);
    gate.resolve(response(h.fixture.state)); rejectBody(new Error("PRIVATE-LATE-BODY")); await settle();
    assert.equal(h.document.body.textContent, observed);
    assert.equal(h.calls.length, count);
    assert.equal(h.calls.filter(c => c.path === "/api/stop").length, 0);
    assert.ok(h.calls.every(c => !Object.hasOwn(c.init, "signal")));
    assert.equal(h.fixture.state.job.status, "running");
  } finally { h.close(); }
});

test("response deadline normal completion and refusal clear their observation timers", async () => {
  let refuse = false;
  const h = harness(stateValue(), call => refuse && call.path === "/api/state" ? response({}, 403) : undefined);
  try {
    await h.app.start(); assert.equal(responseDeadlines(h).length, 0); assert.equal(h.timers.size, 1);
    refuse = true; h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.timers.size, 0);
  } finally { h.close(); }
});

for (const kind of ["validation", "MCPB reconciliation"]) {
  test(`response deadline retains the long budget for ${kind}`, async () => {
    const gate = deferred(), checking = kind === "validation";
    const h = harness(checking ? stateValue() : selectedState("mcpb", "succeeded"), call =>
      call.path === (checking ? "/api/check" : "/api/reconcile") ? gate.promise : undefined);
    try {
      await h.app.start();
      if (checking) { await h.fill(); await h.submit(); }
      else { h.byId("reconcile").click(); await settle(); }
      const deadlines = responseDeadlines(h); assert.equal(deadlines.length, 1);
      assert.equal(h.timerDelays.get(deadlines[0][0]), 180000);
      gate.resolve(response(checking ? validationValue() : { observation: selectedObservation("mcpb") })); await settle();
      assert.equal(responseDeadlines(h).length, 0);
      assert.equal(h.byId(checking ? "review" : "result").hidden, false);
    } finally { h.close(); }
  });
}

const jobSummary = value => ({ id: value.id, kind: value.kind, family: value.family, status: value.status,
  profile: value.result?.profile ?? null,
  ...(value.kind === "compose" ? { action: value.action, request_id: value.request_id, stop_supported: value.stop_supported } : {}) });

const composeState = (action = "start", status = "running") => {
  const plan = "a".repeat(64);
  return {
    families: [], mode: "setup", scope: "Host-selected local container execution",
    stop: { available: true }, reconciliation: { available: false, observation: null },
    compose: { available: true, scope: "execute_agent_selected_local_container_plan", mode: "new",
      actions: ["prepare", "observe", "start", "resume", "stop", "remove"],
      graph_workspace: "/host/graph", staging_root: "/host/staging", host_profile_sha256: "b".repeat(64),
      backend_profile_sha256: "c".repeat(64), plan_sha256: plan, execution_uncertain: false },
    job: { id: "1".repeat(32), kind: "compose", family: "compose", action,
      request_id: "2".repeat(32), stop_supported: false, status,
      ...(status === "running" ? {} : { result: {
        scope: "compose_backend_observation_only", action,
        backend: action === "prepare" ? { schema_version: 1, status: "prepared", plan_sha256: plan,
          summary: { request_sha256: "d".repeat(64), package_sha256: "d".repeat(64), manifest_sha256: "d".repeat(64),
            manifest_source_sha256: "d".repeat(64), profile_sha256: "c".repeat(64), configuration_sha256: "d".repeat(64),
            policy_sha256: "d".repeat(64), host_owner: "fixture-host", project: "fixture", request_components: ["reader"],
            initialization_order: ["reader"], shutdown_order: ["reader"], deployment_order: [],
            components: { reader: { service: "reader", lifetime: "request", image: "sha256:" + "e".repeat(64),
              image_id: "sha256:" + "e".repeat(64), platform: "linux/arm64", dependencies: [], deployment_dependencies: [],
              readiness: "running", stop_timeout_ms: 30000 } }, files: { "inputs/configuration": { sha256: "d".repeat(64), mode: 292 } } } } :
          { status: status === "succeeded" ? "observed" : "unconfirmed", plan_sha256: plan,
            observation: status === "succeeded" ? { plan_sha256: plan, observation_sha256: "d".repeat(64),
              available: { database: true }, pending_operation: false } : null },
        runtime_bindings: status === "unconfirmed" ? [] : [{ scope: "compose_backend_stdio", component: "reader", plan_sha256: plan,
          command: "/host/python", args: ["-I", "-m", "meerkat_toolkit.compose", "--workspace", "/host/graph", "run-stdio"],
          env: {}, runtime_sha256: "e".repeat(64), planner_sha256: "f".repeat(64) }],
      } }),
    },
  };
};

// Real Controller/ComposeBridge outputs over the existing inert subprocess
// fixture. Capture/provenance: docs/verification/20261005-compose-successor-browser/author/.
const successorSnapshots = JSON.parse(await readFile(new URL("./fixtures/compose-successor-browser.json", import.meta.url), "utf8"));
function successorHarness(snapshot = successorSnapshots.idle) {
  const h = harness(snapshot.state, call => {
    if (call.path === "/api/jobs") return response(snapshot.jobs);
    if (call.path === "/api/job") return response(snapshot.details[call.payload.job_id]);
  });
  return h;
}

test("Compose successor idle host remains connected and observation-only", async () => {
  const h = successorHarness();
  try {
    await h.app.start();
    assert.match(h.byId("connection-status").textContent, /Connected/);
    assert.equal(h.byId("job-panel").hidden, true);
    assert.deepEqual(h.calls.map(call => call.path), ["/api/state", "/api/jobs"]);
  } finally { h.close(); }
});

test("Compose successor owner phases expose the existing setup-tool step without dispatch", async () => {
  const messages = { idle: /inputs are pinned.*setup tools/, predecessor: /inputs are pinned/, prepared: /plan is prepared.*fresh observation/,
    running: /Adopt successor plan is running.*Refresh status.*no automatic retry/, pending: /pending: activating.*reconcile.*No current runtime bindings/, selected: /explicit rebind.*No current runtime bindings/,
    rebound: /explicitly bound.*does not establish application readiness/, ordinary_pending: /operation is pending.*No current runtime bindings/,
    outer_uncertain: /execution is uncertain.*quiescent custody.*no automatic retry/ };
  for (const [name, snapshot] of Object.entries(successorSnapshots)) {
    const h = successorHarness(snapshot);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /Connected/, name);
      assert.match(h.byId("connection-status").textContent, messages[name], name);
      assert.equal(h.byId("connection-status").getAttribute("role"), "status");
      assert.equal(h.byId("connection-status").getAttribute("aria-live"), "polite");
      if (snapshot.state.job) {
        assert.equal(h.byId("stop-job").disabled, true);
        assert.equal(h.byId("reconcile").hidden, true);
        if (snapshot.state.job.result) assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent), {
          backend: snapshot.state.job.result.backend, runtime_bindings: snapshot.state.job.result.runtime_bindings });
      }
      h.byId("refresh").click(); await settle();
      assert.match(h.byId("connection-status").textContent, /Connected/, name);
      assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
      assert.doesNotMatch(h.document.body.textContent, /PRIVATE_SUCCESSOR|PRIVATE_COMMAND|PRIVATE_ENV|PRIVATE_COMPOSE/);
    } finally { h.close(); }
  }
});

test("Compose successor rebind keeps exact predecessor history distinct from current selected plan", async () => {
  const snapshot = successorSnapshots.rebound, earlier = successorSnapshots.predecessor.state.job;
  const h = successorHarness(snapshot);
  try {
    await h.app.start();
    h.change(h.byId("job-choice"), earlier.id); await settle();
    assert.match(h.byId("connection-status").textContent, /Connected/);
    assert.equal(JSON.parse(h.byId("mcp-configuration").textContent).backend.plan_sha256, earlier.result.backend.plan_sha256);
    assert.match(h.byId("result-paths").textContent, new RegExp(`Result plan SHA-256${earlier.result.backend.plan_sha256}`));
    assert.match(h.byId("result-paths").textContent, new RegExp(`Host-admitted plan SHA-256${snapshot.state.compose.plan_sha256}`));
    assert.match(h.byId("result-limits").textContent, /retained action results.*does not grant current authority/);
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("job-choice").value, earlier.id);
    assert.equal(h.byId("stop-job").disabled, true);
    assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
  } finally { h.close(); }
});

test("Compose successor malformed target transition result and raw metadata refuse without reflection", async () => {
  const mutations = [
    s => { s.state.compose.successor.extra = "PRIVATE_EXTRA"; },
    s => { delete s.state.compose.pending_operation; },
    s => { s.state.compose.pending_operation = "false"; },
    s => { s.state.compose.rebind_required = true; },
    s => { s.state.compose.successor.backend_profile_sha256 = "0".repeat(64); },
    s => { s.state.compose.successor.transition_sha256 = "0".repeat(64); },
    s => { s.state.job.result.backend.selected.summary.files.secret = { contents: "PRIVATE_EXTRA", sha256: "0".repeat(64), mode: 292 }; },
    s => { s.state.job.result.backend.selected.plan_sha256 = "0".repeat(64); },
    s => { s.state.job.result.runtime_bindings[0].plan_sha256 = s.state.compose.successor.predecessor_plan_sha256; },
  ];
  for (const mutate of mutations) {
    const snapshot = clone(successorSnapshots.rebound); mutate(snapshot);
    const h = successorHarness(snapshot);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result").hidden, true);
      assert.doesNotMatch(h.document.body.textContent, /PRIVATE_EXTRA/);
      assert.deepEqual(h.calls.map(call => call.path), ["/api/state"]);
    } finally { h.close(); }
  }
});

test("Compose successor retained result cannot change from A to B under the same job identity", async () => {
  const snapshot = clone(successorSnapshots.rebound), earlier = successorSnapshots.predecessor.state.job;
  const h = successorHarness(snapshot);
  try {
    await h.app.start(); h.change(h.byId("job-choice"), earlier.id); await settle();
    assert.match(h.byId("connection-status").textContent, /Connected/);
    const result = snapshot.details[earlier.id].job.result, target = snapshot.state.compose.plan_sha256;
    result.backend.plan_sha256 = target; result.backend.observation.plan_sha256 = target;
    result.runtime_bindings.forEach(binding => { binding.plan_sha256 = target; });
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("result").hidden, true);
  } finally { h.close(); }
});

// Project the new closed version/phase onto retained real bridge snapshots.
// The subprocess bridge tests separately exercise these fields at their source.
function migrationSnapshot() {
  const snapshot = clone(successorSnapshots.pending), host = snapshot.state.compose;
  host.pending_transition.phase = "migrate";
  host.selected_plan_sha256 = host.successor.predecessor_plan_sha256;
  host.rebind_required = false;
  snapshot.state.job.result.backend.plan_sha256 = host.selected_plan_sha256;
  snapshot.details[snapshot.state.job.id].job = clone(snapshot.state.job);
  for (const { job } of Object.values(snapshot.details)) {
    if (job.action === "prepare_successor") job.result.backend.schema_version = 2;
  }
  return snapshot;
}

test("Compose migration pending preserves A and offers observation without bindings or dispatch", async () => {
  const snapshot = migrationSnapshot(), h = successorHarness(snapshot);
  try {
    await h.app.start();
    assert.match(h.byId("connection-status").textContent, /Connected.*waiting for confirmed state migration.*explicitly reconcile.*No current runtime bindings/);
    assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent).runtime_bindings, []);
    assert.equal(JSON.parse(h.byId("mcp-configuration").textContent).backend.plan_sha256, snapshot.state.compose.plan_sha256);
    assert.equal(h.byId("stop-job").disabled, true);
    assert.equal(h.byId("reconcile").hidden, true);
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /Connected.*waiting for confirmed state migration/);
    assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
    // A stale/invalid current projection cannot promote B while migrate is pending.
    h.fixture.state.compose.selected_plan_sha256 = snapshot.state.compose.successor.successor_plan_sha256;
    h.fixture.state.compose.rebind_required = true;
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("result").hidden, true);
  } finally { h.close(); }
});

test("Compose successor schema 2 remains prepared and keeps closed summary validation", async () => {
  const prepared = clone(successorSnapshots.prepared);
  prepared.state.job.result.backend.schema_version = 2;
  prepared.details[prepared.state.job.id].job = clone(prepared.state.job);
  const h = successorHarness(prepared);
  try {
    await h.app.start();
    assert.match(h.byId("connection-status").textContent, /Connected.*plan is prepared.*fresh observation/);
    assert.doesNotMatch(h.byId("connection-status").textContent, /migration|ready/);
    assert.equal(JSON.parse(h.byId("mcp-configuration").textContent).backend.schema_version, 2);
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /Connected/);
    assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
  } finally { h.close(); }
  for (const mutate of [
    s => { s.state.job.result.backend.schema_version = 3; },
    s => { s.state.job.result.backend.schema_version = true; },
    s => { s.state.job.result.backend.successor.schema_version = 2; },
    s => { s.state.job.result.backend.receipt = "PRIVATE_RECEIPT"; },
    s => { s.state.job.result.backend.successor.summary.state_migration = "PRIVATE_COMMAND"; },
  ]) {
    const snapshot = clone(prepared); mutate(snapshot);
    const invalid = successorHarness(snapshot);
    try {
      await invalid.app.start();
      assert.match(invalid.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(invalid.byId("result").hidden, true);
      assert.doesNotMatch(invalid.document.body.textContent, /PRIVATE_RECEIPT|PRIVATE_COMMAND/);
      assert.deepEqual(invalid.calls.map(call => call.path), ["/api/state"]);
    } finally { invalid.close(); }
  }
});

test("Compose running actions visibly disable local stop and never dispatch it", async () => {
  for (const [action, label] of [["prepare", "Prepare container plan"], ["start", "Start deployments"],
    ["resume", "Resume deployments"], ["stop", "Stop deployments"], ["remove", "Remove deployment resources"]]) {
    const h = harness(composeState(action));
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /Connected/);
      assert.equal(h.byId("job-status").textContent, `${label} is running in the local host.`);
      assert.equal(h.byId("stop-job").hidden, false);
      assert.equal(h.byId("stop-job").disabled, true);
      assert.match(h.byId("stop-status").textContent, /separate explicit backend action/);
      assert.equal(h.byId("reconcile").hidden, true);
      h.byId("stop-job").dispatchEvent(new h.dom.window.Event("click")); await settle();
      assert.ok(h.calls.every(call => call.path === "/api/state"));
      assert.match(h.document.querySelector("footer").textContent, /host-authorized container execution/);
      assert.match(h.document.querySelector(".page-header > p").textContent, /permits agent-selected local container execution/);
      assert.equal(h.byId("family").disabled, true);
    } finally { h.close(); }
  }
});

test("Compose results distinguish prepared observed and unconfirmed without installation claims", async () => {
  for (const [action, status, backendStatus] of [["prepare", "succeeded", "prepared"],
    ["start", "succeeded", "observed"], ["stop", "unconfirmed", "unconfirmed"], ["remove", "succeeded", "observed"]]) {
    const state = composeState(action, status), h = harness(state);
    try {
      await h.app.start();
      assert.ok(h.byId("job-status").textContent.endsWith(`backend ${backendStatus}.`));
      assert.equal(h.byId("result").hidden, false);
      assert.equal(h.byId("result-heading").textContent, "Compose backend result and runtime bindings");
      assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent), {
        backend: state.job.result.backend, runtime_bindings: state.job.result.runtime_bindings,
      });
      assert.match(h.byId("mcp-configuration").getAttribute("aria-label"), /Compose/);
      assert.match(h.byId("result-limits").textContent, /not an installed MCP configuration/);
      assert.equal(h.byId("result-credential-references").textContent, "");
      assert.doesNotMatch(h.byId("result-paths").textContent, /Installation receipt|Validated profile/);
      assert.equal(h.byId("reconcile").hidden, true);
    } finally { h.close(); }
  }
});

test("Compose unavailable execution preserves uncertainty without inventing a backend result", async () => {
  const state = composeState("start", "unconfirmed"); delete state.job.result;
  state.compose.execution_uncertain = true;
  const h = harness(state);
  try {
    await h.app.start();
    assert.match(h.byId("job-status").textContent, /No backend result was confirmed/);
    assert.match(h.byId("job-context").textContent, /quiescent custody/);
    assert.equal(h.byId("result").hidden, true);
    assert.equal(h.byId("stop-job").disabled, true);
    h.byId("refresh").click(); await settle();
    assert.ok(h.calls.every(call => call.path === "/api/state"));
  } finally { h.close(); }
});

test("Compose malformed scopes bindings statuses and capabilities fail closed", async () => {
  const changes = [
    s => { delete s.compose; }, s => { s.compose.scope = "installation_only"; },
    s => { s.compose.plan_sha256 = "0".repeat(64); }, s => { s.compose.mode = "guessed"; },
    s => { s.job.stop_supported = true; }, s => { delete s.job.stop_supported; },
    s => { s.job.stop = { status: "settled", scope: "local_job_only" }; },
    s => { s.job.family = "agno"; }, s => { s.job.action = "pull"; },
    s => { s.job.request_id = "not-an-id"; }, s => { s.job.result.scope = "native_ready"; },
    s => { s.job.result.action = "remove"; }, s => { s.job.result.backend.status = "unconfirmed"; },
    s => { s.job.status = "unconfirmed"; }, s => { s.job.result.backend.observation.plan_sha256 = "0".repeat(64); },
    s => { s.job.result.backend.observation = null; },
    s => { s.compose.plan_sha256 = null; },
    s => { s.job.result.runtime_bindings[0].plan_sha256 = "0".repeat(64); },
    s => { s.job.result.runtime_bindings[0].env = { TOKEN: "must-not-display" }; },
    s => { s.job.result.runtime_bindings[0].scope = "mcp_ready"; },
  ];
  for (const mutate of changes) {
    const state = composeState("start", "succeeded"); mutate(state);
    const h = harness(state);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result").hidden, true);
      assert.ok(!h.document.documentElement.textContent.includes("must-not-display"));
      assert.ok(h.calls.every(call => call.path === "/api/state"));
    } finally { h.close(); }
  }
});

test("Compose history preserves the action and result while newer work runs", async () => {
  const earlier = composeState("prepare", "succeeded").job, state = composeState("start");
  state.job.id = "3".repeat(32); state.job.request_id = "4".repeat(32);
  state.job_history = { available: true, count: 2 };
  const h = harness(state, call => {
    if (call.path === "/api/jobs") return response({ jobs: [jobSummary(state.job), jobSummary(earlier)], next_before: null });
    if (call.path === "/api/job") return response({ job: earlier, reconciliation: { available: false, observation: null } });
  });
  try {
    await h.app.start();
    assert.match(h.byId("job-choice").textContent, /Prepare container plan/);
    h.change(h.byId("job-choice"), earlier.id); await settle();
    assert.equal(h.byId("job-status").textContent, "Prepare container plan: backend prepared.");
    assert.match(h.byId("current-job-status").textContent, /Start deployments, running/);
    assert.equal(h.byId("stop-job").disabled, true);
    assert.equal(h.byId("result").hidden, false);
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("job-choice").value, earlier.id);
    assert.equal(h.byId("job-status").textContent, "Prepare container plan: backend prepared.");
  } finally { h.close(); }
});

test("Compose historical detail rejects changed action request and plan identities", async () => {
  for (const mutate of [j => { j.action = "resume"; j.result.action = "resume"; },
    j => { j.request_id = "5".repeat(32); }, j => { j.result.backend.plan_sha256 = "5".repeat(64); }]) {
    const earlier = composeState("start", "succeeded").job, state = composeState("stop");
    state.job.id = "3".repeat(32); state.job_history = { available: true, count: 2 };
    const changed = clone(earlier); mutate(changed);
    const h = harness(state, call => {
      if (call.path === "/api/jobs") return response({ jobs: [jobSummary(state.job), jobSummary(earlier)], next_before: null });
      if (call.path === "/api/job") return response({ job: changed, reconciliation: { available: false, observation: null } });
    });
    try {
      await h.app.start(); h.change(h.byId("job-choice"), earlier.id); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    } finally { h.close(); }
  }
});

test("Compose refresh clears stale results and rejects same-job action rebinding", async () => {
  for (const mode of ["failed-read", "action", "plan"]) {
    let fail = false;
    const h = harness(composeState("start", "succeeded"), call => {
      if (fail && call.path === "/api/state") throw Error("private host details");
    });
    try {
      await h.app.start(); assert.equal(h.byId("result").hidden, false);
      if (mode === "failed-read") fail = true;
      if (mode === "action") { h.fixture.state.job.action = "resume"; h.fixture.state.job.result.action = "resume"; }
      if (mode === "plan") h.fixture.state.compose.plan_sha256 = "5".repeat(64);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("mcp-configuration").textContent, "");
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.ok(!h.document.documentElement.textContent.includes("private host details"));
      assert.ok(h.calls.every(call => call.path === "/api/state"));
    } finally { h.close(); }
  }
});

test("Compose prepared metadata rejects payload-bearing replies in current and historical views", async () => {
  const canary = "PRIVATE_STAGED_SQL_CONFIG_COMMAND_CANARY";
  const mutations = [
    result => { result.backend.plan = { files: { seed: { contents: canary } } }; },
    result => { result.backend.summary.compose_json = canary; },
    result => { result.backend.summary.engine = { command: canary }; },
    result => { result.backend.summary.files["inputs/configuration"].contents = canary; },
    result => { result.backend.summary.components.reader.command = [canary]; },
    result => { result.backend.summary.components.reader.entrypoint = [canary]; },
    result => { result.backend.summary.components.reader.environment = { PASSWORD: canary }; },
    result => { result.backend.summary.components.reader.mounts = [canary]; },
    result => { result.raw_request = canary; },
    result => { result.runtime_bindings[0].contents = canary; },
    result => { result.backend.summary.configuration_sha256 = canary; },
    result => { result.backend.summary.profile_sha256 = "0".repeat(64); },
  ];
  for (const historical of [false, true]) for (const mutate of mutations) {
    const prepared = composeState("prepare", "succeeded"), earlier = clone(prepared.job);
    mutate(earlier.result);
    const state = historical ? composeState("start") : { ...prepared, job: earlier };
    if (historical) { state.job.id = "3".repeat(32); state.job_history = { available: true, count: 2 }; }
    const h = harness(state, call => {
      if (call.path === "/api/jobs") return response({ jobs: [jobSummary(state.job), jobSummary(earlier)], next_before: null });
      if (call.path === "/api/job") return response({ job: earlier, reconciliation: { available: false, observation: null } });
    });
    try {
      await h.app.start();
      if (historical) { h.change(h.byId("job-choice"), earlier.id); await settle(); }
      assert.equal(h.byId("result").hidden, true);
      assert.ok(!h.document.documentElement.textContent.includes(canary));
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    } finally { h.close(); }
  }
});

test("Compose metadata summary retains canonical identity without presenting the full plan", async () => {
  const state = composeState("prepare", "succeeded"), h = harness(state);
  try {
    await h.app.start();
    assert.equal(h.byId("result").hidden, false);
    assert.match(h.byId("result-description").textContent, /metadata only/);
    assert.match(h.byId("result-description").textContent, /contents remain in the host workspace/);
    assert.match(h.byId("mcp-configuration").getAttribute("aria-label"), /metadata summary/);
    const displayed = JSON.parse(h.byId("mcp-configuration").textContent);
    assert.deepEqual(displayed.backend, state.job.result.backend);
    assert.equal(displayed.backend.plan, undefined);
    assert.equal(displayed.backend.plan_sha256, state.compose.plan_sha256);
    assert.equal(h.byId("stop-job").disabled, true);
  } finally { h.close(); }
});

test("Compose runtime diagnostics display only fixed codes with unconfirmed outcomes", async () => {
  for (const diagnostic of ["resource_mismatch", "stale_observation", "image_changed", "compose_unavailable"]) {
    const state = composeState("start", "unconfirmed"); delete state.job.result;
    state.job.error = "compose_operation_unconfirmed"; state.job.runtime_error = diagnostic;
    const h = harness(state);
    try {
      await h.app.start();
      assert.match(h.byId("job-context").textContent, new RegExp(`Runtime reported ${diagnostic}`));
      assert.match(h.byId("job-context").textContent, /does not establish whether effects occurred/);
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("stop-job").disabled, true);
      assert.ok(h.calls.every(call => call.path === "/api/state"));
    } finally { h.close(); }
  }
  for (const invalid of ["PRIVATE_STDERR_CANARY", "resource_mismatch\n", { code: "resource_mismatch" }, null]) {
    const state = composeState("start", "unconfirmed"); delete state.job.result;
    state.job.error = "compose_operation_unconfirmed"; state.job.runtime_error = invalid;
    const h = harness(state);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.ok(!h.document.documentElement.textContent.includes("PRIVATE_STDERR_CANARY"));
    } finally { h.close(); }
  }
  for (const status of ["running", "succeeded"]) {
    const state = composeState("start", status);
    state.job.error = "compose_operation_unconfirmed"; state.job.runtime_error = "resource_mismatch";
    const h = harness(state);
    try { await h.app.start(); assert.match(h.byId("connection-status").textContent, /unconfirmed/); }
    finally { h.close(); }
  }
});

const exportedRecipe = (status = "succeeded") => ({
  id: "9".repeat(32), family: "remote-recipe", kind: "export", status, request_sha256: "8".repeat(64),
  ...(status === "succeeded" ? { result: {
    scope: "portable_mcp_configuration_only",
    recipe: { identity: { publisher: "fixture", name: "hosted-example", version: "1.0.0" },
      digest: "a".repeat(64), package_content_digest: "b".repeat(64) },
    selection_digest: "c".repeat(64), destination: "/host/export",
    plugin_path: "/host/export/plugin.json", mcp_path: "/host/export/mcp.json",
    files: { "plugin.json": { sha256: "d".repeat(64), bytes: 150 }, "mcp.json": { sha256: "e".repeat(64), bytes: 100 } },
    mcp_configuration: { $schema: "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
      mcpServers: { example: { type: "streamable-http", url: "https://example.invalid/mcp" } } },
  } } : {}),
});
const recipeExportState = (status = "succeeded") => ({ ...stateValue(), job: exportedRecipe(status),
  local_recipes: { export_available: true }, reconciliation: { available: false, observation: null }, stop: { available: true } });

const exportRetryState = () => ({ ...recipeExportState("unconfirmed"), export_retry: { available: true } });
const retriedExport = (status = "running") => ({ ...exportedRecipe(status), id: "a".repeat(32), retry_of: "9".repeat(32) });

test("export retry is explicit and retains exact request identity", async () => {
  const h = harness(exportRetryState(), (call, fixture) => {
    if (call.path !== "/api/retry_export") return;
    fixture.state.job = retriedExport();
    return response({ job: fixture.state.job });
  });
  try {
    await h.app.start();
    assert.equal(h.byId("retry-export").disabled, false);
    assert.match(h.byId("retry-export-status").textContent, /fresh directory.*retained.*already complete/);
    assert.equal(h.calls.filter(c => c.path === "/api/retry_export").length, 0);
    h.byId("retry-export").click(); h.byId("retry-export").click(); await settle();
    assert.deepEqual(h.calls.filter(c => c.path === "/api/retry_export").map(c => c.payload), [{ job_id: "9".repeat(32) }]);
    assert.match(h.byId("job-context").textContent, /New export after attempt 9{32}/);
    assert.equal(h.byId("retry-export").hidden, true);
    assert.equal(h.byId("result").hidden, true);
  } finally { h.close(); }
});

test("lost or mismatched export retry response needs observation and explicit click", async () => {
  for (const mode of ["lost", "wrong_request", "wrong_predecessor", "wrong_kind"]) {
    const h = harness(exportRetryState(), call => {
      if (call.path !== "/api/retry_export") return;
      if (mode === "lost") throw Error("private provider details");
      const result = retriedExport();
      if (mode === "wrong_request") result.request_sha256 = "b".repeat(64);
      if (mode === "wrong_predecessor") result.retry_of = "b".repeat(32);
      if (mode === "wrong_kind") result.kind = "install";
      return response({ job: result });
    });
    try {
      await h.app.start(); h.byId("retry-export").click(); await settle();
      assert.match(h.byId("retry-export-status").textContent, /Could not confirm/);
      assert.equal(h.byId("retry-export").disabled, true);
      assert.ok(!h.document.body.textContent.includes("private provider details"));
      h.byId("retry-export").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/retry_export").length, 1);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("retry-export").disabled, false);
      assert.equal(h.calls.filter(c => c.path === "/api/retry_export").length, 1);
      h.byId("retry-export").click(); await settle();
      assert.deepEqual(h.calls.filter(c => c.path === "/api/retry_export").map(c => c.payload),
        [{ job_id: "9".repeat(32) }, { job_id: "9".repeat(32) }]);
    } finally { h.close(); }
  }
});

test("export retry is unavailable for recovery, busy, nonexport or settled success", async () => {
  for (const alter of [
    s => { s.job = exportedRecipe(); },
    s => { s.job.status = "running"; },
    s => { s.job = job("unconfirmed"); },
    s => { s.job.recovered = true; },
    s => { delete s.export_retry; },
    s => { s.export_retry.available = false; },
    s => { s.mode = "recovery"; s.job.recovered = true; s.job.recovery_request = { sha256: "b".repeat(64) }; },
    s => { s.operation_in_progress = true; },
  ]) {
    const initial = exportRetryState(); alter(initial);
    const h = harness(initial);
    try {
      await h.app.start();
      assert.equal(h.byId("retry-export").disabled, true);
      h.byId("retry-export").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/retry_export").length, 0);
    } finally { h.close(); }
  }
});

test("late export retry does not steal another selected history item", async () => {
  const gate = deferred(), initial = exportRetryState();
  const older = { ...exportedRecipe(), id: "b".repeat(32) };
  initial.job_history = { available: true, count: 2 };
  const h = harness(initial, async call => {
    if (call.path === "/api/jobs") return response({ jobs: [jobSummary(initial.job), jobSummary(older)], next_before: null });
    if (call.path === "/api/job") return response({ job: older, reconciliation: { available: false, observation: null } });
    if (call.path === "/api/retry_export") return gate.promise;
  });
  try {
    await h.app.start(); h.byId("retry-export").click(); await settle();
    h.change(h.byId("job-choice"), older.id); await settle();
    gate.resolve(response({ job: retriedExport() })); await settle();
    assert.equal(h.byId("job-choice").value, older.id);
    assert.match(h.byId("job-status").textContent, /export completed/);
    assert.equal(h.calls.filter(c => c.path === "/api/retry_export").length, 1);
  } finally { h.close(); }
});

test("export retry follows its exact successor when another surface changes current work", async () => {
  const initial = exportRetryState(); initial.job_history = { available: true, count: 1 };
  const successor = retriedExport("succeeded"), other = job("running", "prepare", "b".repeat(32));
  let retried = false;
  const h = harness(initial, (call, fixture) => {
    if (call.path === "/api/jobs") return response({ jobs: (retried ? [other, successor, initial.job] : [initial.job]).map(jobSummary), next_before: null });
    if (call.path === "/api/job") return response({ job: successor, reconciliation: { available: true, observation: null } });
    if (call.path === "/api/retry_export") {
      retried = true; fixture.state.job = other; fixture.state.job_history.count = 3;
      return response({ job: successor });
    }
  });
  try {
    await h.app.start(); h.byId("retry-export").click(); await settle();
    assert.equal(h.byId("job-choice").value, successor.id);
    assert.match(h.byId("job-status").textContent, /export completed/);
    assert.match(h.byId("current-job-status").textContent, /agno.*running/);
    assert.equal(h.byId("result").hidden, false);
    assert.ok(h.calls.some(c => c.path === "/api/job" && c.payload.job_id === successor.id));
  } finally { h.close(); }
});

const exportObservation = (status = "matched", revision = 1) => ({
  job_id: exportedRecipe().id, revision, status, scope: "current_portable_export_files_only",
  excluded: ["Current files do not prove historical export success."],
  ...(status === "matched" ? { result: { scope: "current_portable_export_files_only",
    export_check: { state: "matches" }, recipe: exportedRecipe().result.recipe,
    selection_digest: exportedRecipe().result.selection_digest,
    destination: exportedRecipe().result.destination, configuration: exportedRecipe().result } } : {}),
});

test("recovered export checks current files without changing its historical outcome", async () => {
  const state = { ...recipeExportState("unconfirmed"), mode: "recovery",
    local_recipes: { available: false }, reconciliation: { available: true, observation: null } };
  state.job.recovered = true;
  state.job.recovery_request = { id: state.job.id, sha256: "a".repeat(64) };
  const h = harness(state, (call, fixture) => {
    if (call.path === "/api/reconcile") {
      fixture.state.reconciliation.observation = exportObservation();
      return response({ observation: fixture.state.reconciliation.observation });
    }
  });
  try {
    await h.app.start();
    assert.equal(h.byId("reconcile").hidden, false);
    assert.equal(h.byId("reconcile").textContent, "Check exported files");
    assert.equal(h.byId("result").hidden, true);
    h.byId("reconcile").click(); await settle();
    assert.match(h.byId("job-status").textContent, /historical export outcome is unknown/);
    assert.match(h.byId("reconciliation-status").textContent, /Current exported files match/);
    assert.equal(h.byId("result").hidden, false);
    assert.deepEqual(h.calls.filter(c => c.path === "/api/reconcile").map(c => c.payload), [{ job_id: state.job.id }]);
    assert.equal(h.calls.some(c => c.path.includes("export_recipe")), false);
  } finally { h.close(); }
});

test("failed export readback hides both historical and previously matched configuration", async () => {
  const state = recipeExportState();
  state.reconciliation = { available: true, observation: exportObservation() };
  const h = harness(state, (call, fixture) => {
    if (call.path === "/api/reconcile") {
      fixture.state.reconciliation.observation = exportObservation("unconfirmed", 2);
      return response({ observation: fixture.state.reconciliation.observation });
    }
  });
  try {
    await h.app.start(); assert.equal(h.byId("result").hidden, false);
    h.byId("reconcile").click(); await settle();
    assert.equal(h.byId("result").hidden, true);
    assert.match(h.byId("job-status").textContent, /export completed/);
    assert.match(h.byId("reconciliation-status").textContent, /exported files are unconfirmed/);
  } finally { h.close(); }
});

test("export readback refuses a configuration joined to another recipe", async () => {
  for (const change of [
    value => { value.result.recipe.identity.name = "another-recipe"; },
    value => { value.result.recipe.digest = "f".repeat(64); },
    value => { value.result.recipe.package_content_digest = "f".repeat(64); },
  ]) {
    const state = recipeExportState();
    state.reconciliation = { available: true, observation: null };
    const h = harness(state, call => {
      if (call.path === "/api/reconcile") {
        const value = exportObservation();
        change(value);
        return response({ observation: value });
      }
    });
    try {
      await h.app.start();
      h.byId("reconcile").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.equal(h.calls.filter(call => call.path === "/api/reconcile").length, 1);
    } finally { h.close(); }
  }
});

test("hosted recipe export shows actual configuration without claiming installation or authentication", async () => {
  const h = harness(recipeExportState());
  try {
    await h.app.start();
    assert.match(h.byId("job-status").textContent, /MCP configuration export completed/);
    assert.equal(h.byId("result-heading").textContent, "Exported MCP configuration");
    assert.equal(h.byId("result").hidden, false);
    assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent), exportedRecipe().result.mcp_configuration);
    assert.match(h.byId("result-limits").textContent, /whole hosted endpoint/);
    assert.match(h.byId("result-paths").textContent, /fixture\/hosted-example@1.0.0/);
    assert.doesNotMatch(h.byId("result-paths").textContent, /Installation receipt/);
    assert.equal(h.byId("result-credential-references").textContent, "");
    assert.equal(h.byId("reconcile").hidden, true);
    assert.equal(h.byId("reconcile").disabled, true);
    assert.deepEqual(h.calls.map(call => call.path), ["/api/state"]);
  } finally { h.close(); }
});

test("hosted export requires explicit capability, correct kind and complete verified result", async () => {
  const mutations = [
    state => { delete state.local_recipes; },
    state => { state.local_recipes.export_available = false; },
    state => { state.mode = "recovery"; },
    state => { state.job.kind = "install"; },
    state => { state.job.family = "agno"; },
    state => { state.job.request_sha256 = "wrong"; },
    state => { state.job.result.scope = "observed_software_installation_only"; },
    state => { state.job.result.mcp_path = "/other/mcp.json"; },
    state => { delete state.job.result.files["mcp.json"]; },
    state => { state.job.result.files["mcp.json"].sha256 = "X".repeat(64); },
    state => { state.job.result.mcp_configuration = {}; },
  ];
  for (const mutate of mutations) {
    const state = recipeExportState(); mutate(state); const h = harness(state);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result").hidden, true);
    } finally { h.close(); }
  }
});

test("stopping a hosted export retains uncertainty and never offers installation readback", async () => {
  const h = harness(recipeExportState("running"), (call, fixture) => {
    if (call.path !== "/api/stop") return;
    fixture.state.job.status = "unconfirmed";
    fixture.state.job.stop = { status: "settled", scope: "local_job_only" };
    return response({ job: fixture.state.job });
  });
  try {
    await h.app.start(); h.byId("stop-job").click(); await settle();
    assert.match(h.byId("stop-status").textContent, /configuration export outcome remains unconfirmed/);
    assert.match(h.byId("job-context").textContent, /may be absent, partial or complete/);
    assert.doesNotMatch(h.byId("job-context").textContent, /checked exported/);
    assert.equal(h.byId("result").hidden, true);
    assert.equal(h.byId("reconcile").hidden, true);
    assert.equal(h.byId("retry-prepare").hidden, true);
    assert.deepEqual(h.calls.filter(call => call.path === "/api/stop").map(call => call.payload), [{ job_id: exportedRecipe().id }]);
  } finally { h.close(); }
});

test("historical hosted export remains viewable while another installation is current", async () => {
  const state = recipeExportState(); state.job = completed(); state.job_history = { available: true, count: 2 };
  const earlier = exportedRecipe();
  const h = harness(state, call => {
    if (call.path === "/api/jobs") return response({ jobs: [jobSummary(state.job), jobSummary(earlier)], next_before: null });
    if (call.path === "/api/job") return response({ job: earlier, reconciliation: { available: false, observation: null } });
  });
  try {
    await h.app.start(); h.change(h.byId("job-choice"), earlier.id); await settle();
    assert.equal(h.byId("result-heading").textContent, "Exported MCP configuration");
    assert.equal(h.byId("result").hidden, false);
    h.change(h.byId("job-choice"), ""); await settle();
    assert.equal(h.byId("result-heading").textContent, "Installed MCP configuration");
    assert.match(h.byId("result-paths").textContent, /Installation receipt/);
    assert.equal(h.byId("reconcile").hidden, false);
  } finally { h.close(); }
});

test("failed export refresh clears previously displayed configuration", async () => {
  const h = harness(recipeExportState());
  try {
    await h.app.start(); assert.equal(h.byId("result").hidden, false);
    delete h.fixture.state.job.result.files["plugin.json"];
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("result").hidden, true);
    assert.equal(h.byId("mcp-configuration").textContent, "");
  } finally { h.close(); }
});

test("stop is explicitly available during selected work and waits for owner settlement", async () => {
  const initial = { ...selectedState(), stop: { available: true }, operation_in_progress: true };
  const h = harness(initial, (call, fixture) => {
    if (call.path !== "/api/stop") return;
    fixture.state.job.stop = { status: "requested", scope: "local_job_only" };
    return response({ job: fixture.state.job });
  });
  try {
    await h.app.start();
    assert.equal(h.byId("stop-job").hidden, false);
    assert.equal(h.byId("stop-job").disabled, false);
    h.byId("stop-job").click(); h.byId("stop-job").click(); await settle();
    assert.deepEqual(h.calls.filter(call => call.path === "/api/stop").map(call => call.payload), [{ job_id: "1".repeat(32) }]);
    assert.equal(h.fixture.state.job.status, "running");
    assert.match(h.byId("stop-status").textContent, /Waiting.*settle/);
    assert.equal(h.byId("stop-job").disabled, true);
    assert.equal(h.byId("reconcile").disabled, true);
    h.fixture.state.job.status = "unconfirmed";
    h.fixture.state.job.stop.status = "settled";
    h.fixture.state.operation_in_progress = false;
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("stop-status").textContent, /Files.*retained.*unconfirmed/);
    assert.equal(h.byId("reconcile").disabled, false);
    assert.equal(h.byId("result").hidden, true);
  } finally { h.close(); }
});

test("lost or mismatched stop replies need fresh observation and explicit retry", async () => {
  for (const mode of ["lost", "foreign"]) {
    const h = harness({ ...selectedState(), stop: { available: true } }, call => {
      if (call.path !== "/api/stop") return;
      if (mode === "lost") throw new Error("private error");
      return response({ job: { ...selectedState().job, id: "2".repeat(32), stop: { status: "requested", scope: "local_job_only" } } });
    });
    try {
      await h.app.start(); h.byId("stop-job").click(); await settle();
      assert.match(h.byId("stop-status").textContent, /Could not confirm/);
      assert.ok(!h.document.body.textContent.includes("private error"));
      assert.equal(h.byId("stop-job").disabled, true);
      h.byId("stop-job").click(); await settle();
      assert.equal(h.calls.filter(call => call.path === "/api/stop").length, 1);
      h.byId("refresh").click(); await settle();
      assert.equal(h.calls.filter(call => call.path === "/api/stop").length, 1);
      assert.match(h.byId("job-status").textContent, /running/);
      assert.match(h.byId("stop-status").textContent, /try Stop local setup again/);
      assert.equal(h.byId("stop-job").disabled, false);
      h.byId("stop-job").click(); await settle();
      assert.deepEqual(h.calls.filter(call => call.path === "/api/stop").map(call => call.payload),
        [{ job_id: "1".repeat(32) }, { job_id: "1".repeat(32) }]);
    } finally { h.close(); }
  }
});

test("failed readback or observed stop does not enable a stop retry", async () => {
  let failRead = false;
  const h = harness({ ...selectedState(), stop: { available: true } }, call => {
    if (call.path === "/api/stop") throw new Error("lost stop response");
    if (call.path === "/api/state" && failRead) throw new Error("lost read response");
  });
  try {
    await h.app.start(); h.byId("stop-job").click(); await settle();
    failRead = true; h.byId("refresh").click(); await settle();
    assert.equal(h.byId("stop-job").disabled, true);
    failRead = false;
    h.fixture.state.job.stop = { status: "requested", scope: "local_job_only" };
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("stop-job").disabled, true);
    assert.match(h.byId("stop-status").textContent, /Waiting.*settle/);
    assert.equal(h.calls.filter(call => call.path === "/api/stop").length, 1);
  } finally { h.close(); }
});

test("old hosts and recovery do not offer stop; closing never sends stop", async () => {
  for (const initial of [selectedState(), { ...selectedState(), mode: "recovery", stop: { available: false } }]) {
    const h = harness(initial);
    await h.app.start();
    assert.equal(h.byId("stop-job").hidden, true);
    h.byId("stop-job").click(); h.close();
    assert.equal(h.calls.filter(call => call.path === "/api/stop").length, 0);
  }
});

test("late stop acknowledgement cannot replace a newer current job", async () => {
  const delayed = deferred();
  const h = harness({ ...selectedState(), stop: { available: true } }, call => call.path === "/api/stop" ? delayed.promise : undefined);
  try {
    await h.app.start(); h.byId("stop-job").click();
    const old = clone(h.fixture.state.job);
    h.fixture.state.job.id = "2".repeat(32);
    h.byId("refresh").click(); await settle();
    delayed.resolve(response({ job: { ...old, stop: { status: "requested", scope: "local_job_only" } } })); await settle();
    assert.equal(h.fixture.state.job.id, "2".repeat(32));
    assert.equal(h.byId("stop-status").textContent, "");
    assert.equal(h.byId("stop-job").disabled, false);
    assert.deepEqual(h.calls.filter(call => call.path === "/api/stop").map(call => call.payload), [{ job_id: old.id }]);
  } finally { h.close(); }
});
function historyFixture({ currentStatus = "succeeded", observed = true } = {}) {
  const first = { ...completed(), id: "earlier-install" };
  first.result.mcp_path = "/host/slack/ready/mcp.json";
  const second = { ...completed(), id: "current-install", status: currentStatus };
  second.result.mcp_path = "/host/gmail/ready/mcp.json";
  const initial = { ...stateValue(), job: second, job_history: { available: true, count: 2 },
    operation_in_progress: currentStatus === "running", reconciliation: { available: true, observation: null } };
  const oldObservation = observation("matched", first.id);
  oldObservation.result.mcp_path = "/host/slack/checked/mcp.json";
  const detail = { job: first, reconciliation: { available: true, observation: observed ? oldObservation : null } };
  const page = { jobs: [jobSummary(second), jobSummary(first)], next_before: null };
  return { initial, detail, page, handler(call) {
    if (call.path === "/api/jobs") return response(page);
    if (call.path === "/api/job") return response(detail);
  } };
}

test("retained job browsing reads the earlier result and observation after a second install and reload", async () => {
  for (const observed of [false, true]) {
    const fixture = historyFixture({ observed });
    // A new workbench against the same host retains the ability to choose it.
    for (let reload = 0; reload < 2; reload += 1) {
      const h = harness(fixture.initial, fixture.handler);
      try {
        await h.app.start();
        assert.equal(h.byId("job-history").hidden, false);
        assert.match(h.byId("result-paths").textContent, /gmail/);
        h.byId("job-choice").focus();
        h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
        assert.equal(h.document.activeElement, h.byId("job-choice"));
        assert.match(h.byId("result-paths").textContent, observed ? /slack\/checked/ : /slack\/ready/);
        assert.equal(h.fixture.state.job.id, "current-install");
        assert.equal(h.calls.filter(call => call.path === "/api/job").length, 1);
        assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
        h.change(h.byId("job-choice"), ""); await settle();
        assert.match(h.byId("result-paths").textContent, /gmail/);
        assert.equal(h.document.activeElement, h.byId("job-choice"));
      } finally { h.close(); }
    }
  }
});

test("historical result arrival preserves focus moved while its read was pending", async () => {
  const fixture = historyFixture(), delayed = deferred();
  const h = harness(fixture.initial, call => call.path === "/api/job" ? delayed.promise : fixture.handler(call));
  try {
    await h.app.start(); h.byId("job-choice").focus();
    h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
    assert.equal(h.byId("result").hidden, true);
    h.byId("refresh").focus();
    delayed.resolve(response(fixture.detail)); await settle();
    assert.match(h.byId("result-paths").textContent, /slack\/checked/);
    assert.equal(h.document.activeElement, h.byId("refresh"));
    assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
  } finally { h.close(); }
});

test("explicit status refresh still focuses a newly completed result", async () => {
  const h = harness({ ...stateValue(), job: job("running") });
  try {
    await h.app.start();
    h.fixture.state.job = completed();
    h.byId("refresh").focus(); h.byId("refresh").click(); await settle();
    assert.equal(h.byId("result").hidden, false);
    assert.equal(h.document.activeElement, h.byId("result-heading"));
    assert.ok(h.calls.every(call => call.path === "/api/state"));
  } finally { h.close(); }
});

test("viewing an earlier job preserves current work locks and reconciles only its selected ID", async () => {
  const fixture = historyFixture({ currentStatus: "running" });
  const h = harness(fixture.initial, call => call.path === "/api/reconcile" ? response({ observation: fixture.detail.reconciliation.observation }) : fixture.handler(call));
  try {
    await h.app.start(); h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
    assert.match(h.byId("result-paths").textContent, /slack/);
    assert.match(h.byId("current-job-status").textContent, /running/);
    assert.equal(h.byId("reconcile").disabled, true);
    assert.equal(h.byId("family").disabled, true);
    h.fixture.state.job.status = "succeeded";
    // An old-job reconciliation can hold the global operation lock even when the latest job is settled.
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("reconcile").disabled, true);
    h.fixture.state.operation_in_progress = false;
    h.byId("refresh").click(); await settle();
    h.byId("reconcile").click(); await settle();
    assert.deepEqual(h.calls.filter(call => call.path === "/api/reconcile").map(call => call.payload), [{ job_id: fixture.detail.job.id }]);
    assert.equal(h.fixture.state.job.id, "current-install");
  } finally { h.close(); }
});

test("malformed job indexes are refused before rendering choices", async () => {
  const changes = [page => { page.jobs.push(clone(page.jobs[0])); }, page => { page.jobs[0].profile = []; },
    page => { page.jobs[0].family = "foreign-family"; }, page => { page.next_before = "not-the-last-job"; },
    page => { page.jobs = Array.from({ length: 51 }, (_, index) => ({ ...page.jobs[0], id: String(index) })); }];
  for (const change of changes) {
    const fixture = historyFixture(); change(fixture.page);
    const h = harness(fixture.initial, fixture.handler);
    try {
      await h.app.start();
      assert.equal(h.byId("result").hidden, true);
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.calls.filter(call => call.path === "/api/job").length, 0);
    } finally { h.close(); }
  }
});

test("mismatched historical jobs and observations never expose their result", async () => {
  const changes = [value => { value.job.id = "other-id"; }, value => { value.job.family = "services"; },
    value => { value.reconciliation.observation.job_id = "other-id"; },
    value => { value.reconciliation.observation.result.family = "services"; },
    value => { value.reconciliation.observation.result.profile = "different-profile"; value.job.result.profile = profile; },
    value => { value.job.status = "invented"; }, value => { value.reconciliation.available = "yes"; }];
  for (const change of changes) {
    const fixture = historyFixture();
    const invalid = clone(fixture.detail); change(invalid);
    const h = harness(fixture.initial, call => call.path === "/api/job" ? response(invalid) : fixture.handler(call));
    try {
      await h.app.start(); h.byId("job-choice").focus(); h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.equal(h.byId("mcp-configuration").textContent, "");
      assert.equal(h.byId("problem").hidden, false);
      assert.equal(h.document.activeElement, h.byId("job-choice"));
      assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
    } finally { h.close(); }
  }
});

test("late historical detail cannot replace a newer selection or survive disposal", async () => {
  for (const dispose of [false, true]) {
    const fixture = historyFixture(), delayed = deferred();
    const h = harness(fixture.initial, call => call.path === "/api/job" ? delayed.promise : fixture.handler(call));
    try {
      await h.app.start(); h.byId("job-choice").focus(); h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
      if (dispose) { h.byId("refresh").focus(); h.app.dispose(); }
      else { h.change(h.byId("job-choice"), ""); await settle(); }
      delayed.resolve(response(fixture.detail)); await settle();
      assert.doesNotMatch(h.byId("result-paths").textContent, /slack/);
      if (!dispose) assert.match(h.byId("result-paths").textContent, /gmail/);
      assert.equal(h.document.activeElement, h.byId(dispose ? "refresh" : "job-choice"));
      assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
    } finally { h.close(); }
  }
});

test("job history loads bounded earlier pages only on explicit request", async () => {
  const fixture = historyFixture(); fixture.initial.job_history.count = 51;
  fixture.page.jobs = Array.from({ length: 50 }, (_, index) => ({ ...jobSummary(fixture.initial.job), id: index ? `retained-${index}` : fixture.initial.job.id }));
  fixture.page.next_before = "retained-49";
  const h = harness(fixture.initial, call => call.path === "/api/jobs" && call.payload.before ?
    response({ jobs: [jobSummary(fixture.detail.job)], next_before: null }) : fixture.handler(call));
  try {
    await h.app.start();
    assert.equal(h.calls.filter(call => call.path === "/api/jobs").length, 1);
    h.byId("jobs-earlier").click(); await settle();
    assert.equal(h.byId("jobs-earlier").hidden, true);
    assert.deepEqual(h.calls.filter(call => call.path === "/api/jobs").map(call => call.payload), [{ before: null }, { before: "retained-49" }]);
    h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
    assert.match(h.byId("result-paths").textContent, /slack/);
  } finally { h.close(); }
});

test("old servers retain current-only behavior without history calls", async () => {
  const h = harness({ ...stateValue(), job: completed() });
  try {
    await h.app.start();
    assert.equal(h.byId("job-history").hidden, true);
    assert.equal(h.byId("result").hidden, false);
    assert.deepEqual(h.calls.map(call => call.path), ["/api/state"]);
  } finally { h.close(); }
});

test("selected summaries and observations refresh without a new job count or stale success", async () => {
  const fixture = historyFixture({ observed: false });
  fixture.detail.job.status = "running";
  fixture.page.jobs[1].status = "running";
  const h = harness(fixture.initial, fixture.handler);
  try {
    await h.app.start(); h.change(h.byId("job-choice"), fixture.detail.job.id); await settle();
    fixture.detail.job.status = "succeeded";
    fixture.detail.reconciliation.observation = observation("matched", fixture.detail.job.id);
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("job-choice").selectedOptions[0].textContent, /succeeded/);
    assert.equal(h.byId("result").hidden, false);
    for (const status of ["checking", "unconfirmed"]) {
      fixture.detail.reconciliation.observation = observation(status, fixture.detail.job.id, 2);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("mcp-configuration").textContent, "");
      assert.match(h.byId("reconciliation-status").textContent, status === "checking" ? /Checking/ : /could not be confirmed/);
    }
    assert.equal(h.calls.filter(call => call.path === "/api/jobs").length, 1);
  } finally { h.close(); }
});

test("selected package history validates matching readback type and selection", async () => {
  const first = selectedState("python", "succeeded");
  const current = selectedState("node", "succeeded");
  current.job.id = "2".repeat(32);
  current.selected_installation.profiles = ["python", "node"];
  current.job_history = { available: true, count: 2 };
  const detail = { job: first.job, reconciliation: { available: true, observation: selectedObservation("python") } };
  const h = harness(current, call => call.path === "/api/jobs" ? response({ jobs: [jobSummary(current.job), jobSummary(first.job)], next_before: null }) :
    call.path === "/api/job" ? response(detail) : undefined);
  try {
    await h.app.start(); h.change(h.byId("job-choice"), first.job.id); await settle();
    assert.equal(h.byId("result").hidden, false);
    detail.reconciliation.observation.result.selection_kind = "node";
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("result").hidden, true);
    assert.equal(h.byId("reconcile").disabled, true);
  } finally { h.close(); }
});

test("session capability is consumed once from fragment, removed, never stored or displayed; initial navigation only reads state", async () => {
  const h = harness();
  try {
    await h.app.start();
    assert.equal(h.dom.window.location.hash, "");
    assert.equal(h.dom.window.localStorage.length, 0); assert.equal(h.dom.window.sessionStorage.length, 0);
    assert.ok(!h.document.documentElement.outerHTML.includes("fixture-capability"));
    assert.deepEqual(h.calls.map(call => call.path), ["/api/state"]);
    assert.equal(h.byId("family").value, ""); assert.equal(h.byId("profile").value, "");
    const { init } = h.calls[0];
    assert.equal(init.headers["X-Toolkit-Session"], "fixture-capability");
    assert.equal(init.method, "POST"); assert.equal(init.credentials, "omit"); assert.equal(init.redirect, "error");
    assert.equal(init.mode, "same-origin"); assert.equal(init.cache, "no-store");
  } finally { h.close(); }
});

test("missing, duplicate and unrelated session fragments refuse all API use", async () => {
  for (const hash of ["", "#session=one&session=two", "#session=one&next=elsewhere", "#session="]) {
    const h = harness(stateValue(), undefined, hash);
    try { await h.app.start(); assert.equal(h.calls.length, 0); assert.equal(h.dom.window.location.hash, ""); assert.equal(h.byId("family").disabled, true); }
    finally { h.close(); }
  }
  const loc = { hash: "#session=one", pathname: "/", search: "?view=setup" }; let path;
  assert.equal(takeSession(loc, { replaceState: (_state, _title, value) => { path = value; } }), "one");
  assert.equal(path, "/?view=setup");
});

test("help is detached, rejects ambiguous fields and effects; typed draft omits inactive fields without credential values", () => {
  const source = helpValue(); const help = normalizeHelp(source, "agno", profile);
  source.fields[0].name = "mutated"; source.prerequisites[0].instruction = "mutated";
  assert.equal(help.fields[0].name, "host"); assert.notEqual(help.prerequisites[0].instruction, "mutated");
  const draft = draftFromFields(help, ["search_web"], { host: "https://localhost", timeout: "10", token_env: "MY_TOKEN", enable_writes: "true" });
  assert.deepEqual({ ...draft.values }, { host: "https://localhost", timeout: 10, token_env: "MY_TOKEN" });
  for (const value of ["token-secret-with-dashes", "Bearer secret", "MY TOKEN", "TOKEN=value"]) {
    assert.throws(() => draftFromFields(help, ["search_web"], { host: "https://localhost", token_env: value }), error => error instanceof FormError && error.code === "reference_name_required");
  }
  assert.throws(() => draftFromFields(help, [], { host: "x" }), /operations_required/);
  assert.throws(() => draftFromFields(help, ["search_web", "search_web"], { host: "x" }), /operations_required/);
  for (const timeout of ["1.5", "1e2", "9007199254740992"]) assert.throws(() => draftFromFields(help, ["search_web"], { host: "x", timeout }), /integer_required/);
  assert.throws(() => draftFromFields(help, ["write_fixture"], { host: "x" }), /field_required/);
  for (const mutate of [value => { value.fields[0].name = "__proto__"; }, value => { value.operations[0].effect = "safe"; }, value => { value.fields.push(value.fields[0]); }, value => { value.fields[0].kind = "json"; }]) {
    const bad = helpValue(); mutate(bad); assert.throws(() => normalizeHelp(bad, "agno", profile), /invalid_help/);
  }
});

test("explicit preparation is the only install trigger; preparing and unconfirmed families cannot retry", async () => {
  const h = harness(stateValue("unprepared"));
  try {
    await h.app.start(); h.change(h.byId("family"), "agno");
    assert.deepEqual(h.calls.map(call => call.path), ["/api/state"]);
    h.byId("prepare").click(); h.byId("prepare").click(); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/prepare").length, 1);
    assert.equal(h.byId("prepare").disabled, true); assert.equal(h.byId("profile").disabled, true);
    h.fixture.state.job = job("unconfirmed", "prepare"); h.fixture.state.families[0].status = "unconfirmed";
    h.byId("refresh").click(); await settle(); assert.equal(h.byId("prepare").disabled, true);
    h.byId("prepare").dispatchEvent(new h.dom.window.Event("click")); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/prepare").length, 1);
    assert.match(h.byId("preparation-description").textContent, /unconfirmed/);
  } finally { h.close(); }
});

test("generated controls have no selected operation or boolean defaults, label references, and render guide only as text", async () => {
  const h = harness();
  try {
    await h.app.start(); await h.choose();
    assert.equal(h.byId("configuration").hidden, false);
    assert.equal(h.document.activeElement.id, "configure-heading");
    assert.equal(h.byId("operations").querySelectorAll("input:checked").length, 0);
    assert.match(h.byId("operations").textContent, /Effect not specified/); assert.match(h.byId("operations").textContent, /Write/);
    assert.equal(h.field("enable_writes").value, ""); assert.equal(h.field("enable_writes").disabled, true);
    h.checked(1); assert.equal(h.field("enable_writes").required, true); assert.equal(h.field("enable_writes").disabled, false);
    assert.match(h.field("token_env").labels[0].textContent, /environment variable name/);
    assert.equal(h.document.querySelectorAll('input[type="password"], textarea').length, 0);
    assert.equal(h.byId("reference-text").textContent, h.fixture.help.guide_text);
    assert.equal(h.byId("reference-text").querySelectorAll("script, a, img").length, 0);
    assert.equal(h.field("host").getAttribute("aria-describedby"), h.field("host").id + "-hint");
    assert.equal(h.calls.filter(call => call.path === "/api/check" || call.path === "/api/install").length, 0);
  } finally { h.close(); }
});

test("installed-validator refusal opens exact reference with fixed error and correction validates; edits invalidate review", async () => {
  let count = 0;
  const h = harness(stateValue(), call => call.path === "/api/check" && ++count === 1 ? response({ error: "RAW-CONFIG-DO-NOT-RENDER" }, 400) : undefined);
  try {
    await h.app.start(); await h.fill(); h.change(h.field("timeout"), "0", "input"); await h.submit();
    assert.equal(h.byId("review").hidden, true); assert.equal(h.byId("configuration-reference").open, true);
    assert.match(h.byId("validation-status").textContent, /installed validator/);
    assert.ok(!h.document.documentElement.outerHTML.includes("RAW-CONFIG-DO-NOT-RENDER"));
    h.change(h.field("timeout"), "10", "input"); await h.submit();
    assert.equal(h.byId("review").hidden, false); assert.equal(h.document.activeElement.id, "review-heading");
    assert.match(h.byId("review-values").textContent, /search_web \(Effect not specified\)/);
    h.change(h.field("timeout"), "11", "input"); assert.equal(h.byId("review").hidden, true); assert.equal(h.byId("install").disabled, true);
    h.byId("install").dispatchEvent(new h.dom.window.Event("click")); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/install").length, 0);
  } finally { h.close(); }
});

test("install sends the exact reviewed draft once and displays only actual host-published MCP configuration", async () => {
  const h = harness();
  try {
    await h.app.start(); await h.fill(); h.change(h.field("token_env"), "MY_TOKEN", "input"); await h.submit();
    const checked = h.calls.find(call => call.path === "/api/check").payload;
    h.byId("install").click(); h.byId("install").click(); await settle();
    const installs = h.calls.filter(call => call.path === "/api/install"); assert.equal(installs.length, 1);
    assert.deepEqual(installs[0].payload, { ...checked, validation_id: "fixture-validation", configuration_sha256: "a".repeat(64) });
    assert.equal(Object.hasOwn(installs[0].payload, "destination"), false); assert.equal(h.byId("review").hidden, true);
    assert.equal(h.byId("result").hidden, true); assert.match(h.byId("job-context").textContent, /does not cancel/);
    h.fixture.state.job = completed(); h.byId("refresh").click(); await settle();
    assert.equal(h.byId("result").hidden, false); assert.equal(h.document.activeElement.id, "result-heading");
    assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent), completed().result.mcp_configuration);
    assert.match(h.byId("result-paths").textContent, /\/host\/chosen\/new\/ready\/mcp.json/);
    assert.match(h.byId("result").textContent, /not a connected account/);
  } finally { h.close(); }
});

test("prior output is cleared when current job is running, unconfirmed, preparation or absent", async () => {
  for (const next of [job("running", "install", "new"), job("unconfirmed", "install", "new"), job("succeeded", "prepare", "new"), null]) {
    const initial = stateValue(); initial.job = completed(); const h = harness(initial);
    try {
      await h.app.start(); assert.equal(h.byId("result").hidden, false);
      h.fixture.state.job = next; h.byId("refresh").click(); await settle();
      assert.equal(h.byId("result").hidden, true); assert.equal(h.byId("mcp-configuration").textContent, ""); assert.equal(h.byId("result-paths").textContent, "");
    } finally { h.close(); }
  }
});

test("check installation is explicit, default-disabled, and displays a separate current match without changing history", async () => {
  const initial = stateValue(); initial.job = job("unconfirmed");
  const h = harness(initial, (call, fixture) => {
    if (call.path === "/api/reconcile") {
      fixture.state.reconciliation.observation = observation();
      return response({ observation: observation() });
    }
  });
  try {
    await h.app.start(); assert.equal(h.byId("reconcile").disabled, true);
    assert.match(h.byId("reconciliation-status").textContent, /explicitly select a trusted verifier/);
    h.fixture.state.reconciliation = { available: true, observation: null };
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("reconcile").disabled, false);
    assert.equal(h.calls.filter(call => call.path === "/api/reconcile").length, 0);
    h.byId("reconcile").click(); h.byId("reconcile").click(); await settle();
    const checks = h.calls.filter(call => call.path === "/api/reconcile");
    assert.equal(checks.length, 1); assert.deepEqual(checks[0].payload, { job_id: "fixture-job" });
    assert.match(h.byId("job-status").textContent, /unconfirmed/);
    assert.match(h.byId("reconciliation-status").textContent, /historical job outcome above is unchanged/);
    assert.match(h.byId("reconciliation-limits").textContent, /Runtime integrity is not verified/);
    assert.equal(h.byId("result").hidden, false);
    assert.match(h.byId("result-paths").textContent, /Checked profile/);
    assert.equal(h.calls.filter(call => call.path === "/api/install").length, 0);
    h.fixture.state.reconciliation.observation = observation("unconfirmed", "fixture-job", 2);
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("result").hidden, true);
  } finally { h.close(); }
});

test("late reconciliation cannot attach old output to another job and stale same-job observations cannot supersede newer checks", async () => {
  const pending = deferred(); const initial = stateValue(); initial.job = job("unconfirmed");
  initial.reconciliation = { available: true, observation: null };
  const h = harness(initial, call => call.path === "/api/reconcile" ? pending.promise : undefined);
  try {
    await h.app.start(); h.byId("reconcile").click(); await settle();
    h.fixture.state.job = job("unconfirmed", "install", "new-job");
    h.fixture.state.reconciliation.observation = observation("unconfirmed", "new-job", 3);
    h.byId("refresh").click(); await settle();
    pending.resolve(response({ observation: observation() })); await settle();
    assert.equal(h.byId("result").hidden, true);
    assert.match(h.byId("reconciliation-status").textContent, /could not be confirmed/);
    h.fixture.state.reconciliation.observation = observation("matched", "new-job", 2);
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("result").hidden, true);
    h.fixture.state.reconciliation.observation = observation("checking", "new-job", 4);
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("reconcile").disabled, true);
    assert.match(h.byId("reconciliation-status").textContent, /Checking/);
  } finally { h.close(); }
});

test("malformed observation and check response loss cannot expose verifier diagnostics or enable retries", async () => {
  const initial = stateValue(); initial.job = job("unconfirmed");
  initial.reconciliation = { available: true, observation: null };
  const h = harness(initial, call => call.path === "/api/reconcile" ? response({ error: "PRIVATE_VERIFIER_TEXT" }, 500) : undefined);
  try {
    await h.app.start(); h.byId("reconcile").click(); await settle();
    assert.equal(h.byId("reconcile").disabled, true);
    assert.equal(h.byId("result").hidden, true);
    assert.ok(!h.document.documentElement.outerHTML.includes("PRIVATE_VERIFIER_TEXT"));
    h.fixture.state.reconciliation.observation = observation("matched", "wrong-job");
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("family").disabled, true);
    assert.equal(h.byId("result").hidden, true);
  } finally { h.close(); }
});

test("stale help response cannot replace a newer selection or overlap the serialized settings read", async () => {
  const first = deferred(), second = deferred(); let calls = 0;
  const initial = stateValue(); initial.families[0].profiles.push({ id: "second-profile", operations: clone(operations) });
  const h = harness(initial, call => call.path === "/api/help" ? (++calls === 1 ? first.promise : second.promise) : undefined);
  try {
    await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile); h.byId("show-profile").click();
    h.byId("profile").focus(); h.change(h.byId("profile"), "second-profile"); h.byId("show-profile").click(); await settle();
    assert.equal(calls, 1); assert.equal(h.byId("profile").disabled, false);
    assert.equal(h.byId("show-profile").getAttribute("aria-disabled"), "true");
    assert.match(h.byId("profile-status").textContent, /earlier settings request/);
    first.resolve(response(helpValue())); await settle();
    assert.equal(h.byId("configuration").hidden, true); assert.equal(h.byId("profile").value, "second-profile");
    assert.equal(h.document.activeElement.id, "profile"); assert.equal(calls, 1);
    assert.equal(h.byId("show-profile").getAttribute("aria-disabled"), "false");
    h.byId("show-profile").focus(); h.byId("show-profile").click(); await settle();
    assert.equal(calls, 2);
    second.resolve(response({ ...helpValue(), profile: "second-profile" })); await settle();
    assert.equal(h.byId("configuration").hidden, false); assert.match(h.byId("profile-description").textContent, /second-profile/);
    assert.equal(h.byId("profile").disabled, false);
  } finally { h.close(); }
});

test("profile browsing preserves focus and waits for an explicit settings action", async () => {
  const initial = stateValue(); initial.families[0].profiles.push({ id: "second-profile", operations: clone(operations) });
  const h = harness(initial);
  try {
    await h.app.start(); h.change(h.byId("family"), "agno");
    assert.equal(h.byId("show-profile").getAttribute("aria-disabled"), "true");
    h.byId("show-profile").click(); await settle();
    h.byId("profile").focus();
    for (const selected of [profile, "second-profile", profile]) {
      h.change(h.byId("profile"), selected); await settle();
      assert.equal(h.document.activeElement.id, "profile");
      assert.equal(h.byId("configuration").hidden, true);
      assert.equal(h.byId("show-profile").getAttribute("aria-disabled"), "false");
    }
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("profile").value, profile);
    assert.ok(h.calls.every(call => call.path === "/api/state"));
    assert.equal(h.byId("profile").getAttribute("aria-describedby"), "profile-status");
    assert.equal(h.byId("show-profile").getAttribute("aria-describedby"), "profile-status");
    assert.equal(h.byId("profile-status").getAttribute("role"), "status");
    assert.match(h.byId("profile-status").textContent, /Show profile settings.*without installing/);
  } finally { h.close(); }
});

test("explicit settings action retains pending focus and advances only its owned focus", async () => {
  const gate = deferred();
  const h = harness(stateValue(), call => call.path === "/api/help" ? gate.promise : undefined);
  try {
    await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile);
    const button = h.byId("show-profile"); button.focus(); button.click(); button.click(); await settle();
    assert.equal(h.document.activeElement, button); assert.equal(button.disabled, false);
    assert.equal(button.getAttribute("aria-disabled"), "true");
    assert.match(h.byId("profile-status").textContent, /Loading settings/);
    assert.deepEqual(h.calls.filter(call => call.path === "/api/help").map(call => call.payload), [{ family: "agno", profile }]);
    gate.resolve(response(helpValue())); await settle();
    assert.equal(h.document.activeElement.id, "configure-heading");
    assert.match(h.byId("profile-status").textContent, /Settings loaded/);
    assert.equal(button.getAttribute("aria-disabled"), "false");
    assert.equal(h.byId("install").disabled, true);
  } finally { h.close(); }
});

test("delayed settings preserve later focus and unfocused activation never takes focus", async () => {
  for (const startedWithFocus of [true, false]) {
    const gate = deferred();
    const h = harness(stateValue(), call => call.path === "/api/help" ? gate.promise : undefined);
    try {
      await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile);
      if (startedWithFocus) h.byId("show-profile").focus();
      else h.byId("profile").focus();
      h.byId("show-profile").click(); await settle();
      const target = h.byId(startedWithFocus ? "catalog-filter" : "profile");
      if (startedWithFocus) { h.byId("catalog-controls").hidden = false; target.focus(); }
      gate.resolve(response(helpValue())); await settle();
      assert.equal(h.document.activeElement, target);
      assert.equal(h.byId("configuration").hidden, false);
    } finally { h.close(); }
  }
});

test("changing library during settings invalidates the read while keeping new choices usable", async () => {
  const gate = deferred(), initial = stateValue();
  initial.families.push({ id: "services", status: "prepared", profiles: [{ id: "service-profile", operations: clone(operations) }] });
  const h = harness(initial, call => call.path === "/api/help" ? gate.promise : undefined);
  try {
    await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile); h.byId("show-profile").click();
    h.byId("family").focus(); h.change(h.byId("family"), "services"); h.change(h.byId("profile"), "service-profile");
    assert.equal(h.byId("family").disabled, false); assert.equal(h.byId("profile").disabled, false);
    h.byId("show-profile").click(); await settle();
    gate.resolve(response(helpValue())); await settle();
    assert.equal(h.byId("family").value, "services"); assert.equal(h.byId("profile").value, "service-profile");
    assert.equal(h.document.activeElement.id, "family"); assert.equal(h.byId("configuration").hidden, true);
    assert.equal(h.byId("show-profile").getAttribute("aria-disabled"), "false");
    assert.equal(h.calls.filter(call => call.path === "/api/help").length, 1);
  } finally { h.close(); }
});

test("settings failure has a named retry action without reflected diagnostics or implicit replay", async () => {
  let failed = true;
  const h = harness(stateValue(), call => call.path === "/api/help" ? response(failed ? { error: "PRIVATE_HELP_DETAIL" } : helpValue(), failed ? 500 : 200) : undefined);
  try {
    await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile);
    h.byId("show-profile").focus(); h.byId("show-profile").click(); await settle();
    assert.equal(h.document.activeElement.id, "show-profile"); assert.equal(h.byId("configuration").hidden, true);
    assert.match(h.byId("profile-status").textContent, /could not load|Could not load/);
    assert.match(h.byId("profile-status").textContent, /Show profile settings.*again/);
    assert.doesNotMatch(h.document.body.textContent, /PRIVATE_HELP_DETAIL/);
    h.byId("refresh").click(); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/help").length, 1);
    failed = false; h.byId("show-profile").click(); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/help").length, 2);
    assert.equal(h.byId("configuration").hidden, false); assert.equal(h.byId("problem").hidden, true);
    assert.ok(h.calls.every(call => ["/api/state", "/api/help"].includes(call.path)));
  } finally { h.close(); }
});

test("stale settings failure cannot overwrite a newer selection status", async () => {
  const gate = deferred(), initial = stateValue();
  initial.families[0].profiles.push({ id: "second-profile", operations: clone(operations) });
  const h = harness(initial, call => call.path === "/api/help" ? gate.promise : undefined);
  try {
    await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile); h.byId("show-profile").click();
    h.byId("profile").focus(); h.change(h.byId("profile"), "second-profile");
    gate.resolve(response({}, 500)); await settle();
    assert.equal(h.byId("problem").hidden, true);
    assert.match(h.byId("profile-status").textContent, /configure second-profile/);
    assert.equal(h.document.activeElement.id, "profile");
  } finally { h.close(); }
});

test("settings selection changes invalidate a reviewed configuration before any new read", async () => {
  const initial = stateValue(); initial.families[0].profiles.push({ id: "second-profile", operations: clone(operations) });
  const h = harness(initial);
  try {
    await h.app.start(); await h.fill(); await h.submit();
    assert.equal(h.byId("review").hidden, false);
    h.byId("profile").focus(); h.change(h.byId("profile"), "second-profile");
    assert.equal(h.byId("configuration").hidden, true); assert.equal(h.byId("review").hidden, true);
    assert.equal(h.byId("install").disabled, true); assert.equal(h.byId("validate").disabled, true);
    assert.equal(h.document.activeElement.id, "profile");
    assert.equal(h.calls.filter(call => call.path === "/api/help").length, 1);
    h.byId("install").dispatchEvent(new h.dom.window.Event("click")); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/install").length, 0);
  } finally { h.close(); }
});

test("late settings after deadline or disposal cannot render or steal focus", async () => {
  for (const stop of ["deadline", "dispose"]) {
    const gate = deferred();
    const h = harness(stateValue(), call => call.path === "/api/help" ? gate.promise : undefined);
    try {
      await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile);
      h.byId("show-profile").focus(); h.byId("show-profile").click(); await settle();
      if (stop === "deadline") await expireResponses(h); else h.app.dispose();
      const status = h.byId("profile-status").textContent;
      h.byId("profile").focus(); gate.resolve(response(helpValue())); await settle();
      assert.equal(h.byId("configuration").hidden, true); assert.equal(h.byId("install").disabled, true);
      assert.equal(h.byId("profile-status").textContent, status);
      assert.equal(h.document.activeElement.id, "profile");
      assert.equal(h.calls.filter(call => call.path === "/api/help").length, 1);
      assert.ok(h.calls.every(call => !Object.hasOwn(call.init, "signal")));
    } finally { h.close(); }
  }
});

test("settings action stays guarded when host work, readback mode or host loss forbids it", async () => {
  for (const scenario of ["running", "recovery", "unavailable"]) {
    let denyState = false;
    const h = harness(stateValue(), call => denyState && call.path === "/api/state" ? response({}, 500) : undefined);
    try {
      await h.app.start(); h.change(h.byId("family"), "agno"); h.change(h.byId("profile"), profile);
      if (scenario === "running") h.fixture.state.job = job("running");
      if (scenario === "recovery") h.fixture.state.mode = "recovery";
      if (scenario === "unavailable") denyState = true;
      h.byId("refresh").click(); await settle(); h.byId("show-profile").click(); await settle();
      assert.equal(h.byId("show-profile").getAttribute("aria-disabled"), "true");
      assert.equal(h.calls.filter(call => call.path === "/api/help").length, 0);
      assert.equal(h.byId("configuration").hidden, true);
    } finally { h.close(); }
  }
});

test("stale validation after an edit cannot enable install", async () => {
  const check = deferred(); const h = harness(stateValue(), call => call.path === "/api/check" ? check.promise : undefined);
  try {
    await h.app.start(); await h.fill(); void h.submit(); await settle();
    h.change(h.field("timeout"), "11", "input"); check.resolve(response(validationValue())); await settle();
    assert.equal(h.byId("review").hidden, true); assert.equal(h.byId("install").disabled, true);
  } finally { h.close(); }
});

test("malformed state, mismatched help operations and incomplete successful results fail closed", async () => {
  for (const malformed of [null, { ...stateValue(), families: [null] }, { ...stateValue(), job: job("succeeded") }]) {
    const h = harness(malformed);
    try { await h.app.start(); assert.equal(h.byId("family").disabled, true); assert.equal(h.byId("result").hidden, true); assert.match(h.byId("connection-status").textContent, /unconfirmed/); }
    finally { h.close(); }
  }
  const h = harness();
  try {
    h.fixture.help.operations[0].effect = "read";
    await h.app.start(); await h.choose(); assert.equal(h.byId("configuration").hidden, true); assert.equal(h.byId("validate").disabled, true);
    assert.match(h.byId("problem").textContent, /Could not load verified/);
  } finally { h.close(); }
});

test("uncertain install response permits only status recovery and does not retry or cancel host work on disposal", async () => {
  const h = harness(stateValue(), call => {
    if (call.path === "/api/install") throw new Error("RAW-CONFIG-PRIVATE-ERROR");
    return undefined;
  });
  try {
    await h.app.start(); await h.fill(); await h.submit(); h.byId("install").click(); await settle();
    assert.equal(h.byId("install").disabled, true); assert.equal(h.byId("family").disabled, true);
    assert.match(h.byId("problem").textContent, /response is unconfirmed/); assert.ok(!h.document.documentElement.outerHTML.includes("RAW-CONFIG-PRIVATE-ERROR"));
    h.byId("install").dispatchEvent(new h.dom.window.Event("click")); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/install").length, 1);
    const before = h.calls.length; h.app.dispose(); await settle(); assert.equal(h.calls.length, before); assert.equal(h.timers.size, 0);
    assert.ok(h.calls.every(call => !call.path.includes("cancel") && !Object.hasOwn(call.init, "signal")));
  } finally { h.close(); }
});


test("recovered request stays read-only and unknown after a matching check", async () => {
  const state = stateValue("unprepared");
  state.mode = "recovery";
  state.job = { ...job("unconfirmed"), recovered: true, error: "historical_outcome_unknown",
    recovery_request: { id: "1".repeat(32), sha256: "2".repeat(64) } };
  state.reconciliation = { available: true, observation: null };
  const h = harness(state, (call, fixture) => {
    if (call.path === "/api/reconcile") {
      fixture.state.reconciliation.observation = observation();
      return response({ observation: observation() });
    }
  });
  await h.app.start(); await settle();
  for (const name of ["family", "profile", "prepare", "validate", "install"]) assert.equal(h.byId(name).disabled, true, name);
  assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
  assert.match(h.byId("job-context").textContent, /Readback only/);
  assert.match(h.byId("job-context").textContent, /22222222/);
  h.byId("reconcile").click(); await settle();
  assert.equal(h.byId("result").hidden, false);
  assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
  assert.deepEqual(h.calls.filter(c => c.path !== "/api/state").map(c => c.path), ["/api/reconcile"]);
  h.app.dispose(); h.dom.window.close();
});

function recoverySelectionFixture(handler = () => undefined) {
  const items = ["1", "2"].map(id => ({ ...job("unconfirmed", "install", id.repeat(32)), recovered: true,
    error: "historical_outcome_unknown", recovery_request: { id: id.repeat(32), sha256: id.repeat(64) } }));
  const state = { ...stateValue("unprepared"), mode: "recovery", job: items[1],
    job_history: { available: true, count: 2, recovery_selection: true },
    reconciliation: { available: true, observation: null } };
  const observed = new Map();
  const summaries = items.map(item => ({ ...jobSummary(item), profile,
    recovered: true, recovery_request: item.recovery_request })).reverse();
  const h = harness(state, (call, fixture) => {
    const handled = handler(call, fixture, { items, summaries, observed });
    if (handled !== undefined) return handled;
    if (call.path === "/api/jobs") return response({ jobs: summaries, next_before: null });
    if (call.path === "/api/job") return response({ job: items.find(item => item.id === call.payload.job_id),
      reconciliation: { available: true, observation: observed.get(call.payload.job_id) ?? null } });
    if (call.path === "/api/reconcile") {
      const value = observation(call.payload.job_id === items[0].id ? "unconfirmed" : "matched", call.payload.job_id);
      observed.set(value.job_id, value);
      if (value.job_id === fixture.state.job.id) fixture.state.reconciliation.observation = value;
      return response({ observation: value });
    }
  });
  return Object.assign(h, { items, summaries, observed });
}

test("recovery selection checks two retained requests independently without enabling effects", async () => {
  const h = recoverySelectionFixture();
  try {
    await h.app.start();
    assert.equal(h.byId("job-history").hidden, false);
    assert.match(h.byId("current-job-status").textContent, /2 host-selected requests.*reverse selection order/);
    assert.equal(h.byId("job-choice").options[0].textContent, "Last request in recovery selection");
    for (const name of ["family", "profile", "prepare", "validate", "install"]) assert.equal(h.byId(name).disabled, true, name);
    h.byId("reconcile").click(); await settle();
    assert.equal(h.byId("result").hidden, false);
    h.change(h.byId("job-choice"), h.items[0].id); await settle();
    assert.equal(h.byId("result").hidden, true);
    h.byId("reconcile").click(); await settle();
    assert.match(h.byId("reconciliation-status").textContent, /could not be confirmed/);
    assert.equal(h.byId("result").hidden, true);
    h.change(h.byId("job-choice"), ""); await settle();
    assert.equal(h.byId("result").hidden, false);
    assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
    assert.deepEqual(h.calls.filter(call => call.path === "/api/reconcile").map(call => call.payload.job_id), [h.items[1].id, h.items[0].id]);
    for (const name of ["prepare", "install", "retry-prepare", "retry-export", "retry-selected", "stop-job"]) {
      h.byId(name).dispatchEvent(new h.dom.window.Event("click"));
    }
    await settle();
    assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job", "/api/reconcile"].includes(call.path)));
  } finally { h.close(); }
});

test("recovery selection rejects changed request anchors and restored success claims", async () => {
  for (const mutate of [
    value => { value.job.recovery_request.sha256 = "e".repeat(64); },
    value => { value.job.status = "succeeded"; value.job.result = completed().result; },
    value => { value.job.result = completed().result; },
    value => { value.job.error = "recovered_success"; },
  ]) {
    const h = recoverySelectionFixture((call, _fixture, { items }) => {
      if (call.path === "/api/job") {
        const value = { job: clone(items[0]), reconciliation: { available: true, observation: null } };
        mutate(value); return response(value);
      }
    });
    try {
      await h.app.start(); h.change(h.byId("job-choice"), h.items[0].id); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.notEqual(h.byId("problem").textContent, "");
      assert.ok(h.calls.every(call => call.path !== "/api/reconcile"));
    } finally { h.close(); }
  }
});

test("recovery selection cannot change membership or latest request identity on refresh", async () => {
  for (const mutate of [
    state => { state.job_history.count = 3; },
    state => { delete state.job_history.recovery_selection; },
    state => { state.job.recovery_request.sha256 = "f".repeat(64); },
    state => { state.job.family = "services"; },
    state => { state.job.kind = "prepare"; },
  ]) {
    const h = recoverySelectionFixture();
    try {
      await h.app.start(); mutate(h.fixture.state); h.byId("refresh").click(); await settle();
      assert.equal(h.byId("reconcile").disabled, true);
      assert.equal(h.byId("result").hidden, true);
      assert.notEqual(h.byId("problem").textContent, "");
    } finally { h.close(); }
  }
});

test("recovery selection reads multiple history pages without replaying requests", async () => {
  const h = recoverySelectionFixture(call => {
    if (call.path === "/api/jobs") return response(call.payload.before === null ?
      { jobs: [h.summaries[0]], next_before: h.items[1].id } : { jobs: [h.summaries[1]], next_before: null });
  });
  try {
    await h.app.start(); assert.equal(h.byId("jobs-earlier").hidden, false);
    h.byId("jobs-earlier").click(); await settle();
    assert.equal(h.byId("jobs-earlier").hidden, true);
    h.change(h.byId("job-choice"), h.items[0].id); await settle();
    assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
    assert.equal(h.calls.filter(call => call.path === "/api/reconcile").length, 0);
    assert.deepEqual(h.calls.filter(call => call.path === "/api/jobs").map(call => call.payload.before), [null, h.items[1].id]);
  } finally { h.close(); }
});

test("selected Python, Node and MCPB installation progress reaches the actual result without library controls", async () => {
  for (const [kind, software] of selectedForms) {
    const label = { python: "Python", node: "Node", mcpb: "MCPB (Node)" }[kind];
    const h = harness(selectedState(kind));
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /Connected/);
      assert.equal(h.byId("job-status").textContent, `Selected ${label} package installation is running in the local host.`);
      assert.equal(h.byId("family").disabled, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.equal(h.timers.size, 1);
      h.fixture.state = selectedState(kind, "succeeded", software);
      h.byId("refresh").click(); await settle();
      assert.equal(h.timers.size, 1);
      assert.equal(h.byId("result").hidden, false);
      assert.match(h.byId("job-status").textContent, /completed/);
      assert.match(h.byId("result-paths").textContent, /Componentselected-server/);
      assert.match(h.byId("result-paths").textContent, /Selection SHA-256/);
      assert.match(h.byId("result-paths").textContent, /Installation effect SHA-256/);
      assert.ok(h.byId("result-paths").textContent.includes(`${software ? "Software declaration digest" : "Manifest digest"}${"b".repeat(64)}`));
      assert.ok(h.byId("result-paths").textContent.includes(`Selection type${label} package`));
      if (kind === "mcpb") assert.ok(h.byId("result-paths").textContent.includes(`Configuration SHA-256${"e".repeat(64)}`));
      else assert.doesNotMatch(h.byId("result-paths").textContent, /Configuration SHA-256/);
      assert.doesNotMatch(h.byId("result-paths").textContent, /undefined|Validated profile|Checked profile/);
      assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent), selectedResult(kind, "observed_software_installation_only", software).mcp_configuration);
      assert.match(h.byId("result-credential-references").textContent, /requirements are unknown/);
      assert.equal(h.byId("reconcile").disabled, false);
      assert.ok(h.calls.every(call => call.path === "/api/state"));
    } finally { h.close(); }
  }
});

test("selected reconciliation replaces the displayed observation without rewriting installation history", async () => {
  for (const [kind, software] of selectedForms) {
    const pending = deferred();
    const h = harness(selectedState(kind, "succeeded", software), call => call.path === "/api/reconcile" ? pending.promise : undefined);
    try {
      await h.app.start(); h.byId("reconcile").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.match(h.byId("job-status").textContent, /completed/);
      const matched = selectedObservation(kind, "matched", 1, software);
      h.fixture.state.reconciliation.observation = matched;
      pending.resolve(response({ observation: matched })); await settle();
      assert.equal(h.byId("result").hidden, false);
      assert.match(h.byId("reconciliation-status").textContent, /match the original request/);
      assert.match(h.byId("result-paths").textContent, /Observed receipt SHA-256/);
      if (kind === "mcpb") assert.ok(h.byId("result-paths").textContent.includes(`Configuration SHA-256${"e".repeat(64)}`));
      assert.match(h.byId("result-credential-references").textContent, /did not revalidate credential references/);
      assert.deepEqual(h.calls.filter(call => call.path === "/api/reconcile").map(call => call.payload), [{ job_id: "1".repeat(32) }]);
      h.fixture.state.reconciliation.observation = selectedObservation(kind, "unconfirmed", 2, software);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.match(h.byId("job-status").textContent, /completed/);
      assert.match(h.byId("reconciliation-status").textContent, /could not be confirmed/);
    } finally { h.close(); }
  }
});

test("selected recovered Python, Node and MCPB requests can be checked without restoring installation authority", async () => {
  for (const [kind, software] of selectedForms) {
    const state = selectedState(kind, "unconfirmed");
    state.mode = "recovery"; delete state.selected_installation;
    state.job.recovered = true;
    const h = harness(state, (call, fixture) => {
      if (call.path === "/api/reconcile") {
        fixture.state.reconciliation.observation = selectedObservation(kind, "matched", 1, software);
        return response({ observation: selectedObservation(kind, "matched", 1, software) });
      }
    });
    try {
      await h.app.start();
      assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
      assert.equal(h.byId("reconcile").disabled, false);
      h.byId("reconcile").click(); await settle();
      assert.equal(h.byId("result").hidden, false);
      assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
      assert.match(h.byId("result-paths").textContent, /Componentselected-server/);
      if (kind === "mcpb") assert.ok(h.byId("result-paths").textContent.includes(`Configuration SHA-256${"e".repeat(64)}`));
      for (const control of ["family", "profile", "prepare", "validate", "install"]) assert.equal(h.byId(control).disabled, true);
      assert.deepEqual(h.calls.filter(call => call.path !== "/api/state").map(call => call.path), ["/api/reconcile"]);
    } finally { h.close(); }
  }
});

test("selected malformed jobs and mismatched readback kinds remain unconfirmed without exposing results", async () => {
  const mutations = [
    state => { state.job.family = "selected-ruby"; },
    state => { state.job.family = ["selected-python"]; },
    state => { state.job.kind = "prepare"; },
    state => { delete state.selected_installation; },
    state => { state.selected_installation.profiles = ["node"]; },
    state => { state.selected_installation.scope = "claimed_permission"; },
    state => { state.job.result.selection_kind = "node"; },
    state => { delete state.job.result.component; },
    state => { state.job.result.selection_sha256 = "invalid"; },
    state => { state.job.result.scope = "retained_payload_and_launch_binding_only"; },
    state => { state.job.result.family = "agno"; state.job.result.profile = profile; },
    state => { state.job.result.credential_environment_references = []; },
    state => { state.reconciliation.observation = selectedObservation("node"); },
    state => { state.reconciliation.observation = selectedObservation(); state.reconciliation.observation.job_id = "2".repeat(32); },
    state => { state.reconciliation.observation = selectedObservation(); delete state.reconciliation.observation.result.effect_sha256; },
    state => { state.reconciliation.observation = selectedObservation(); state.reconciliation.observation.result.component = "other-component"; },
  ];
  for (const mutate of mutations) {
    const state = selectedState("python", "succeeded"); mutate(state);
    const h = harness(state);
    try {
      await h.app.start();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.equal(h.byId("install").disabled, true);
      assert.ok(h.calls.every(call => call.path === "/api/state"));
    } finally { h.close(); }
  }
});

test("selected results reject contradictory document identities without granting installation authority", async () => {
  for (const [kind, software] of selectedForms) {
    const mutations = [
      result => { result.document_kind = null; },
      result => { result.document_kind = "activation_manifest"; },
      result => { result.document_kind = "unknown"; },
      result => { result[software ? "manifest_digest" : "declaration_digest"] = "b".repeat(64); },
      result => { delete result[software ? "declaration_digest" : "manifest_digest"]; },
      result => { result[software ? "declaration_digest" : "manifest_digest"] = "B".repeat(64); },
      ...(software ? [result => { delete result.document_kind; }] : []),
      ...(kind === "mcpb" ? [result => {
        delete result.manifest_digest; result.document_kind = "local_software_declaration"; result.declaration_digest = "b".repeat(64);
      }] : []),
    ];
    for (const mutate of mutations) {
      const state = selectedState(kind, "succeeded", software); mutate(state.job.result);
      const h = harness(state);
      try {
        await h.app.start();
        assert.match(h.byId("connection-status").textContent, /unconfirmed/);
        assert.equal(h.byId("result").hidden, true);
        assert.equal(h.byId("install").disabled, true);
        assert.ok(h.calls.every(call => call.path === "/api/state"));
      } finally { h.close(); }
    }
  }
});

test("selected readback cannot change the document form raw hash or canonical digest", async () => {
  for (const kind of ["python", "node"]) for (const software of [false, true]) {
    const changed = [
      selectedObservation(kind, "matched", 1, !software),
      selectedObservation(kind, "matched", 1, software),
      selectedObservation(kind, "matched", 1, software),
    ];
    changed[1].result.selection_sha256 = "f".repeat(64);
    changed[2].result[software ? "declaration_digest" : "manifest_digest"] = "f".repeat(64);
    for (const observed of changed) {
      const h = harness(selectedState(kind, "succeeded", software), call =>
        call.path === "/api/reconcile" ? response({ observation: observed }) : undefined);
      try {
        await h.app.start(); h.byId("reconcile").click(); await settle();
        assert.equal(h.byId("result").hidden, true);
        assert.match(h.byId("job-status").textContent, /completed/);
        assert.match(h.byId("problem").textContent, /installation check response is unconfirmed/);
        assert.equal(h.byId("reconcile").disabled, true);
        assert.deepEqual(h.calls.filter(call => call.path !== "/api/state").map(call => call.path), ["/api/reconcile"]);
      } finally { h.close(); }
    }
  }
});

test("software declaration history detail keeps its form and readback after a later installation", async () => {
  for (const kind of ["python", "node"]) {
    const earlier = selectedState(kind, "succeeded", true).job;
    const current = selectedState(kind, "succeeded"); current.job.id = "2".repeat(32);
    current.job_history = { available: true, count: 2 };
    const h = harness(current, call => {
      if (call.path === "/api/jobs") return response({ jobs: [jobSummary(current.job), jobSummary(earlier)], next_before: null });
      if (call.path === "/api/job") return response({ job: earlier, reconciliation: {
        available: true, observation: selectedObservation(kind, "matched", 1, true) } });
    });
    try {
      await h.app.start(); h.change(h.byId("job-choice"), earlier.id); await settle();
      assert.equal(h.byId("problem").textContent, "");
      assert.equal(h.byId("result").hidden, false);
      assert.ok(h.byId("result-paths").textContent.includes(`Software declaration digest${"b".repeat(64)}`));
      assert.doesNotMatch(h.byId("result-paths").textContent, /Manifest digest|undefined/);
      h.change(h.byId("job-choice"), ""); await settle();
      assert.ok(h.byId("result-paths").textContent.includes(`Manifest digest${"b".repeat(64)}`));
      assert.ok(h.calls.every(call => ["/api/state", "/api/jobs", "/api/job"].includes(call.path)));
    } finally { h.close(); }
  }
});

test("selected cached observations cannot be rebound to a different current job kind or selection", async () => {
  const library = stateValue(); library.job = { ...completed(), id: "1".repeat(32) };
  library.reconciliation = { available: true, observation: null };
  const changedSelection = selectedState("python", "succeeded"); changedSelection.job.result.component = "different-component";
  for (const replacement of [selectedState("node", "succeeded"), library, changedSelection]) {
    const initial = selectedState("python", "succeeded"); initial.reconciliation.observation = selectedObservation();
    const h = harness(initial);
    try {
      await h.app.start();
      const originalPaths = h.byId("result-paths").textContent;
      h.fixture.state = replacement;
      h.byId("refresh").click(); await settle();
      assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result-paths").textContent, originalPaths);
      assert.match(h.byId("job-status").textContent, /Selected Python/);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.ok(h.calls.every(call => call.path === "/api/state"));
    } finally { h.close(); }
  }
});

test("selected MCPB requires a lowercase configuration hash in results and observations without library fallback", async () => {
  const mutations = [
    result => { delete result.configuration_sha256; },
    result => { result.configuration_sha256 = "E".repeat(64); },
    result => { result.configuration_sha256 = "e".repeat(63); },
    result => { result.configuration_sha256 = 123; },
    result => { result.family = "agno"; result.profile = profile; },
  ];
  for (const target of ["job", "observation"]) {
    for (const mutate of mutations) {
      const state = selectedState("mcpb", "succeeded");
      state.reconciliation.observation = selectedObservation("mcpb");
      mutate(target === "job" ? state.job.result : state.reconciliation.observation.result);
      const h = harness(state);
      try {
        await h.app.start();
        assert.match(h.byId("connection-status").textContent, /unconfirmed/);
        assert.equal(h.byId("result").hidden, true);
        assert.equal(h.byId("mcp-configuration").textContent, "");
        assert.equal(h.byId("reconcile").disabled, true);
        assert.equal(h.byId("install").disabled, true);
      } finally { h.close(); }
    }
  }
});

test("selected MCPB state and check responses bind readback to the completed configuration hash", async () => {
  for (const source of ["state", "reconcile"]) {
    const state = selectedState("mcpb", "succeeded"), mismatched = selectedObservation("mcpb");
    mismatched.result.configuration_sha256 = "f".repeat(64);
    if (source === "state") state.reconciliation.observation = mismatched;
    const h = harness(state, call => call.path === "/api/reconcile" ? response({ observation: mismatched }) : undefined);
    try {
      await h.app.start();
      if (source === "reconcile") {
        assert.equal(h.byId("result").hidden, false);
        h.byId("reconcile").click(); await settle();
        assert.match(h.byId("problem").textContent, /check response is unconfirmed/);
      } else assert.match(h.byId("connection-status").textContent, /unconfirmed/);
      assert.equal(h.byId("result").hidden, true);
      assert.doesNotMatch(h.byId("result-paths").textContent, /f{64}/);
      assert.equal(h.byId("reconcile").disabled, true);
      assert.equal(h.calls.filter(call => call.path === "/api/state").length, 1);
      assert.ok(h.calls.every(call => ["/api/state", "/api/reconcile"].includes(call.path)));
    } finally { h.close(); }
  }
});

test("selected MCPB historical readback validates and retains its configuration binding", async () => {
  const earlier = selectedState("mcpb", "succeeded"), current = selectedState("node", "succeeded");
  current.job.id = "2".repeat(32);
  current.selected_installation.profiles.push("mcpb");
  current.job_history = { available: true, count: 2 };
  const detail = { job: earlier.job, reconciliation: { available: true, observation: selectedObservation("mcpb") } };
  const h = harness(current, call => call.path === "/api/jobs" ?
    response({ jobs: [jobSummary(current.job), jobSummary(earlier.job)], next_before: null }) :
    call.path === "/api/job" ? response(detail) : undefined);
  try {
    await h.app.start(); h.change(h.byId("job-choice"), earlier.job.id); await settle();
    assert.equal(h.byId("result").hidden, false);
    assert.match(h.byId("job-status").textContent, /MCPB \(Node\)/);
    assert.ok(h.byId("result-paths").textContent.includes(`Configuration SHA-256${"e".repeat(64)}`));
    detail.reconciliation.observation.result.configuration_sha256 = "f".repeat(64);
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("result").hidden, true);
    assert.equal(h.byId("mcp-configuration").textContent, "");
    assert.equal(h.byId("reconcile").disabled, true);
  } finally { h.close(); }
});

test("selected MCPB cached observations cannot be rebound to a changed configuration hash", async () => {
  const state = selectedState("mcpb", "succeeded");
  state.reconciliation.observation = selectedObservation("mcpb");
  const h = harness(state);
  try {
    await h.app.start();
    const originalPaths = h.byId("result-paths").textContent;
    h.fixture.state = selectedState("mcpb", "succeeded");
    h.fixture.state.job.result.configuration_sha256 = "f".repeat(64);
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    assert.equal(h.byId("result-paths").textContent, originalPaths);
    assert.doesNotMatch(h.byId("result-paths").textContent, /f{64}/);
    assert.equal(h.byId("reconcile").disabled, true);
    assert.ok(h.calls.every(call => call.path === "/api/state"));
  } finally { h.close(); }
});

test("selected MCPB recovered requests refuse missing or malformed configuration hashes and stay unknown", async () => {
  for (const hash of [undefined, "E".repeat(64)]) {
    const state = selectedState("mcpb", "unconfirmed"), malformed = selectedObservation("mcpb");
    state.mode = "recovery"; delete state.selected_installation;
    state.job.recovered = true;
    if (hash === undefined) delete malformed.result.configuration_sha256;
    else malformed.result.configuration_sha256 = hash;
    const h = harness(state, call => call.path === "/api/reconcile" ? response({ observation: malformed }) : undefined);
    try {
      await h.app.start();
      assert.equal(h.byId("reconcile").disabled, false);
      h.byId("reconcile").click(); await settle();
      assert.equal(h.byId("result").hidden, true);
      assert.equal(h.byId("mcp-configuration").textContent, "");
      assert.match(h.byId("job-status").textContent, /historical installation outcome is unknown/);
      assert.match(h.byId("problem").textContent, /check response is unconfirmed/);
      for (const control of ["family", "profile", "prepare", "validate", "install", "reconcile"]) assert.equal(h.byId(control).disabled, true);
      assert.deepEqual(h.calls.map(call => call.path), ["/api/state", "/api/reconcile"]);
    } finally { h.close(); }
  }
});

const handoffValue = (references) => ({ family: "agno", profile, configuration_sha256: "a".repeat(64),
  contents_sha256: "c".repeat(64), ...(references === undefined ? {} : { credential_environment_references: references }) });
const referenceRow = (field = "token_env", name = "TOKEN_REF") => ({ field_path: ["config", field], environment_variable: name });

test("review distinguishes reported none, declared names and unsupported older validators", async () => {
  for (const references of [undefined, [], [referenceRow()]]) {
    const h = harness(stateValue(), call => call.path === "/api/check" ? response({ ...validationValue(), ...handoffValue(references) }) : undefined);
    try {
      await h.app.start(); await h.fill(); h.change(h.field("token_env"), "TOKEN_REF", "input"); await h.submit();
      assert.equal(h.byId("review").hidden, false);
      const text = h.byId("review-credential-references").textContent;
      if (references === undefined) assert.match(text, /requirements are unknown/);
      else if (!references.length) assert.match(text, /reports no credential environment references/);
      else { assert.match(text, /TOKEN_REF/); assert.match(text, /\["config","token_env"\]/); }
      assert.match(text, /No credential or account access was checked/);
      h.change(h.field("timeout"), "11", "input");
      assert.equal(h.byId("review").hidden, true);
      assert.equal(h.byId("review-credential-references").textContent, "");
    } finally { h.close(); }
  }
});

test("malformed or mismatched handoff output is never rendered or installed", async () => {
  const cases = [
    { ...handoffValue([referenceRow()]), family: "services" },
    { ...handoffValue([referenceRow()]), profile: "other-profile" },
    { ...handoffValue([referenceRow()]), profile: undefined },
    { ...handoffValue([referenceRow()]), contents_sha256: "invalid" },
    handoffValue(null), handoffValue([referenceRow("token_env", "SECRET value")]),
    handoffValue([{ ...referenceRow(), private: "PRIVATE_HANDOFF" }]),
    handoffValue([referenceRow(), referenceRow()]),
    handoffValue([{ field_path: ["config.token_env"], environment_variable: "TOKEN_REF" }]),
  ];
  for (const bad of cases) {
    const h = harness(stateValue(), call => call.path === "/api/check" ? response({ ...validationValue(), ...bad }) : undefined);
    try {
      await h.app.start(); await h.fill(); await h.submit();
      assert.equal(h.byId("review").hidden, true);
      assert.doesNotMatch(h.byId("review-credential-references").textContent, /PRIVATE_HANDOFF|TOKEN_REF|SECRET value/);
      h.byId("install").click(); await settle();
      assert.equal(h.calls.filter(call => call.path === "/api/install").length, 0);
    } finally { h.close(); }
  }
});

test("final result retains aliases while current file checks never present old names as current", async () => {
  const initial = stateValue(); initial.job = completed();
  initial.job.result = { ...initial.job.result, ...handoffValue([referenceRow("token_env", "SAME_REF"), referenceRow("other_env", "SAME_REF")]) };
  const h = harness(initial);
  try {
    await h.app.start();
    assert.equal(h.byId("result").hidden, false);
    assert.equal((h.byId("result-credential-references").textContent.match(/SAME_REF/g) ?? []).length, 2);
    assert.match(h.byId("result-paths").textContent, /Configuration SHA-256/);
    h.fixture.state.reconciliation = { available: true, observation: observation("matched") };
    // Even accidental extra data on a readback result must not revive the old projection.
    h.fixture.state.reconciliation.observation.result.credential_environment_references = [referenceRow("token_env", "STALE_REF")];
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("result-credential-references").textContent, /did not revalidate credential references/);
    assert.doesNotMatch(h.byId("result-credential-references").textContent, /SAME_REF|STALE_REF/);
    h.fixture.state.mode = "recovery"; h.fixture.state.job = { ...job("unconfirmed"), recovered: true };
    h.byId("refresh").click(); await settle();
    assert.match(h.byId("result-credential-references").textContent, /did not revalidate/);
    assert.equal(h.byId("validate").disabled, true);
    assert.equal(h.byId("install").disabled, true);
  } finally { h.close(); }
});

test("slow validation cannot attach reference names after configuration changes", async () => {
  const pending = deferred();
  const h = harness(stateValue(), call => call.path === "/api/check" ? pending.promise : undefined);
  try {
    await h.app.start(); await h.fill();
    h.byId("configuration-form").dispatchEvent(new h.dom.window.Event("submit", { bubbles: true, cancelable: true }));
    await settle(); h.change(h.field("timeout"), "11", "input");
    pending.resolve(response({ ...validationValue(), ...handoffValue([referenceRow("token_env", "STALE_REF")]) }));
    await settle();
    assert.equal(h.byId("review").hidden, true);
    assert.doesNotMatch(h.byId("review-credential-references").textContent, /STALE_REF/);
  } finally { h.close(); }
});

const catalogBinding = () => ({ scope: "catalog_metadata_only", catalog_sha256: "c".repeat(64), contents_sha256: "d".repeat(64) });
const catalogEntries = () => ["Slack", "Gmail"].map(name => ({
  identity: { publisher: "fixture", name: name.toLowerCase(), version: "metadata-sha256:" + "e".repeat(64) },
  display_name: name, description: name + " catalog description",
}));
const catalogEntry = row => ({ ...row, provenance: { source: "https://example.invalid/<script>", license: "MIT" },
  strategies: [{ id: "primary", kind: "configuration_recipe", instructions: "First line\\n<img src=x onerror=alert(1)>",
    sources: ["https://example.invalid/source"], unresolved_details: ["Account scopes remain unchecked."] },
    { id: "alternative", kind: "native_adapter", instructions: "Keep the alternative and its distinct account semantics." }],
  known_exports: [], assessments: [{ status: "experimental" }],
});
const catalogState = () => ({ ...stateValue("unprepared"), catalog: { available: true, ...catalogBinding() } });
const catalogKey = row => JSON.stringify([row.identity.publisher, row.identity.name, row.identity.version]);

test("catalog before preparation retains alternatives as text without choosing any setup route", async () => {
  const rows = catalogEntries();
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
    if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(), entry: catalogEntry(rows.find(r => r.identity.name === call.payload.name)) });
  });
  try {
    await h.app.start();
    assert.deepEqual(h.calls.map(c => c.path), ["/api/state", "/api/catalog"]);
    assert.equal(h.byId("catalog-choice").options.length, 2);
    assert.equal(h.byId("catalog-choice").value, "");
    h.byId("catalog-choice").focus();
    h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
    assert.equal(h.byId("catalog-title").textContent, "Slack");
    const labels = [...h.byId("catalog-strategies").querySelectorAll("h5")].map(heading => heading.textContent);
    assert.deepEqual(labels, ["Option 1: Manual setup - configuration recipe / primary", "Option 2: Manual setup - native adapter / alternative"]);
    assert.equal(h.byId("catalog-strategy-details").querySelectorAll("details").length, 2);
    assert.ok([...h.byId("catalog-detail").querySelectorAll("details")].every(detail => !detail.open));
    assert.deepEqual(JSON.parse(h.byId("catalog-record").textContent), catalogEntry(rows[0]));
    assert.equal(h.byId("catalog-detail").querySelectorAll("img,script,a").length, 0);
    assert.match(h.byId("catalog-next").textContent, /does not choose a library or profile/);
    assert.match(h.byId("catalog-strategy-details").textContent, /First line\n<img/);
    assert.match(h.byId("catalog-strategy-details").textContent, /Account scopes remain unchecked/);
    assert.match(h.byId("catalog-next").textContent, /No setup action is currently available/);
    assert.equal(h.byId("family").value, ""); assert.equal(h.byId("profile").value, "");
    assert.equal(h.document.activeElement.id, "catalog-choice");
    h.change(h.byId("catalog-filter"), "GMA", "input");
    assert.equal(h.byId("catalog-choice").options.length, 1);
    assert.equal(h.byId("catalog-detail").hidden, true);
    h.change(h.byId("catalog-filter"), "mailing my coworkers", "input");
    assert.equal(h.byId("catalog-choice").options.length, 0);
    assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
  } finally { h.close(); }
});

test("catalog visible version labels preserve exact identities without selecting or ranking an entry", async () => {
  const row = catalogEntries()[0], rows = [row,
    { ...row, identity: { ...row.identity, version: "discovery-2026-09-30.2" } },
    { ...row, identity: { ...row.identity, version: "metadata-sha256:" + "e".repeat(63) + "f" } }];
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
    if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(),
      entry: catalogEntry(rows.find(row => row.identity.version === call.payload.version)) });
  });
  try {
    await h.app.start();
    const options = [...h.byId("catalog-choice").options];
    assert.equal(h.byId("catalog-choice").value, "");
    assert.deepEqual(options.map(option => option.value), rows.map(catalogKey));
    assert.equal(new Set(options.map(option => option.textContent)).size, rows.length);
    for (let i = 0; i < rows.length; i++) {
      assert.ok(options[i].textContent.includes(rows[i].identity.version));
      assert.ok(options[i].textContent.indexOf(rows[i].identity.version) < options[i].textContent.indexOf("fixture/slack"));
    }
    h.byId("catalog-choice").focus(); h.change(h.byId("catalog-choice"), catalogKey(rows[2])); await settle();
    assert.deepEqual(h.calls.find(call => call.path === "/api/catalog_entry").payload, rows[2].identity);
    assert.ok(h.byId("catalog-identity").textContent.includes(rows[2].identity.version));
    assert.equal(h.document.activeElement.id, "catalog-choice");
    assert.ok(h.calls.every(call => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(call.path)));
  } finally { h.close(); }
});

test("catalog setup controls precede closed literal metadata with distinct option names and no automatic effects", async () => {
  const row = { ...catalogEntries()[0], description: "Internal adoption/proof <img src=x onerror=alert(1)>" };
  const entry = catalogEntry(row);
  entry.strategies[0].instructions += "\n" + "Long retained instructions. ".repeat(400);
  entry.strategies[0].known_requirements = ["Use the approved account only."];
  const options = entry.strategies.map((strategy, i) => ({ identity: row.identity, strategy_id: strategy.id,
    family: "agno", profile: i ? "another-profile" : profile,
    artifact: { path: "agno/fixture.whl", sha256: "a".repeat(64) }, status: i ? "prepared" : "unprepared" }));
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: [row] });
    if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(), routes_sha256: "f".repeat(64), entry, setup_options: options });
  });
  try {
    await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(row)); await settle();
    const technical = h.byId("catalog-technical"), buttons = [...h.byId("catalog-strategies").querySelectorAll("button")];
    assert.equal(buttons.length, 2); assert.equal(technical.open, false);
    assert.ok(buttons.every(button => !button.closest("details") &&
      (button.compareDocumentPosition(technical) & h.dom.window.Node.DOCUMENT_POSITION_FOLLOWING)));
    assert.equal(new Set(buttons.map(button => button.getAttribute("aria-label"))).size, 2);
    options.forEach((option, index) => {
      assert.ok(buttons[index].getAttribute("aria-label").includes(option.profile));
      assert.ok(buttons[index].getAttribute("aria-label").includes("Agno library"));
    });
    assert.match(h.byId("catalog-description").textContent, /may prepare local software/);
    assert.match(h.byId("catalog-description").textContent, /Account access and readiness are checked separately/);
    assert.doesNotMatch(h.byId("catalog-description").textContent, /adoption|proof|Internal/);
    assert.equal(h.byId("catalog-source-description").textContent, row.description);
    assert.deepEqual(JSON.parse(h.byId("catalog-record").textContent), entry);
    assert.equal(technical.querySelectorAll("img,script,a").length, 0);
    assert.match(h.byId("catalog-strategy-details").textContent, /Use the approved account only/);
    assert.match(h.byId("catalog-strategy-details").textContent, /Account scopes remain unchecked/);
    assert.ok(technical.querySelector(":scope > summary"));
    technical.open = true;
    assert.equal(technical.querySelectorAll("details").length, 3);
    assert.ok([...technical.querySelectorAll("details")].every(detail => detail.querySelector(":scope > summary")));
    assert.ok([...h.byId("catalog-strategy-details").querySelectorAll("details")].every(detail => !detail.open));
    assert.equal(h.byId("family").value, ""); assert.equal(h.byId("profile").value, "");
    assert.ok(h.calls.every(call => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(call.path)));
  } finally { h.close(); }
});

test("catalog unavailable options keep distinct visible reasons without setup controls or fallback", async () => {
  const row = catalogEntries()[0], statuses = ["manual", "disabled", "preparing", "unconfirmed"];
  const entry = { ...catalogEntry(row), strategies: statuses.map(id => ({ id, kind: "local_mcp", instructions: "Exact retained " + id })) };
  const options = statuses.map(status => status === "manual" ? { strategy_id: status, status, reason: "no_bound_setup_route" } :
    { identity: row.identity, strategy_id: status, family: "agno", profile,
      artifact: { path: "agno/fixture.whl", sha256: "a".repeat(64) }, status });
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: [row] });
    if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(), routes_sha256: "f".repeat(64), entry, setup_options: options });
  });
  try {
    await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(row)); await settle();
    const cards = [...h.byId("catalog-strategies").children];
    assert.equal(cards.length, 4); assert.equal(h.byId("catalog-strategies").querySelectorAll("button").length, 0);
    [/Manual setup: no setup action/, /not enabled by the host/, /preparation is running/, /outcome is unconfirmed/].forEach((reason, index) => {
      assert.match(cards[index].textContent, reason); assert.equal(cards[index].closest("details"), null);
    });
    assert.match(h.byId("catalog-next").textContent, /No setup action is currently available/);
    assert.equal(h.byId("family").value, ""); assert.equal(h.byId("profile").value, "");
    assert.ok(h.calls.every(call => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(call.path)));
  } finally { h.close(); }
});

test("catalog entry retry recovers the same sole result once and returns keyboard focus to the list", async () => {
  const rows = catalogEntries(), pending = deferred(); let reads = 0;
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
    if (call.path === "/api/catalog_entry") return ++reads === 1 ? response({}, 503) : pending.promise;
  });
  try {
    await h.app.start(); h.change(h.byId("catalog-filter"), "Slack", "input");
    h.byId("catalog-choice").focus(); h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
    const retry = h.byId("catalog-entry-retry");
    assert.equal(h.byId("catalog-choice").options.length, 1);
    assert.equal(h.byId("catalog-choice").value, catalogKey(rows[0]));
    assert.equal(h.byId("catalog-detail").hidden, true);
    assert.equal(retry.hidden, false); assert.equal(retry.getAttribute("aria-disabled"), "false");
    assert.match(h.byId("catalog-status").textContent, /Retry entry details/);
    h.byId("refresh").click(); await settle(); assert.equal(reads, 1);
    retry.focus(); retry.click(); retry.click();
    retry.dispatchEvent(new h.dom.window.Event("click")); await settle();
    assert.equal(reads, 2); assert.equal(retry.hidden, false);
    assert.equal(retry.getAttribute("aria-disabled"), "true");
    assert.equal(h.document.activeElement, retry);
    h.change(h.byId("catalog-filter"), "Sla", "input");
    assert.equal(h.byId("catalog-choice").value, catalogKey(rows[0]));
    assert.equal(retry.hidden, false);
    pending.resolve(response({ ...catalogBinding(), entry: catalogEntry(rows[0]) })); await settle();
    assert.equal(h.byId("catalog-detail").hidden, false); assert.equal(retry.hidden, true);
    assert.equal(h.document.activeElement.id, "catalog-choice");
    assert.equal(h.byId("catalog-title").textContent, "Slack");
    assert.deepEqual(h.calls.filter(c => c.path === "/api/catalog_entry").map(c => c.payload), [rows[0].identity, rows[0].identity]);
    assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
  } finally { h.close(); }
});

test("catalog entry reads and retries do not take focus after the user moves elsewhere", async () => {
  for (const retrying of [false, true]) {
    const rows = catalogEntries(), pending = deferred(); let reads = 0;
    const h = harness(catalogState(), call => {
      if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
      if (call.path === "/api/catalog_entry") return retrying && ++reads === 1 ? response({}, 503) : pending.promise;
    });
    try {
      await h.app.start(); h.byId("catalog-choice").focus();
      h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
      if (retrying) { h.byId("catalog-entry-retry").focus(); h.byId("catalog-entry-retry").click(); await settle(); }
      h.byId("catalog-filter").focus();
      pending.resolve(response({ ...catalogBinding(), entry: catalogEntry(rows[0]) })); await settle();
      assert.equal(h.document.activeElement.id, "catalog-filter");
      assert.equal(h.byId("catalog-detail").hidden, false);
      assert.equal(h.byId("catalog-entry-retry").hidden, true);
      assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
    } finally { h.close(); }
  }
});

test("catalog entry retry cannot restore an old selection or filtered result from a late reply", async () => {
  for (const change of ["selection", "filter"]) for (const success of [false, true]) {
    const rows = catalogEntries(), pending = deferred(); let reads = 0;
    const h = harness(catalogState(), call => {
      if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
      if (call.path === "/api/catalog_entry") {
        reads += 1;
        return reads === 1 ? response({}, 503) : reads === 2 ? pending.promise :
          response({ ...catalogBinding(), entry: catalogEntry(rows[1]) });
      }
    });
    try {
      await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
      h.byId("catalog-entry-retry").click(); await settle();
      if (change === "selection") h.change(h.byId("catalog-choice"), catalogKey(rows[1]));
      else h.change(h.byId("catalog-filter"), "Gmail", "input");
      await settle();
      const status = h.byId("catalog-status").textContent, before = reads;
      h.byId("catalog-entry-retry").dispatchEvent(new h.dom.window.Event("click")); await settle();
      assert.equal(reads, before); assert.equal(h.byId("catalog-entry-retry").hidden, true);
      pending.resolve(success ? response({ ...catalogBinding(), entry: catalogEntry(rows[0]) }) : response({}, 503)); await settle();
      assert.equal(h.byId("catalog-status").textContent, status);
      assert.equal(h.byId("catalog-entry-retry").hidden, true);
      assert.equal(h.byId("catalog-detail").hidden, change === "filter");
      assert.equal(h.byId("catalog-choice").value, change === "filter" ? "" : catalogKey(rows[1]));
      if (change === "selection") assert.equal(h.byId("catalog-title").textContent, "Gmail");
      assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
    } finally { h.close(); }
  }
});

test("catalog filtering retains the exact selected entry and retry until that entry is filtered out", async () => {
  const rows = catalogEntries(); let reads = 0;
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
    if (call.path === "/api/catalog_entry") { reads += 1; return response({}, 503); }
  });
  try {
    await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
    assert.equal(h.byId("catalog-entry-retry").hidden, false);
    h.change(h.byId("catalog-filter"), "Slack", "input");
    assert.equal(h.byId("catalog-choice").value, catalogKey(rows[0]));
    assert.equal(h.byId("catalog-choice").options.length, 1);
    assert.equal(h.byId("catalog-entry-retry").hidden, false);
    assert.equal(h.byId("catalog-entry-retry").getAttribute("aria-disabled"), "false");
    h.byId("catalog-entry-retry").click(); await settle(); assert.equal(reads, 2);
    h.change(h.byId("catalog-filter"), "Gmail", "input");
    assert.equal(h.byId("catalog-choice").value, "");
    assert.equal(h.byId("catalog-entry-retry").hidden, true);
    h.byId("catalog-entry-retry").dispatchEvent(new h.dom.window.Event("click")); await settle();
    assert.equal(reads, 2);
    assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
  } finally { h.close(); }
});

test("catalog entry retry stays closed after disposal or recovery entry", async () => {
  for (const close of ["dispose", "recovery"]) for (const success of [false, true]) {
    const rows = catalogEntries(), pending = deferred(); let reads = 0;
    const h = harness(catalogState(), call => {
      if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
      if (call.path === "/api/catalog_entry") return ++reads === 1 ? response({}, 503) : pending.promise;
    });
    try {
      await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
      h.byId("catalog-entry-retry").click(); await settle();
      if (close === "dispose") h.app.dispose();
      else { h.fixture.state.mode = "recovery"; h.byId("refresh").click(); await settle(); }
      const status = h.byId("catalog-status").textContent;
      pending.resolve(success ? response({ ...catalogBinding(), entry: catalogEntry(rows[0]) }) : response({}, 503)); await settle();
      h.byId("catalog-entry-retry").dispatchEvent(new h.dom.window.Event("click")); await settle();
      assert.equal(reads, 2); assert.equal(h.byId("catalog-status").textContent, status);
      assert.equal(h.byId("catalog-entry-retry").hidden, true);
      assert.equal(h.byId("catalog-detail").hidden, true);
      assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
    } finally { h.close(); }
  }
});

test("catalog entry retry still refuses wrong catalog and entry identities", async () => {
  for (const mismatch of ["catalog", "entry"]) {
    const rows = catalogEntries(); let reads = 0;
    const h = harness(catalogState(), call => {
      if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
      if (call.path === "/api/catalog_entry") return ++reads === 1 ? response({}, 503) : response({
        ...catalogBinding(), ...(mismatch === "catalog" ? { catalog_sha256: "f".repeat(64) } : {}),
        entry: catalogEntry(rows[mismatch === "entry" ? 1 : 0]),
      });
    });
    try {
      await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
      h.byId("catalog-entry-retry").click(); await settle();
      assert.equal(reads, 2); assert.equal(h.byId("catalog-detail").hidden, true);
      assert.equal(h.byId("catalog-entry-retry").hidden, false);
      assert.equal(h.byId("catalog-entry-retry").getAttribute("aria-disabled"), "false");
      assert.equal(h.byId("catalog-choice").value, catalogKey(rows[0]));
      assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry"].includes(c.path)));
    } finally { h.close(); }
  }
});

test("delayed catalog detail cannot replace a later exact identity or recovery session", async () => {
  const rows = catalogEntries(), pending = [deferred(), deferred()]; let index = 0;
  const h = harness(catalogState(), call => {
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: rows });
    if (call.path === "/api/catalog_entry") return pending[index++].promise;
  });
  try {
    await h.app.start();
    h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle();
    h.change(h.byId("catalog-choice"), catalogKey(rows[1])); await settle();
    pending[1].resolve(response({ ...catalogBinding(), entry: catalogEntry(rows[1]) })); await settle();
    assert.equal(h.byId("catalog-title").textContent, "Gmail");
    h.fixture.state.mode = "recovery"; h.byId("refresh").click(); await settle();
    pending[0].resolve(response({ ...catalogBinding(), entry: catalogEntry(rows[0]) })); await settle();
    assert.equal(h.byId("catalog-detail").hidden, true);
    assert.equal(h.byId("catalog-controls").hidden, true);
    assert.match(h.byId("catalog-status").textContent, /recovery/);
  } finally { h.close(); }
});

test("wrong catalog or entry identity bindings refuse presentation without preparing", async () => {
  for (const fail of ["catalog", "entry"]) {
    const rows = catalogEntries();
    const h = harness(catalogState(), call => {
      if (call.path === "/api/catalog") return response({ ...catalogBinding(), ...(fail === "catalog" ? { catalog_sha256: "f".repeat(64) } : {}), entries: rows });
      if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(), entry: catalogEntry(rows[1]) });
    });
    try {
      await h.app.start();
      if (fail === "entry") { h.change(h.byId("catalog-choice"), catalogKey(rows[0])); await settle(); }
      assert.equal(h.byId("catalog-detail").hidden, true);
      assert.match(h.byId("catalog-status").textContent, /could not/);
      assert.equal(h.byId("family").value, "");
      assert.ok(!h.calls.some(c => c.path === "/api/prepare"));
    } finally { h.close(); }
  }
});

test("official GitHub preparation is metadata only and fixed operations/reference remain explicit", async () => {
  const ops = ["get_me", "list_commits", "get_file_contents"].map(id => ({ id, effect: "read" }));
  const state = { families: [{ id: "official-github", status: "unprepared", profiles: [] }], job: null, scope: "software only" };
  const h = harness(state, (call, fixture) => {
    if (call.path === "/api/prepare") {
      fixture.state.families[0] = { id: "official-github", status: "prepared", profiles: [{ id: "selected-reads", operations: ops }] };
      fixture.state.job = { id: "metadata", family: "official-github", kind: "prepare", status: "succeeded", result: { scope: "official_metadata_only" } };
      return response({ job: fixture.state.job });
    }
    if (call.path === "/api/help") return response({ family: "official-github", profile: "selected-reads", operations: ops, selectable_operations: false,
      fields: [{ name: "token_env", kind: "string", required: true, credential_reference: true }],
      prerequisites: [{ kind: "fixed_route", instruction: "Select all three operations. Use only GITHUB_PERSONAL_ACCESS_TOKEN." }],
      unresolved_details: ["The host supplies the account; no secret is read here."], scope: "configuration_only" });
    if (call.path === "/api/check") return response({ ...validationValue(), family: "official-github", profile: "selected-reads", contents_sha256: "b".repeat(64),
      credential_environment_references: [{ field_path: ["config", "token_env"], environment_variable: "GITHUB_PERSONAL_ACCESS_TOKEN" }] });
  });
  try {
    await h.app.start(); h.change(h.byId("family"), "official-github");
    assert.match(h.byId("preparation-description").textContent, /metadata only/);
    h.byId("prepare").click(); await settle(); h.change(h.byId("profile"), "selected-reads"); h.byId("show-profile").click(); await settle();
    assert.equal(h.field("token_env").value, ""); assert.equal(h.byId("operations").querySelectorAll(":checked").length, 3);
    assert.ok(Array.from(h.byId("operations").querySelectorAll("input")).every(input => input.disabled));
    assert.equal(h.byId("operations-fieldset").querySelector("legend").textContent, "Fixed operations");
    h.change(h.field("token_env"), "GITHUB_PERSONAL_ACCESS_TOKEN", "input"); await h.submit();
    const check = h.calls.find(c => c.path === "/api/check");
    assert.deepEqual(check.payload, { family: "official-github", profile: "selected-reads", operations: ops.map(o => o.id), values: { token_env: "GITHUB_PERSONAL_ACCESS_TOKEN" } });
    assert.equal(h.byId("review").hidden, false); assert.match(h.byId("review-credential-references").textContent, /GITHUB_PERSONAL_ACCESS_TOKEN/);
  } finally { h.close(); }
});


test("official Playwright keeps fixed coverage checked without suggesting a tool filter", async () => {
  const ops = ["browser_navigate", "browser_snapshot", "browser_fill_form", "browser_click", "browser_close"].map(id => ({ id, effect: "unspecified" }));
  const state = { families: [{ id: "official-playwright", status: "prepared", profiles: [{ id: "isolated-chrome", operations: ops }] }], job: null, scope: "software only" };
  const h = harness(state, call => {
    if (call.path === "/api/help") return response({ family: "official-playwright", profile: "isolated-chrome", operations: ops, selectable_operations: false, fields: [],
      prerequisites: [{ kind: "whole_endpoint", instruction: "All endpoint tools are exposed; these five operations are coverage, not a filter." }],
      unresolved_details: ["No browser has been launched."], scope: "configuration_only" });
    if (call.path === "/api/check") return response({ ...validationValue(), family: "official-playwright", profile: "isolated-chrome", contents_sha256: "b".repeat(64), credential_environment_references: [] });
  });
  try {
    await h.app.start(); h.change(h.byId("family"), "official-playwright"); h.change(h.byId("profile"), "isolated-chrome"); h.byId("show-profile").click(); await settle();
    const boxes = Array.from(h.byId("operations").querySelectorAll("input"));
    assert.equal(boxes.length, 5); assert.ok(boxes.every(b => b.checked && b.disabled));
    assert.match(h.byId("preparation-description").textContent, /metadata only/);
    await h.submit();
    assert.deepEqual(h.calls.find(c => c.path === "/api/check").payload, { family: "official-playwright", profile: "isolated-chrome", operations: ops.map(o => o.id), values: {} });
    assert.equal(h.byId("review").hidden, false); assert.match(h.byId("review").textContent, /whole endpoint exposed/);
    assert.ok(boxes.every(b => b.disabled));
  } finally { h.close(); }
});


test("operation selectability is a strict owner declaration with a legacy default", () => {
  const source = helpValue();
  for (const flag of [false, true]) assert.equal(normalizeHelp({ ...source, selectable_operations: flag }, "agno", profile).selectable_operations, flag);
  const legacy = { ...source }; delete legacy.selectable_operations;
  assert.equal(normalizeHelp(legacy, "agno", profile).selectable_operations, true);
  for (const flag of [null, 0, 1, "false", [], {}]) assert.throws(() => normalizeHelp({ ...source, selectable_operations: flag }, "agno", profile), /invalid_help/);
});


test("operation controls follow the declared flag independently of family", async () => {
  for (const flag of [false, true, undefined]) {
    const h = harness();
    if (flag === undefined) delete h.fixture.help.selectable_operations;
    else h.fixture.help.selectable_operations = flag;
    try {
      await h.app.start(); await h.choose();
      const fixed = flag === false;
      const boxes = Array.from(h.byId("operations").querySelectorAll("input"));
      assert.equal(boxes.length, 2);
      assert.ok(boxes.every(box => box.checked === fixed && box.disabled === fixed));
      assert.equal(h.byId("operations-fieldset").querySelector("legend").textContent, fixed ? "Fixed operations" : "Choose operations");
      assert.ok(!h.calls.some(call => ["/api/check", "/api/install"].includes(call.path)));
    } finally { h.close(); }
  }
});

const routeOption = (status = "unprepared") => ({ identity: catalogEntries()[0].identity, strategy_id: "primary", family: "agno", profile,
  artifact: { path: "agno/fixture.whl", sha256: "a".repeat(64) }, status });
function routeHarness(status = "unprepared", override = () => undefined) {
  const initial = { ...catalogState(), ...stateValue(status) };
  return harness(initial, async (call, fixture) => {
    const changed = await override(call, fixture);
    if (changed !== undefined) return changed;
    if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: catalogEntries() });
    if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(), routes_sha256: "f".repeat(64),
      entry: catalogEntry(catalogEntries().find(r => r.identity.name === call.payload.name)),
      setup_options: call.payload.name === "slack" ? [routeOption(fixture.state.families[0].status), { strategy_id: "alternative", status: "manual", reason: "no_bound_setup_route" }] :
        ["primary", "alternative"].map(strategy_id => ({ strategy_id, status: "manual", reason: "no_bound_setup_route" })) });
    if (call.path === "/api/catalog_route") {
      const option = routeOption(); const { identity, strategy_id, family, profile: id, artifact } = option;
      return response({ ...catalogBinding(), scope: "catalog_setup_handoff", routes_sha256: "f".repeat(64), route: { identity, strategy_id, family, profile: id }, artifact,
        runtime_target: null, status: fixture.state.families[0].status === "prepared" ? "prepared" : "preparation_required",
        ...(fixture.state.families[0].status === "prepared" ? { help: helpValue() } : {}) });
    }
  });
}
async function openRoute(h) {
  await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(catalogEntries()[0])); await settle();
}
test("catalog filtering keeps an exact visible selection and its pending setup route", async () => {
  const gate = deferred(), h = routeHarness("prepared", call => call.path === "/api/catalog_route" ? gate.promise : undefined);
  try {
    await openRoute(h);
    h.byId("catalog-strategies").querySelector("button").click(); await settle();
    h.change(h.byId("catalog-filter"), "Slack", "input");
    assert.equal(h.byId("catalog-choice").value, catalogKey(catalogEntries()[0]));
    assert.equal(h.byId("catalog-detail").hidden, false);
    const { identity, strategy_id, family, profile: id, artifact } = routeOption("prepared");
    gate.resolve(response({ ...catalogBinding(), scope: "catalog_setup_handoff", routes_sha256: "f".repeat(64),
      route: { identity, strategy_id, family, profile: id }, artifact, runtime_target: null,
      status: "prepared", help: helpValue() })); await settle();
    assert.equal(h.byId("configuration").hidden, false); assert.equal(h.byId("profile").value, profile);
    assert.equal(h.calls.filter(c => c.path === "/api/catalog_route").length, 1);
    assert.ok(h.calls.every(c => ["/api/state", "/api/catalog", "/api/catalog_entry", "/api/catalog_route"].includes(c.path)));
  } finally { h.close(); }
});
test("explicit catalog route prepares once then verifies exact profile without choosing operations or fields", async () => {
  const h = routeHarness();
  try {
    await openRoute(h);
    assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 0);
    assert.match(h.byId("catalog-next").textContent, /Use Set up this option.*prepare its library if needed and open its exact profile/);
    assert.doesNotMatch(h.byId("catalog-next").textContent, /After preparation, choose its exact profile/);
    assert.match(h.byId("catalog-next").textContent, /Setup choices are above; their technical instructions are below/);
    assert.match(h.byId("catalog-strategies").textContent, /Manual setup/);
    h.byId("catalog-strategies").querySelector("button").click(); await settle();
    assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 1);
    h.fixture.state = { ...h.fixture.state, ...stateValue("prepared"), job: job("succeeded", "prepare") };
    h.byId("refresh").click(); await settle();
    assert.equal(h.byId("family").value, "agno"); assert.equal(h.byId("profile").value, profile);
    assert.equal(h.byId("configuration").hidden, false);
    assert.equal(h.byId("operations").querySelectorAll("input:checked").length, 0);
    assert.equal(h.field("host").value, ""); assert.equal(h.field("token_env").value, "");
    assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 1);
    assert.equal(h.calls.filter(c => c.path === "/api/check" || c.path === "/api/install").length, 0);
  } finally { h.close(); }
});
test("prepared route clears old configuration before the readonly lookup resolves", async () => {
  const gate = deferred(); const h = routeHarness("prepared", call => call.path === "/api/catalog_route" ? gate.promise : undefined);
  try {
    await openRoute(h); await h.fill(); await h.submit();
    assert.equal(h.byId("review").hidden, false);
    h.byId("catalog-strategies").querySelector("button").click(); await settle();
    assert.equal(h.byId("review").hidden, true); assert.equal(h.byId("configuration").hidden, true);
    assert.equal(h.byId("fields").children.length, 0);
    gate.resolve(response({}, 409)); await settle();
    assert.equal(h.byId("configuration").hidden, true);
  } finally { h.close(); }
});
test("late route replies cannot replace manual changes, other catalog choice, recovery or disposal", async () => {
  for (const action of ["family", "profile", "catalog", "recovery", "dispose"]) {
    const gate = deferred(); const h = routeHarness("prepared", call => call.path === "/api/catalog_route" ? gate.promise : undefined);
    try {
      await openRoute(h); h.byId("catalog-strategies").querySelector("button").click(); await settle();
      if (action === "family") h.change(h.byId("family"), "");
      if (action === "profile") h.change(h.byId("profile"), "");
      if (action === "catalog") h.change(h.byId("catalog-choice"), catalogKey(catalogEntries()[1]));
      if (action === "recovery") { h.fixture.state.mode = "recovery"; h.byId("refresh").click(); }
      if (action === "dispose") h.app.dispose();
      await settle();
      const { status, artifact, ...route } = routeOption();
      gate.resolve(response({ ...catalogBinding(), scope: "catalog_setup_handoff", routes_sha256: "f".repeat(64), status: "prepared", route, artifact, help: helpValue() })); await settle();
      assert.equal(h.byId("configuration").hidden, true, action);
      assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 0, action);
    } finally { h.close(); }
  }
});
test("route missing installed profile and uncertain preparation refuse without fallback or retry", async () => {
  for (const mode of ["missing", "uncertain"]) {
    const h = routeHarness(mode === "missing" ? "prepared" : "unprepared");
    try {
      await openRoute(h);
      if (mode === "missing") { h.fixture.state.families[0].profiles = []; h.byId("refresh").click(); await settle(); }
      h.byId("catalog-strategies").querySelector("button").click(); await settle();
      if (mode === "uncertain") {
        h.fixture.state.families[0].status = "unconfirmed"; h.fixture.state.job = job("unconfirmed", "prepare");
        h.byId("refresh").click(); await settle(); h.byId("refresh").click(); await settle();
      }
      assert.equal(h.byId("configuration").hidden, true);
      assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, mode === "missing" ? 0 : 1);
    } finally { h.close(); }
  }
});

const priorSha = "9".repeat(64);
const priorState = (status = "prepared") => ({ ...stateValue(status), mode: "setup",
  catalog: { available: false, contents_sha256: "d".repeat(64), catalog_sha256: null },
  family_contents_sha256: { agno: "d".repeat(64) },
  prior_selection: { request_sha256: priorSha, family: "agno", profile } });
const priorResponse = (extra = {}) => ({ scope: "successor_configuration_candidate", prior_request_sha256: priorSha,
  current_contents_sha256: "d".repeat(64), prior_configuration_sha256: "b".repeat(64), reported_prior_contents_sha256: "c".repeat(64),
  family: "agno", profile, status: "review_required", help: helpValue(), prior_operations: ["search_web"],
  values: { host: "https://prior.example", timeout: 10, token_env: "PRIOR_TOKEN" }, unapplied: [], ...extra });
const priorHarness = (initial = priorState(), handler = () => undefined) => harness(initial, async (call, fixture) => {
  const value = await handler(call, fixture);
  if (value !== undefined) return value;
  if (call.path === "/api/successor") return response(priorResponse(fixture.state.families[0].status === "prepared" ? {} : { status: "preparation_required" }));
});

const officialPrior = (family) => {
  const playwright = family === "official-playwright";
  const currentProfile = playwright ? "isolated-chrome" : "selected-reads";
  const ops = (playwright ? ["browser_navigate", "browser_snapshot", "browser_fill_form", "browser_click", "browser_close"] :
    ["get_me", "list_commits", "get_file_contents"]).map(id => ({ id, effect: playwright ? "unspecified" : "read" }));
  const values = playwright ? {} : { token_env: "GITHUB_PERSONAL_ACCESS_TOKEN" };
  return {
    state: { ...priorState(), families: [{ id: family, status: "prepared", profiles: [{ id: currentProfile, operations: ops }] }],
      family_contents_sha256: { [family]: "e".repeat(64) }, prior_selection: { request_sha256: priorSha, family, profile: currentProfile } },
    proposal: priorResponse({ family, profile: currentProfile, current_contents_sha256: "e".repeat(64), values,
      prior_operations: [ops[0].id], help: { family, profile: currentProfile, operations: ops, selectable_operations: false,
        fields: playwright ? [] : [{ name: "token_env", kind: "string", required: true, credential_reference: true }],
        prerequisites: [], unresolved_details: [], scope: "configuration_only" } }),
  };
};

test("bound official catalog options open their fixed profile without a second family choice", async () => {
  for (const family of ["official-github", "official-playwright"]) {
    const data = officialPrior(family);
    const row = { identity: { publisher: "meerkat-toolkit", name: family, version: "exact-version" },
      display_name: family, description: "Official catalog option" };
    const artifact = { path: family + "/artifact", sha256: "a".repeat(64) };
    const route = { identity: row.identity, strategy_id: "selected-local-mcp", family, profile: data.proposal.profile };
    const initial = { ...data.state, prior_selection: undefined, catalog: { available: true, ...catalogBinding() } };
    const h = harness(initial, call => {
      if (call.path === "/api/catalog") return response({ ...catalogBinding(), entries: [row] });
      if (call.path === "/api/catalog_entry") return response({ ...catalogBinding(), routes_sha256: "f".repeat(64),
        entry: { ...row, strategies: [{ id: route.strategy_id, instructions: "Explicit selected option", kind: "local_mcp" }] },
        setup_options: [{ ...route, artifact, status: "prepared" }] });
      if (call.path === "/api/catalog_route") return response({ ...catalogBinding(), scope: "catalog_setup_handoff",
        routes_sha256: "f".repeat(64), route, artifact, status: "prepared", help: data.proposal.help });
    });
    try {
      await h.app.start(); h.change(h.byId("catalog-choice"), catalogKey(row)); await settle();
      assert.equal(h.byId("catalog-strategies").querySelectorAll("button").length, 1);
      assert.equal(h.byId("catalog-technical").open, false);
      assert.match(h.byId("catalog-strategies").textContent, family === "official-playwright" ? /whole Playwright endpoint/ : /fixed read-only route/);
      assert.match(h.byId("catalog-strategies").textContent, /operations cannot be (limited|changed) here/);
      h.byId("catalog-strategies").querySelector("button").click(); await settle();
      assert.equal(h.byId("family").value, family);
      assert.equal(h.byId("profile").value, route.profile);
      assert.equal(h.byId("configuration").hidden, false);
      const boxes = [...h.byId("operations").querySelectorAll("input")];
      assert.equal(boxes.length, data.proposal.help.operations.length);
      assert.ok(boxes.every(box => box.checked && box.disabled));
      assert.ok(!h.calls.some(call => ["/api/prepare", "/api/check", "/api/install"].includes(call.path)));
    } finally { h.close(); }
  }
});

test("catalog family validation refuses unknown names and non-string aliases", async () => {
  for (const family of ["official-unknown", "toString", ["official-github"], null]) {
    const h = routeHarness("prepared", call => call.path === "/api/catalog_entry" ? response({
      ...catalogBinding(), routes_sha256: "f".repeat(64), entry: catalogEntry(catalogEntries()[0]),
      setup_options: [{ ...routeOption("prepared"), family }, { strategy_id: "alternative", status: "manual", reason: "no_bound_setup_route" }],
    }) : undefined);
    try {
      await openRoute(h);
      assert.match(h.byId("catalog-status").textContent, /could not be confirmed/);
      assert.ok(!h.calls.some(call => call.path === "/api/catalog_route" || call.path === "/api/prepare"));
    } finally { h.close(); }
  }
});

test("official prior review binds its current family artifact and requires explicit check and install", async () => {
  for (const family of ["official-github", "official-playwright"]) {
    const data = officialPrior(family);
    // Both an absent library bundle and a different library hash must be irrelevant.
    if (family === "official-github") data.state.catalog.contents_sha256 = null;
    const h = priorHarness(data.state, (call, fixture) => {
      if (call.path === "/api/successor") return response(data.proposal);
      if (call.path === "/api/install") {
        fixture.state.job = { id: "1".repeat(32), family, kind: "install", status: "running" };
        return response({ job: fixture.state.job });
      }
    });
    try {
      await h.app.start(); assert.equal(h.byId("prior-panel").hidden, false);
      h.byId("reuse-prior").click(); await settle();
      assert.equal(h.byId("configuration").hidden, false);
      const boxes = [...h.byId("operations").querySelectorAll("input")];
      assert.ok(boxes.every(input => input.checked && input.disabled));
      assert.equal(boxes.length, data.proposal.help.operations.length);
      assert.match(h.byId("prior-operations").textContent, /current profile requires the complete list/);
      assert.doesNotMatch(h.byId("prior-operations").textContent, /Choose the current operations again/);
      if (family === "official-playwright") assert.match(h.byId("prior-operations").textContent, /whole endpoint/);
      assert.ok(!h.calls.some(c => c.path === "/api/check" || c.path === "/api/install"));
      await h.submit();
      const draft = { family, profile: data.proposal.profile, operations: data.proposal.help.operations.map(o => o.id), values: data.proposal.values };
      assert.deepEqual(h.calls.find(c => c.path === "/api/check").payload, draft);
      assert.ok(!h.calls.some(c => c.path === "/api/install"));
      h.byId("install").click(); await settle();
      assert.deepEqual(h.calls.find(c => c.path === "/api/install").payload,
        { ...draft, validation_id: "fixture-validation", configuration_sha256: "a".repeat(64) });
    } finally { h.close(); }
  }
});

test("official successor rejects missing family binding and never substitutes the library hash", async () => {
  for (const binding of [undefined, null, {}, { "official-github": null }, { "official-github": "invalid" }, { agno: "e".repeat(64) }]) {
    const data = officialPrior("official-github"); data.state.family_contents_sha256 = binding;
    const h = priorHarness(data.state, call => call.path === "/api/successor" ? response(data.proposal) : undefined);
    try {
      await h.app.start(); assert.equal(h.byId("prior-panel").hidden, true);
      h.byId("reuse-prior").click(); await settle();
      assert.ok(!h.calls.some(c => c.path === "/api/successor" || c.path === "/api/check" || c.path === "/api/install"));
    } finally { h.close(); }
  }
  for (const digest of [undefined, null, "invalid", "d".repeat(64)]) {
    const data = officialPrior("official-github"); data.proposal.current_contents_sha256 = digest;
    const h = priorHarness(data.state, call => call.path === "/api/successor" ? response(data.proposal) : undefined);
    try {
      await h.app.start(); h.byId("reuse-prior").click(); await settle();
      assert.equal(h.byId("configuration").hidden, true);
      assert.ok(!h.calls.some(c => c.path === "/api/check" || c.path === "/api/install"));
    } finally { h.close(); }
  }
});

test("fixed official coverage still requires an explicit decision for removed historical items", async () => {
  const data = officialPrior("official-playwright");
  data.proposal.prior_operations.push("removed_operation");
  data.proposal.unapplied = [{ kind: "operation", name: "removed_operation", reason: "not_in_current_profile" },
    { kind: "field", name: "old_directory", prior_type: "string", reason: "not_in_current_profile" }];
  const h = priorHarness(data.state, call => call.path === "/api/successor" ? response(data.proposal) : undefined);
  try {
    await h.app.start(); h.byId("reuse-prior").click(); await settle();
    await h.submit(); assert.equal(h.byId("validate").disabled, true);
    assert.ok(!h.calls.some(c => c.path === "/api/check"));
    for (const issue of ["operation:removed_operation", "field:old_directory"]) h.document.querySelector(`[data-prior-issue="${issue}"]`).click();
    assert.equal(h.byId("validate").disabled, false); await h.submit();
    const draft = h.calls.find(c => c.path === "/api/check").payload;
    assert.deepEqual(draft.operations, data.proposal.help.operations.map(o => o.id));
    assert.deepEqual(draft.values, {});
  } finally { h.close(); }
});

test("a delayed official proposal cannot survive a changed current family binding", async () => {
  const data = officialPrior("official-playwright"), gate = deferred();
  const h = priorHarness(data.state, call => call.path === "/api/successor" ? gate.promise : undefined);
  try {
    await h.app.start(); h.byId("reuse-prior").click(); await settle();
    h.fixture.state.family_contents_sha256["official-playwright"] = "f".repeat(64);
    h.byId("refresh").click(); await settle();
    gate.resolve(response(data.proposal)); await settle();
    assert.equal(h.byId("configuration").hidden, true);
    assert.ok(!h.calls.some(c => c.path === "/api/check" || c.path === "/api/install"));
  } finally { h.close(); }
});

test("prior configuration is explicit, keeps current operations unchecked and validates a new selection", async () => {
  const h = priorHarness();
  try {
    await h.app.start();
    assert.deepEqual(h.calls.map(c => c.path), ["/api/state"]);
    assert.equal(h.byId("prior-panel").hidden, false);
    assert.equal(h.byId("configuration").hidden, true);
    h.byId("reuse-prior").click(); await settle();
    assert.deepEqual(h.calls.find(c => c.path === "/api/successor").payload, { prior_request_sha256: priorSha });
    assert.equal(h.field("host").value, "https://prior.example"); assert.equal(h.field("timeout").value, "10");
    assert.equal(h.field("token_env").value, "PRIOR_TOKEN");
    assert.equal(h.byId("operations").querySelectorAll("input:checked").length, 0);
    assert.match(h.byId("prior-operations").textContent, /search_web/);
    h.checked(0); await h.submit();
    const checked = h.calls.find(c => c.path === "/api/check").payload;
    assert.deepEqual(checked, { family: "agno", profile, operations: ["search_web"], values: { host: "https://prior.example", timeout: 10, token_env: "PRIOR_TOKEN" } });
    assert.equal(h.byId("review").hidden, false);
    assert.equal(h.calls.filter(c => c.path === "/api/install").length, 0);
  } finally { h.close(); }
});

test("unsupported prior items require explicit omission or replacement and never silently enable validation", async () => {
  const h = priorHarness(priorState(), call => call.path === "/api/successor" ? response(priorResponse({
    values: { host: "https://prior.example" }, prior_operations: ["search_web", "removed_operation"],
    unapplied: [{ kind: "field", name: "removed_field", prior_type: "string", reason: "not_in_current_profile" },
      { kind: "field", name: "timeout", prior_type: "string", reason: "current_type_differs" },
      { kind: "operation", name: "removed_operation", reason: "not_in_current_profile" }] })) : undefined);
  try {
    await h.app.start(); h.byId("reuse-prior").click(); await settle(); h.checked(0);
    assert.equal(h.byId("validate").disabled, true); await h.submit();
    assert.equal(h.calls.filter(c => c.path === "/api/check").length, 0);
    h.change(h.field("timeout"), "30", "input");
    assert.equal(h.byId("validate").disabled, true);
    for (const key of ["field:removed_field", "operation:removed_operation"]) {
      const box = h.document.querySelector(`[data-prior-issue="${key}"]`); box.click();
    }
    assert.equal(h.byId("validate").disabled, false); await h.submit();
    const value = h.calls.find(c => c.path === "/api/check").payload;
    assert.deepEqual(value.values, { host: "https://prior.example", timeout: 30 });
    assert.deepEqual(value.operations, ["search_web"]);
    assert.match(h.byId("prior-unapplied").textContent, /removed_field/);
  } finally { h.close(); }
});

test("conditional prior values remain explicit and an omitted value does not revive with a later operation", async () => {
  const h = priorHarness(priorState(), call => call.path === "/api/successor" ? response(priorResponse({
    prior_operations: ["write_fixture"], values: { host: "https://prior.example", enable_writes: false } })) : undefined);
  try {
    await h.app.start(); h.byId("reuse-prior").click(); await settle(); h.checked(0);
    assert.equal(h.byId("validate").disabled, true);
    h.document.querySelector('[data-prior-issue="field:enable_writes"]').click();
    assert.equal(h.byId("validate").disabled, false);
    h.checked(1);
    assert.equal(h.field("enable_writes").value, "");
    await h.submit(); assert.equal(h.calls.filter(c => c.path === "/api/check").length, 0);
    h.change(h.field("enable_writes"), "false", "input"); await h.submit();
    assert.equal(h.calls.find(c => c.path === "/api/check").payload.values.enable_writes, false);
  } finally { h.close(); }
});

test("prior setup prepares once without a catalog and uncertainty never retries", async () => {
  for (const outcome of ["prepared", "unconfirmed"]) {
    const h = priorHarness(priorState("unprepared"));
    try {
      await h.app.start(); h.byId("reuse-prior").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 1);
      h.fixture.state = { ...priorState(outcome), job: job(outcome === "prepared" ? "succeeded" : "unconfirmed", "prepare") };
      h.byId("refresh").click(); await settle(); h.byId("refresh").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/prepare").length, 1);
      assert.equal(h.byId("configuration").hidden, outcome !== "prepared");
    } finally { h.close(); }
  }
});

test("late prior proposals cannot overwrite A-to-B-to-A changes, recovery, changed source or disposal", async () => {
  for (const action of ["aba", "recovery", "source", "dispose"]) {
    const gate = deferred(); const h = priorHarness(priorState(), call => call.path === "/api/successor" ? gate.promise : undefined);
    try {
      await h.app.start(); h.byId("reuse-prior").click(); await settle();
      if (action === "aba") { h.change(h.byId("family"), ""); h.change(h.byId("family"), "agno"); }
      if (action === "recovery") { h.fixture.state.mode = "recovery"; h.byId("refresh").click(); }
      if (action === "source") { h.fixture.state.prior_selection.request_sha256 = "8".repeat(64); h.byId("refresh").click(); }
      if (action === "dispose") h.app.dispose();
      await settle(); gate.resolve(response(priorResponse())); await settle();
      assert.equal(h.byId("configuration").hidden, true, action);
      assert.equal(h.calls.filter(c => c.path === "/api/check" || c.path === "/api/install").length, 0);
    } finally { h.close(); }
  }
});


test("prior issue keyboard focus survives review rebuild without taking focus from another control", async () => {
  const h = priorHarness(priorState(), call => call.path === "/api/successor" ? response(priorResponse({
    unapplied: [{ kind: "field", name: "removed_field", prior_type: "string", reason: "not_in_current_profile" },
      { kind: "operation", name: "removed_operation", reason: "not_in_current_profile" }] })) : undefined);
  try {
    await h.app.start(); h.byId("reuse-prior").click(); await settle();
    const issue = () => h.document.querySelector('[data-prior-issue="field:removed_field"]');
    issue().focus();
    for (const checked of [true, false]) {
      const before = issue();
      assert.equal(h.document.activeElement, before);
      // Model the native Space activation's change event; jsdom has no keyboard default action.
      before.dispatchEvent(new h.dom.window.KeyboardEvent("keydown", { key: " ", code: "Space", bubbles: true }));
      before.checked = checked;
      before.dispatchEvent(new h.dom.window.Event("change", { bubbles: true }));
      const after = issue();
      assert.notEqual(after, before);
      assert.equal(after.checked, checked);
      assert.equal(h.document.activeElement, after);
      after.dispatchEvent(new h.dom.window.KeyboardEvent("keyup", { key: " ", code: "Space", bubbles: true }));
    }
    h.field("host").focus();
    h.change(h.field("host"), "https://edited.example", "input");
    assert.equal(h.document.activeElement, h.field("host"));
    assert.equal(issue().checked, false);
    assert.equal(h.calls.filter(call => ["/api/check", "/api/install"].includes(call.path)).length, 0);
  } finally { h.close(); }
});


test("idle polling observes work started by another surface and preserves the local draft", async () => {
  const h = harness();
  const tick = async () => {
    assert.equal(h.timers.size, 1);
    const [id, callback] = [...h.timers][0]; h.timers.delete(id); callback(); await settle();
  };
  try {
    await h.app.start(); await h.fill();
    const input = h.field("host"); input.focus();
    h.fixture.state.job = job("running");
    await tick();
    assert.match(h.byId("job-status").textContent, /running/);
    assert.equal(h.field("host"), input);
    assert.equal(input.value, "https://127.0.0.1:49352");
    assert.equal(h.byId("validate").disabled, true);
    h.fixture.state.job = completed();
    await tick();
    assert.equal(h.byId("result").hidden, false);
    assert.deepEqual(JSON.parse(h.byId("mcp-configuration").textContent), completed().result.mcp_configuration);
    assert.equal(input.value, "https://127.0.0.1:49352");
    assert.notEqual(h.document.activeElement.id, "result-heading");
    assert.ok(h.calls.every(call => ["/api/state", "/api/help"].includes(call.path)));
    h.app.dispose(); assert.equal(h.timers.size, 0);
  } finally { h.close(); }
});

test("background refresh waits for validation and does not erase a validation error", async () => {
  const gate = deferred();
  const h = harness(stateValue(), call => call.path === "/api/check" ? gate.promise : undefined);
  try {
    await h.app.start(); await h.fill();
    const validating = h.submit(); await settle();
    const before = h.calls.filter(call => call.path === "/api/state").length;
    let [id, callback] = [...h.timers][0]; h.timers.delete(id); callback(); await settle();
    assert.equal(h.calls.filter(call => call.path === "/api/state").length, before);
    gate.resolve(response({ error: "invalid_configuration" }, 400)); await validating; await settle();
    const problem = h.byId("problem").textContent; assert.ok(problem);
    [id, callback] = [...h.timers][0]; h.timers.delete(id); callback(); await settle();
    assert.equal(h.byId("problem").textContent, problem);
    assert.equal(h.byId("review").hidden, true);
  } finally { h.close(); }
});

test("failed idle observation stops automatic polling until explicit refresh", async () => {
  let failed = false;
  const h = harness(stateValue(), call => failed && call.path === "/api/state" ? response({}, 403) : undefined);
  try {
    await h.app.start(); failed = true;
    const [id, callback] = [...h.timers][0]; h.timers.delete(id); callback(); await settle();
    assert.equal(h.timers.size, 0);
    assert.match(h.byId("connection-status").textContent, /unconfirmed/);
    failed = false; h.byId("refresh").click(); await settle();
    assert.equal(h.timers.size, 1);
    assert.match(h.byId("connection-status").textContent, /Connected/);
  } finally { h.close(); }
});

test("delayed idle observation cannot replace a newer validation result", async () => {
  const gate = deferred(); let deferState = false;
  const h = harness(stateValue(), call => deferState && call.path === "/api/state" ? gate.promise : undefined);
  try {
    await h.app.start(); await h.fill();
    deferState = true;
    const [id, callback] = [...h.timers][0]; h.timers.delete(id); callback(); await settle();
    await h.submit(); await settle();
    assert.equal(h.byId("review").hidden, false);
    const reviewed = h.byId("review-values").textContent;
    gate.resolve(response(stateValue())); await settle();
    assert.equal(h.byId("review").hidden, false);
    assert.equal(h.byId("review-values").textContent, reviewed);
    assert.equal(h.byId("install").disabled, false);
    assert.equal(h.calls.filter(call => call.path === "/api/check").length, 1);
  } finally { h.close(); }
});

const failedPreparation = (id = "a".repeat(32)) => ({ id, family: "agno", kind: "prepare", status: "unconfirmed", error: "operation_stopped" });
const retryState = () => ({ ...stateValue(), preparation_retry: { available: true },
  families: [{ id: "agno", status: "unconfirmed", profiles: [], preparation_job_id: "a".repeat(32) }],
  job: failedPreparation() });

test("preparation retry is explicit, follows the exact predecessor and keeps failures visible", async () => {
  const h = harness(retryState(), (call, fixture) => {
    if (call.path !== "/api/retry_prepare") return;
    fixture.state.job = { id: "b".repeat(32), family: "agno", kind: "prepare", status: "running", retry_of: "a".repeat(32) };
    fixture.state.families[0].status = "preparing";
    fixture.state.families[0].preparation_job_id = "b".repeat(32);
    return response({ job: fixture.state.job });
  });
  try {
    await h.app.start();
    assert.equal(h.byId("retry-prepare").disabled, false);
    assert.match(h.byId("retry-prepare-status").textContent, /fresh files.*retained/);
    assert.equal(h.calls.filter(c => c.path === "/api/retry_prepare").length, 0);
    h.byId("retry-prepare").click(); h.byId("retry-prepare").click(); await settle();
    assert.deepEqual(h.calls.filter(c => c.path === "/api/retry_prepare").map(c => c.payload), [{ job_id: "a".repeat(32) }]);
    assert.match(h.byId("job-context").textContent, /New preparation after attempt a{32}/);
    assert.match(h.byId("job-status").textContent, /running/);
    assert.equal(h.byId("retry-prepare").hidden, true);
  } finally { h.close(); }
});

test("lost or mismatched preparation reply requires readback and another explicit click", async () => {
  for (const mode of ["lost", "wrong_predecessor", "wrong_family"]) {
    const h = harness(retryState(), call => {
      if (call.path !== "/api/retry_prepare") return;
      if (mode === "lost") throw new Error("private failure details");
      return response({ job: { id: "b".repeat(32), kind: "prepare", status: "running",
        family: mode === "wrong_family" ? "services" : "agno",
        retry_of: mode === "wrong_predecessor" ? "c".repeat(32) : "a".repeat(32) } });
    });
    try {
      await h.app.start(); h.byId("retry-prepare").click(); await settle();
      assert.match(h.byId("retry-prepare-status").textContent, /Could not confirm/);
      assert.ok(!h.document.body.textContent.includes("private failure details"));
      assert.equal(h.byId("retry-prepare").disabled, true);
      h.byId("retry-prepare").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/retry_prepare").length, 1);
      h.byId("refresh").click(); await settle();
      assert.equal(h.byId("retry-prepare").disabled, false);
      assert.equal(h.calls.filter(c => c.path === "/api/retry_prepare").length, 1);
      h.byId("retry-prepare").click(); await settle();
      assert.deepEqual(h.calls.filter(c => c.path === "/api/retry_prepare").map(c => c.payload),
        [{ job_id: "a".repeat(32) }, { job_id: "a".repeat(32) }]);
    } finally { h.close(); }
  }
});

test("preparation retry refuses stale attempts, installations, absent capability and recovery", async () => {
  for (const alter of [
    s => { s.families[0].preparation_job_id = "b".repeat(32); },
    s => { s.job.kind = "install"; },
    s => { s.job.status = "running"; },
    s => { s.job.status = "succeeded"; },
    s => { delete s.preparation_retry; },
    s => { s.mode = "recovery"; },
    s => { s.operation_in_progress = true; },
  ]) {
    const initial = retryState(); alter(initial);
    const h = harness(initial);
    try {
      await h.app.start();
      assert.equal(h.byId("retry-prepare").disabled, true);
      h.byId("retry-prepare").click(); await settle();
      assert.equal(h.calls.filter(c => c.path === "/api/retry_prepare").length, 0);
    } finally { h.close(); }
  }
});

test("late retry reply does not steal a newly selected history view", async () => {
  const gate = deferred(), initial = retryState();
  const older = { ...failedPreparation("c".repeat(32)), family: "services" };
  initial.job_history = { available: true, count: 2 };
  initial.families.push({ id: "services", status: "unconfirmed", profiles: [], preparation_job_id: older.id });
  const h = harness(initial, async (call, fixture) => {
    if (call.path === "/api/jobs") return response({ jobs: [jobSummary(fixture.state.job), jobSummary(older)], next_before: null });
    if (call.path === "/api/job") return response({ job: older, reconciliation: { available: false, observation: null } });
    if (call.path === "/api/retry_prepare") return gate.promise;
  });
  try {
    await h.app.start(); h.byId("retry-prepare").click(); await settle();
    h.change(h.byId("job-choice"), older.id); await settle();
    gate.resolve(response({ job: { id: "b".repeat(32), kind: "prepare", status: "running", family: "agno", retry_of: "a".repeat(32) } }));
    await settle();
    assert.equal(h.byId("job-choice").value, older.id);
    assert.equal(h.calls.filter(c => c.path === "/api/retry_prepare").length, 1);
    assert.equal(h.byId("retry-prepare").disabled, false);
  } finally { h.close(); }
});

test("retry response follows its returned job when another surface makes unrelated work current", async () => {
  const initial = retryState(); initial.job_history = { available: true, count: 1 };
  initial.families.push({ id: "services", status: "unprepared", profiles: [] });
  const successor = { id: "b".repeat(32), family: "agno", kind: "prepare", status: "succeeded", retry_of: "a".repeat(32) };
  const other = { id: "c".repeat(32), family: "services", kind: "prepare", status: "running" };
  let retried = false;
  const h = harness(initial, (call, fixture) => {
    if (call.path === "/api/jobs") return response({ jobs: (retried ? [other, successor, failedPreparation()] : [failedPreparation()]).map(jobSummary), next_before: null });
    if (call.path === "/api/job") return response({ job: successor, reconciliation: { available: false, observation: null } });
    if (call.path === "/api/retry_prepare") {
      retried = true;
      fixture.state.job = other;
      fixture.state.job_history.count = 3;
      fixture.state.families[0] = { id: "agno", status: "prepared", profiles: [], preparation_job_id: successor.id };
      return response({ job: successor });
    }
  });
  try {
    await h.app.start(); h.byId("retry-prepare").click(); await settle();
    assert.equal(h.byId("job-choice").value, successor.id);
    assert.match(h.byId("job-status").textContent, /completed/);
    assert.match(h.byId("current-job-status").textContent, /services.*running/);
    assert.ok(h.calls.some(c => c.path === "/api/job" && c.payload.job_id === successor.id));
  } finally { h.close(); }
});

const listField = () => ({ name: "allowed_channel_ids", kind: "string_list", required: false, credential_reference: false });
const listHelp = () => { const value = helpValue(); value.fields.push(listField()); return value; };

test("generic string list form distinguishes omitted, empty and explicit arrays", () => {
  const help = normalizeHelp(listHelp(), "agno", profile);
  for (const [text, expected] of [["", undefined], ["[]", []], ['["C1","C2"]', ["C1", "C2"]], ['["","same","same"]', ["", "same", "same"]]]) {
    const draft = draftFromFields(help, ["search_web"], { host: "example", allowed_channel_ids: text });
    assert.equal(Object.hasOwn(draft.values, "allowed_channel_ids"), expected !== undefined);
    if (expected !== undefined) assert.deepEqual(draft.values.allowed_channel_ids, expected);
  }
  for (const value of ["null", '"C1"', "[1]", "[true]", "[[]]", "{}", "C1,C2", '["a\\nb"]', '["\\u007f"]',
    JSON.stringify(["x".repeat(2049)]), JSON.stringify(Array(257).fill("x"))]) {
    assert.throws(() => draftFromFields(help, ["search_web"], { host: "example", allowed_channel_ids: value }),
      error => error instanceof FormError && error.code === "string_list_required" && error.field === "allowed_channel_ids");
  }
  for (const value of [Array(256).fill("x"), ["\u{1f600}".repeat(2048)]]) {
    assert.deepEqual(draftFromFields(help, ["search_web"], { host: "example", allowed_channel_ids: JSON.stringify(value) }).values.allowed_channel_ids, value);
  }
  help.fields.at(-1).required = true;
  assert.throws(() => draftFromFields(help, ["search_web"], { host: "example" }), error => error.code === "field_required");
  assert.deepEqual(draftFromFields(help, ["search_web"], { host: "example", allowed_channel_ids: "[]" }).values.allowed_channel_ids, []);
});

test("string list textarea sends exact values and review displays [] explicitly", async () => {
  for (const values of [undefined, [], ["C1", "C2"]]) {
    const h = harness(); h.fixture.help = listHelp();
    try {
      await h.app.start(); await h.fill();
      assert.equal(h.field("allowed_channel_ids").tagName, "TEXTAREA");
      assert.match(h.field("allowed_channel_ids").parentNode.textContent, /Optional.*JSON array.*\[\].*empty list/);
      if (values !== undefined) h.change(h.field("allowed_channel_ids"), JSON.stringify(values), "input");
      await h.submit();
      const draft = h.calls.find(c => c.path === "/api/check").payload;
      assert.equal(Object.hasOwn(draft.values, "allowed_channel_ids"), values !== undefined);
      const review = h.byId("review").textContent;
      if (values !== undefined) {
        assert.deepEqual(draft.values.allowed_channel_ids, values);
        assert.ok(review.includes(JSON.stringify(values)));
      } else assert.ok(!review.includes("allowed_channel_ids"));
    } finally { h.close(); }
  }
});

test("invalid list input prevents setup check", async () => {
  const h = harness(); h.fixture.help = listHelp();
  try {
    await h.app.start(); await h.fill(); h.change(h.field("allowed_channel_ids"), "null", "input"); await h.submit();
    assert.equal(h.calls.filter(c => c.path === "/api/check").length, 0);
    assert.match(h.field("allowed_channel_ids").validationMessage, /JSON array/);
  } finally { h.close(); }
});

test("successor list restore keeps [] explicit and requires review before check", async () => {
  for (const values of [undefined, [], ["C1", "C2"]]) {
    const h = priorHarness(priorState(), call => call.path === "/api/successor" ? response(priorResponse({
      help: listHelp(), values: { host: "https://prior.example", ...(values === undefined ? {} : { allowed_channel_ids: values }) },
    })) : undefined);
    try {
      await h.app.start(); h.byId("reuse-prior").click(); await settle();
      assert.equal(h.field("allowed_channel_ids").value, values === undefined ? "" : JSON.stringify(values));
      assert.equal(h.calls.filter(c => c.path === "/api/check").length, 0);
      h.checked(0); await h.submit();
      const checked = h.calls.find(c => c.path === "/api/check").payload;
      assert.equal(Object.hasOwn(checked.values, "allowed_channel_ids"), values !== undefined);
      if (values !== undefined) assert.deepEqual(checked.values.allowed_channel_ids, values);
    } finally { h.close(); }
  }
});

test("successor rejects malformed lists and explicitly resolves removed list fields", async () => {
  for (const value of [null, "[]", [true], ["\n"], Array(257).fill("x")]) {
    const h = priorHarness(priorState(), call => call.path === "/api/successor" ? response(priorResponse({
      help: listHelp(), values: { allowed_channel_ids: value },
    })) : undefined);
    try {
      await h.app.start(); h.byId("reuse-prior").click(); await settle();
      assert.equal(h.byId("configuration").hidden, true);
      assert.equal(h.calls.filter(c => c.path === "/api/check").length, 0);
    } finally { h.close(); }
  }
  const h = priorHarness(priorState(), call => call.path === "/api/successor" ? response(priorResponse({
    unapplied: [{ kind: "field", name: "old_list", prior_type: "string_list", reason: "not_in_current_profile" }],
  })) : undefined);
  try {
    await h.app.start(); h.byId("reuse-prior").click(); await settle(); h.checked(0);
    assert.equal(h.byId("validate").disabled, true);
    h.document.querySelector('[data-prior-issue="field:old_list"]').click(); await h.submit();
    assert.equal(h.calls.filter(c => c.path === "/api/check").length, 1);
  } finally { h.close(); }
});
