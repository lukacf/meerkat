#!/usr/bin/env node
import { loadConfiguration } from "./mcp-config.js";
import { listProfiles, configurationHelp } from "./config-fields.js";

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "profiles") {
    process.stdout.write(JSON.stringify(listProfiles()) + "\n");
    return;
  }
  if (args.length === 3 && args[0] === "config-help" && args[1] === "--profile") {
    process.stdout.write(JSON.stringify(configurationHelp(args[2])) + "\n");
    return;
  }
  if (args.length !== 3 || !["check-config", "describe", "serve"].includes(args[0]!) || args[1] !== "--config" || !args[2]) throw 0;
  const config = await loadConfiguration(args[2]);
  if (args[0] === "check-config") {
    // Project declared selected references, never environment values or
    // permission to overwrite the host's protected process environment.
    const references = configurationHelp(config.profile).fields
      .filter(field => field.credential_reference && Object.hasOwn(config, field.name))
      .map(field => ({ field_path: [field.name], environment_variable: Reflect.get(config, field.name) }));
    process.stdout.write(JSON.stringify({ profile: config.profile, valid: true, scope: "configuration_only",
      credential_environment_references: references }) + "\n");
    return;
  }
  const { describeMcpConfiguration, serveMcpStdio } = await import("./mcp.js");
  if (args[0] === "describe") {
    process.stdout.write(JSON.stringify(describeMcpConfiguration(config)) + "\n");
    return;
  }
  const serving = serveMcpStdio(config, { onerror: () => process.stderr.write("Activepieces MCP protocol error\n") });
  const close = () => { void serving.close().catch(() => { process.exitCode = 1; }); };
  process.once("SIGINT", close);
  process.once("SIGTERM", close);
  try { await serving.closed; }
  finally { process.removeListener("SIGINT", close); process.removeListener("SIGTERM", close); await serving.close(); }
}
main().catch(() => {
  process.stderr.write("Activepieces MCP command failed; check configuration and dependencies.\n");
  process.exitCode = 1;
});
