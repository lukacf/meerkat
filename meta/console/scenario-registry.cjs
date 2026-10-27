"use strict";

/** Stable scenario IDs make discovery, focused execution and CI sharding agree. */
function selectScenarios(scenarios, args) {
  const ids = scenarios.map((scenario) => scenario.id);
  if (new Set(ids).size !== ids.length || ids.some((id) => !id)) throw new Error("Scenario IDs must be unique and nonempty");
  for (const scenario of scenarios) {
    if (scenario.diagnostic === undefined) continue;
    const diagnostic = scenario.diagnostic;
    if (!diagnostic || typeof diagnostic.reason !== "string" || !diagnostic.reason.trim()
      || !Array.isArray(diagnostic.issues) || !diagnostic.issues.length
      || diagnostic.issues.some(issue => typeof issue !== "string" || !issue.trim())) {
      throw new Error(`Invalid diagnostic metadata for scenario: ${scenario.id}`);
    }
  }
  let selected = scenarios;
  let list = false;
  let explicitSelection = false;
  let shard;
  for (const arg of args) {
    if (arg === "--list") list = true;
    else if (arg === "--topology-only") selected = selected.filter((scenario) => scenario.family === "topology");
    else if (arg.startsWith("--scenario=")) {
      explicitSelection = true;
      const wanted = arg.slice("--scenario=".length).split(",");
      for (const id of wanted) if (!ids.includes(id)) throw new Error(`Unknown scenario: ${id}`);
      selected = selected.filter((scenario) => wanted.includes(scenario.id));
    } else if (arg.startsWith("--family=")) {
      const family = arg.slice("--family=".length);
      if (!scenarios.some((scenario) => scenario.family === family)) throw new Error(`Unknown scenario family: ${family}`);
      selected = selected.filter((scenario) => scenario.family === family);
    } else if (arg.startsWith("--shard=")) {
      const match = /^--shard=([1-9]\d*)\/([1-9]\d*)$/.exec(arg);
      if (!match || Number(match[1]) > Number(match[2])) throw new Error(`Invalid shard: ${arg}`);
      shard = { index: Number(match[1]) - 1, count: Number(match[2]) };
    } else throw new Error(`Unknown scenario argument: ${arg}`);
  }
  // Discovery includes known repros. Only an exact scenario selection opts into
  // executing them; ordinary acceptance and its shards exclude diagnostics.
  if (!list && !explicitSelection) selected = selected.filter(scenario => !scenario.diagnostic);
  if (shard) selected = selected.filter((_, index) => index % shard.count === shard.index);
  if (!selected.length) throw new Error("Scenario selection is empty");
  return { selected, list };
}

async function runScenarios(scenarios, args = process.argv.slice(2)) {
  const { selected, list } = selectScenarios(scenarios, args);
  if (list) {
    process.stdout.write(`${JSON.stringify(selected.map(({ id, family, backend, diagnostic }) => ({ id, family, backend, ...(diagnostic ? { diagnostic } : {}) })), null, 2)}\n`);
    return;
  }
  for (const scenario of selected) {
    if (scenario.diagnostic) {
      process.stdout.write(`scenario:diagnostic:start ${scenario.id} backend=${scenario.backend} excluded from release acceptance ${JSON.stringify(scenario.diagnostic)}\n`);
      await scenario.run();
      process.stdout.write(`scenario:diagnostic:complete ${scenario.id} excluded from release acceptance\n`);
      continue;
    }
    process.stdout.write(`scenario:start ${scenario.id} backend=${scenario.backend}\n`);
    await scenario.run();
    process.stdout.write(`scenario:pass ${scenario.id}\n`);
  }
}

module.exports = { selectScenarios, runScenarios };
