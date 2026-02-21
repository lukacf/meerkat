/**
 * 023 — JSON-RPC IDE Integration (TypeScript)
 *
 * The JSON-RPC server (`rkat-rpc`) is designed for IDE and desktop
 * integrations. Unlike REST, it keeps agents alive between turns via
 * `SessionRuntime` for fast multi-turn workflows.
 *
 * What you'll learn:
 * - Session lifecycle via RPC methods
 * - Capability detection for feature flags
 * - Config management at runtime
 * - Realm-isolated runtime startup
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import { MeerkatClient } from "@rkat/sdk";

async function main() {
  const client = new MeerkatClient();

  // connect() spawns rkat-rpc and performs the initialize handshake.
  await client.connect({ isolated: true });

  try {
    console.log("=== 1. Capability Detection ===\n");
    const caps = await client.getCapabilities();
    console.log(`Contract version: ${caps.contract_version}`);
    console.log("Capabilities:");
    for (const cap of caps.capabilities) {
      const status =
        typeof cap.status === "string" ? cap.status : JSON.stringify(cap.status);
      console.log(`  ${cap.id}: ${status}`);
    }

    console.log("\n=== 2. Runtime Config ===\n");
    const config = await client.getConfig();
    console.log(`Config envelope: ${JSON.stringify(config).slice(0, 240)}...`);

    console.log("\n=== 3. Multi-Turn Session ===\n");

    const result1 = await client.createSession({
      prompt: "I'm building a VS Code extension. What are the key APIs I need?",
      model: "claude-sonnet-4-5",
      system_prompt:
        "You are a VS Code extension development expert. Be concise and practical.",
    });
    console.log(`Session: ${result1.session_id}`);
    console.log(`Turn 1: ${result1.text.slice(0, 220)}...\n`);

    const result2 = await client.startTurn(
      result1.session_id,
      "How do I add a custom TreeView to the sidebar?",
    );
    console.log(`Turn 2: ${result2.text.slice(0, 220)}...\n`);

    const result3 = await client.startTurn(
      result1.session_id,
      "Show me a minimal package.json contribution + activationEvents example.",
    );
    console.log(`Turn 3: ${result3.text.slice(0, 220)}...`);
    console.log(`\nTokens: ${result3.usage.input_tokens + result3.usage.output_tokens}`);

    console.log("\n=== 4. Session Management ===\n");

    const sessions = await client.listSessions();
    console.log(`Active sessions: ${sessions.length}`);

    const info = await client.readSession(result1.session_id);
    console.log(`Session ${result1.session_id.slice(0, 8)}... summary:`);
    console.log(JSON.stringify(info, null, 2).slice(0, 300));

    await client.archiveSession(result1.session_id);
    console.log("Archived.");

    console.log("\n=== 5. JSON-RPC Protocol Reference ===\n");
    console.log(`
Methods used here:
  initialize           Handshake
  session/create       Create + run first turn
  session/list         List sessions
  session/read         Read session state
  session/archive      Archive session
  turn/start           Continue session
  capabilities/get     List runtime capabilities
  config/get           Read runtime config

Why JSON-RPC for IDE integrations:
  - Stateful sessions across turns
  - Low-latency local stdio transport
  - Runtime capability probing for feature flags
`);
  } finally {
    await client.close();
  }
}

main().catch(console.error);
