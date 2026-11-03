import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import YAML from "yaml";

import { Meta } from "../../sdk/typescript/src/index.ts";

const here = dirname(fileURLToPath(import.meta.url));
const configDir = join(here, "config");
const scenario = YAML.parse(readFileSync(join(here, "scenario.yaml"), "utf-8")) as {
  agents: Array<Record<string, unknown>>;
  links: Array<[string, string]>;
};

assert.equal(scenario.agents.length, 8);
assert.ok(scenario.links.length >= 8);

for (const agent of scenario.agents) {
  assert.equal(typeof agent.identity, "string");
  assert.equal(typeof agent.profile, "string");
  assert.equal(typeof agent.console_group, "string");
  assert.equal(typeof agent.org, "string");
  assert.equal(typeof agent.lane, "string");
}

const consoleToml = readFileSync(join(configDir, "console.toml"), "utf-8");
assert.match(consoleToml, /title = "Foresight Studio"/);
assert.match(consoleToml, /group_by = \["labels\.console_group"/);
assert.match(consoleToml, /subgroup_by = \["labels\.org"/);
assert.match(consoleToml, /\[\[sidebar\.buttons\]\]/);

const builder = Meta.builder()
  .mob(resolve(configDir, "mob.toml"))
  .consoleConfig(resolve(configDir, "console.toml"))
  .consoleAuthRequired(false)
  .demoLlm()
  .gateway("/tmp/rpc_gateway");

assert.equal(builder._config.consoleConfigPath, resolve(configDir, "console.toml"));
assert.equal(builder._config.consoleRequireAppAuth, false);
assert.equal(builder._config.demoLlm, true);
assert.equal(builder._config.mobConfigPath, resolve(configDir, "mob.toml"));
assert.equal(builder._config.gatewayBin, "/tmp/rpc_gateway");

console.log("[foresight-smoke] scenario and SDK consoleConfig wiring look good");
