/**
 * 023 — JSON-RPC IDE Integration (TypeScript)
 *
 * The JSON-RPC server (`rkat-rpc`) is designed for IDE and desktop
 * integrations. Unlike REST, it keeps agents alive between turns via
 * `SessionRuntime` for fast multi-turn workflows.
 *
 * What you'll learn:
 * - Session lifecycle via the SDK
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
    // Capabilities are fetched during connect() and available as a property.
    const caps = client.capabilities;
    console.log("Capabilities:");
    for (const cap of caps) {
      console.log(`  ${cap.id}: ${cap.status}`);
    }

    console.log("\n=== 2. Runtime Config ===\n");
    const config = await client.getConfig();
    console.log(`Config envelope: ${JSON.stringify(config).slice(0, 240)}...`);

    console.log("\n=== 3. Multi-Turn Session ===\n");

    // createSession() returns a Session object — the handle for multi-turn.
    const session = await client.createSession(
      "I'm building a VS Code extension. What are the key APIs I need?",
      {
        model: "claude-sonnet-4-5",
        systemPrompt:
          "You are a VS Code extension development expert. Be concise and practical.",
      },
    );
    console.log(`Session: ${session.id}`);
    console.log(`Turn 1: ${session.text.slice(0, 220)}...\n`);

    // Multi-turn: call session.turn() to continue the conversation.
    const result2 = await session.turn(
      "How do I add a custom TreeView to the sidebar?",
    );
    console.log(`Turn 2: ${result2.text.slice(0, 220)}...\n`);

    const result3 = await session.turn(
      "Show me a minimal package.json contribution + activationEvents example.",
    );
    console.log(`Turn 3: ${result3.text.slice(0, 220)}...`);
    console.log(`\nTokens: ${result3.usage.inputTokens + result3.usage.outputTokens}`);

    console.log("\n=== 4. Session Management ===\n");

    const sessions = await client.listSessions();
    console.log(`Active sessions: ${sessions.length}`);

    const info = await client.readSession(session.id);
    console.log(`Session ${session.id.slice(0, 8)}... summary:`);
    console.log(JSON.stringify(info, null, 2).slice(0, 300));

    await session.archive();
    console.log("Archived.");

    console.log("\n=== 5. JSON-RPC Protocol Reference ===\n");
    console.log(`
Methods used here (via SDK):
  initialize           Handshake (during connect())
  session/create       Create + run first turn
  session/list         List sessions
  session/read         Read session state
  session/archive      Archive session
  turn/start           Continue session
  capabilities/get     List runtime capabilities (during connect())
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
