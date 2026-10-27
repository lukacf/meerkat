const assert = require("node:assert/strict");
const { test } = require("node:test");
const { selectScenarios } = require("./scenario-registry.cjs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const scenarios = [
  { id: "runtime", family: "runtime", backend: "real" },
  { id: "scroll", family: "presentation", backend: "mock" },
  { id: "markdown", family: "presentation", backend: "mock" },
  { id: "recovery", family: "runtime", backend: "real" },
];
test("complete deterministic shards cover every scenario exactly once including real backends", () => {
  const a = selectScenarios(scenarios, ["--shard=1/2"]).selected;
  const b = selectScenarios(scenarios, ["--shard=2/2"]).selected;
  assert.equal(new Set([...a, ...b].map((s) => s.id)).size, scenarios.length);
  assert.deepEqual(a.map((s) => s.id), ["runtime", "markdown"]);
  assert.deepEqual(b.map((s) => s.id), ["scroll", "recovery"]);
  assert.deepEqual(selectScenarios(scenarios, []).selected, scenarios);
});
test("typos, empty selections, duplicate IDs and invalid shards cannot produce green empty runs", () => {
  for (const args of [["--scenario=missing"], ["--family=absent"], ["--shard=3/2"], ["--shard=5/6"], ["--scenario=scroll", "--family=runtime"], ["--typo"]]) {
    assert.throws(() => selectScenarios(scenarios, args));
  }
  assert.throws(() => selectScenarios([...scenarios, scenarios[0]], []));
});

test("shipping runners discover every registered scenario and shard default acceptance exactly once", () => {
  const browser = require("./browser-e2e.cjs").scenarios;
  for (const scenario of [...require("./scenarios/settled-history-queue.cjs").scenarios,
    ...require("./scenarios/operator-panel-frame.cjs").scenarios]) {
    assert.equal(browser.filter(item => item.id === scenario.id && item.backend === "mock").length, 1, scenario.id);
    assert(selectScenarios(browser, []).selected.some(item => item.id === scenario.id), `${scenario.id} runs by default`);
  }
  const api = JSON.parse(execFileSync(process.execPath, [path.join(__dirname, "api-e2e.cjs"), "--list"], { encoding: "utf8" }));
  const browserModules = [
    require("./scenarios/real-conversation.cjs").scenarios,
    require("./scenarios/real-recovery-scope.cjs").scenarios,
    require("./scenarios/real-markdown-url-policy.cjs").scenarios,
    require("./scenarios/real-reasoning.cjs").scenarios,
    require("./scenarios/real-assistant-identity.cjs").scenarios,
    require("./scenarios/real-startup-lineage.cjs").scenarios,
    require("./scenarios/real-routine-tools.cjs").browserScenarios,
    require("./scenarios/approval-lifecycle.cjs").browserScenarios,
    require("./scenarios/real-checked-save.cjs").scenarios,
    require("./scenarios/real-workgraph.cjs").browserScenarios,
    require("./scenarios/real-images.cjs").browserScenarios,
    require("./scenarios/real-send-context.cjs").scenarios,
    require("./scenarios/real-durable-steer.cjs").scenarios,
    require("./scenarios/real-tab-isolation.cjs").scenarios,
    require("./scenarios/real-legacy-import.cjs").scenarios,
    require("./scenarios/real-sidebar-activity.cjs").scenarios,
  ];
  const requiredBrowser = browserModules.flat();
  assert(requiredBrowser.length >= 25, "real backend acceptance families must remain registered");
  for (const scenario of requiredBrowser) {
    assert.equal(browser.filter(item => item.id === scenario.id && item.backend === "real").length, 1, scenario.id);
  }
  for (const id of ["api-query-faults", "api-member-ingress", "api-identity-ingress", "api-member-send-restart", "api-identity-send-restart",
    ...require("./scenarios/approval-lifecycle.cjs").apiScenarios.map(item => item.id),
    ...require("./scenarios/real-workgraph.cjs").apiScenarios.map(item => item.id),
    ...require("./scenarios/real-routine-tools.cjs").apiScenarios.map(item => item.id),
    ...require("./scenarios/stream-parity.cjs").apiScenarios.map(item => item.id),
    ...require("./scenarios/real-correlation-overlap.cjs").apiScenarios.map(item => item.id)]) {
    assert.equal(api.filter(item => item.id === id && item.backend === "real").length, 1, id);
  }
  for (const runner of [browser, api]) {
    const shards = [1, 2, 3].flatMap(index => selectScenarios(runner, [`--shard=${index}/3`]).selected);
    assert.deepEqual(shards.map(item => item.id).sort(), selectScenarios(runner, []).selected.map(item => item.id).sort());
  }
});

const diagnostic = {
  id: "hard-stop", family: "runtime", backend: "real",
  diagnostic: { reason: "Known terminal projection defect", issues: ["https://github.com/lukacf/meerkat-meta/issues/460"] },
};

test("diagnostics are discoverable and explicitly selectable but excluded from default acceptance", () => {
  const registry = [scenarios[0], diagnostic, ...scenarios.slice(1)];
  assert.deepEqual(selectScenarios(registry, []).selected, scenarios);
  assert.deepEqual(selectScenarios(registry, ["--family=runtime"]).selected, [scenarios[0], scenarios[3]]);
  assert.deepEqual(selectScenarios(registry, ["--list"]).selected, registry);
  for (const args of [["--scenario=hard-stop"], ["--family=runtime", "--scenario=hard-stop"], ["--scenario=hard-stop", "--family=runtime"]]) {
    assert.deepEqual(selectScenarios(registry, args).selected, [diagnostic]);
  }
  assert.deepEqual(selectScenarios(registry, ["--scenario=hard-stop,scroll"]).selected, [diagnostic, scenarios[1]]);
  const shards = [1, 2].map(index => selectScenarios(registry, [`--shard=${index}/2`]).selected);
  assert.deepEqual(shards, [[scenarios[0], scenarios[2]], [scenarios[1], scenarios[3]]]);
  assert.throws(() => selectScenarios([diagnostic], []), /selection is empty/);
});

test("diagnostic exclusions require a concrete reason and issue references", () => {
  for (const invalid of [null, {}, { reason: " ", issues: ["issue"] }, { reason: "Known issue", issues: [] }, { reason: "Known issue", issues: [""] }]) {
    assert.throws(() => selectScenarios([...scenarios, { ...diagnostic, diagnostic: invalid }], []), /diagnostic metadata/);
  }
});

test("hard-interrupt repros retain executable registrations and linked diagnostic metadata", () => {
  const negative = require("./scenarios/real-routine-negative.cjs");
  const registered = [...negative.apiScenarios, ...negative.browserScenarios];
  const issues = ["https://github.com/lukacf/meerkat-meta/issues/460", "https://github.com/lukacf/meerkat/issues/1233"];
  for (const id of ["api-routine-interrupt", "real-stock-routine-interrupt", "real-shared-routine-interrupt"]) {
    const [selected] = selectScenarios(registered, [`--scenario=${id}`]).selected;
    assert.equal(selected.id, id);
    assert.equal(typeof selected.run, "function");
    assert.match(selected.diagnostic?.reason ?? "", /hard.*cancel|hard.*interrupt/i);
    assert.match(selected.diagnostic.reason, /terminal|Working/);
    assert.deepEqual(selected.diagnostic.issues, issues);
    assert(!selectScenarios(registered, []).selected.some(item => item.id === id));
  }
  const defaults = selectScenarios(registered, []).selected;
  assert.equal(defaults.length, 3);
  assert(defaults.every(item => item.id.endsWith("cancel-after-boundary") && !item.diagnostic));
  for (const runner of ["api-e2e.cjs", "browser-e2e.cjs"]) {
    const listed = JSON.parse(execFileSync(process.execPath, [path.join(__dirname, runner), "--list"], { encoding: "utf8" }));
    const hard = listed.filter(item => item.id.endsWith("routine-interrupt"));
    assert.equal(hard.length, runner === "api-e2e.cjs" ? 1 : 2);
    for (const item of hard) assert.deepEqual(item.diagnostic.issues, issues);
  }
});

function runSyntheticRegistry(diagnosticFails = false) {
  const script = `
    const { runScenarios } = require(${JSON.stringify(path.join(__dirname, "scenario-registry.cjs"))});
    const registry = [
      { id: "ordinary", family: "runtime", backend: "mock", run: async () => {} },
      { ...${JSON.stringify(diagnostic)}, run: async () => { ${diagnosticFails ? 'throw new Error("diagnostic remains red")' : ""} } },
    ];
    runScenarios(registry, ["--scenario=ordinary,hard-stop"]).catch(error => {
      process.stderr.write(error.message); process.exitCode = 1;
    });
  `;
  return require("node:child_process").spawnSync(process.execPath, ["-e", script], { encoding: "utf8" });
}

test("explicit diagnostics never emit acceptance passes even if their run resolves", () => {
  const completed = runSyntheticRegistry();
  assert.equal(completed.status, 0, completed.stderr);
  assert.match(completed.stdout, /scenario:pass ordinary/);
  assert.doesNotMatch(completed.stdout, /scenario:pass hard-stop/);
  assert.match(completed.stdout, /scenario:diagnostic:complete hard-stop/);
  assert.match(completed.stdout, /excluded from release acceptance/);
  assert.match(completed.stdout, /Known terminal projection defect/);
  assert.match(completed.stdout, /https:\/\/github.com\/lukacf\/meerkat-meta\/issues\/460/);
  const failed = runSyntheticRegistry(true);
  assert.equal(failed.status, 1);
  assert.match(failed.stderr, /diagnostic remains red/);
  assert.doesNotMatch(failed.stdout, /scenario:pass hard-stop|scenario:diagnostic:complete hard-stop/);
});

test("default browser coverage includes activity filters against real runtime phases", () => {
  const runner = require("./browser-e2e.cjs").scenarios;
  const matches = selectScenarios(runner, []).selected.filter(item => item.id === "real-stock-sidebar-activity");
  assert.equal(matches.length, 1);
  assert.equal(matches[0].backend, "real");
  assert.equal(typeof matches[0].run, "function");
});

test("production and scoped lost-ack checks remain independently selectable", () => {
  const runner = require("./browser-e2e.cjs").scenarios;
  for (const id of ["real-embedded-quoted-lost-ack", "real-scoped-quoted-lost-ack", "real-scoped-lost-ack"]) {
    const selected = selectScenarios(runner, [`--scenario=${id}`]).selected;
    assert.equal(selected.length, 1);
    assert.equal(selected[0].id, id);
    assert.equal(selected[0].backend, "real");
    assert.equal(typeof selected[0].run, "function");
  }
});

test("production, scoped and shared assistant identity checks remain independently selectable", () => {
  const runner = require("./browser-e2e.cjs").scenarios;
  for (const id of ["real-embedded-assistant-identity", "real-stock-assistant-identity", "real-shared-assistant-identity"]) {
    const selected = selectScenarios(runner, [`--scenario=${id}`]).selected;
    assert.equal(selected.length, 1);
    assert.equal(selected[0].id, id);
    assert.equal(selected[0].family, "real-presentation");
    assert.equal(selected[0].backend, "real");
    assert.equal(typeof selected[0].run, "function");
  }
});
