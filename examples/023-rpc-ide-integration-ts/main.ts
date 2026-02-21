/**
 * 023 — JSON-RPC IDE Integration (TypeScript)
 *
 * The JSON-RPC server (`rkat-rpc`) is designed for IDE and desktop
 * integrations. Unlike REST, it keeps agents alive between turns via
 * `SessionRuntime` — enabling fast multi-turn conversations and real-time
 * event streaming without agent reconstruction overhead.
 *
 * What you'll learn:
 * - The JSON-RPC protocol (JSONL over stdio)
 * - Session lifecycle via RPC methods
 * - Event streaming via notifications
 * - Capability detection for feature flags
 * - Config management at runtime
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import { MeerkatClient } from "meerkat-sdk";

async function main() {
  const client = new MeerkatClient();

  // connect() spawns rkat-rpc and performs the initialize handshake
  await client.connect({ isolated: true });

  try {
    // ── 1. Capability detection ──
    console.log("=== 1. Capability Detection ===\n");
    const caps = await client.getCapabilities();
    console.log(`Contract version: ${caps.contract_version}`);
    console.log("Capabilities:");
    for (const cap of caps.capabilities) {
      const status =
        typeof cap.status === "string" ? cap.status : JSON.stringify(cap.status);
      console.log(`  ${cap.id}: ${status}`);
    }

    // Check for specific capabilities before using them
    if (client.hasCapability("builtins")) {
      console.log("\n  Built-in tools are available.");
    }
    if (client.hasCapability("comms")) {
      console.log("  Comms (peer messaging) is available.");
    }

    // ── 2. Config management ──
    console.log("\n=== 2. Runtime Config ===\n");
    const config = await client.getConfig();
    console.log(`Current config: ${JSON.stringify(config).substring(0, 200)}...`);

    // ── 3. Multi-turn session ──
    console.log("\n=== 3. Multi-Turn Session ===\n");

    // Turn 1: Create session
    const result1 = await client.createSession(
      "I'm building a VS Code extension. What are the key APIs I need?",
      {
        model: "claude-sonnet-4-5",
        system_prompt:
          "You are a VS Code extension development expert. Be concise and practical.",
      }
    );
    console.log(`Session: ${result1.session_id}`);
    console.log(`Turn 1: ${result1.text.substring(0, 200)}...\n`);

    // Turn 2: Continue (agent is kept alive in SessionRuntime — no reconstruction!)
    const result2 = await client.startTurn(
      result1.session_id,
      "How do I add a custom TreeView to the sidebar?"
    );
    console.log(`Turn 2: ${result2.text.substring(0, 200)}...\n`);

    // Turn 3: Streaming response
    console.log("Turn 3 (streaming): ");
    const stream = client.startTurnStreaming(
      result1.session_id,
      "Show me the activation function in package.json for this extension."
    );
    for await (const event of stream) {
      if (event.type === "text_delta") {
        process.stdout.write(event.delta);
      }
    }
    const result3 = await stream.result;
    console.log(`\n\nTokens: ${result3.usage.input_tokens + result3.usage.output_tokens}`);

    // ── 4. Session management ──
    console.log("\n=== 4. Session Management ===\n");

    const sessions = await client.listSessions();
    console.log(`Active sessions: ${sessions.length}`);

    const info = await client.readSession(result1.session_id);
    console.log(`Session ${result1.session_id.substring(0, 8)}...:`);
    console.log(`  Messages: ${info.message_count}`);
    console.log(`  Tokens: ${info.total_tokens}`);

    await client.archiveSession(result1.session_id);
    console.log(`  Archived.`);

    // ── 5. RPC protocol reference ──
    console.log("\n=== 5. JSON-RPC Protocol Reference ===\n");
    console.log(`
The TypeScript SDK wraps the JSON-RPC protocol over stdio.
Raw protocol example:

  → {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
  ← {"jsonrpc":"2.0","id":1,"result":{"contract_version":"0.3.4",...}}

  → {"jsonrpc":"2.0","id":2,"method":"session/create","params":{...}}
  ← {"jsonrpc":"2.0","method":"session/event","params":{...}}  (notification)
  ← {"jsonrpc":"2.0","method":"session/event","params":{...}}  (notification)
  ← {"jsonrpc":"2.0","id":2,"result":{...}}                   (final result)

Methods:
  initialize           Handshake
  session/create       Create + run first turn
  session/list         List sessions
  session/read         Get session state
  session/archive      Archive session
  turn/start           Continue session
  turn/interrupt       Cancel in-flight turn
  capabilities/get     List capabilities
  config/get           Read config
  config/set           Replace config
  config/patch         Merge-patch config
  comms/send           Send peer message
  comms/peers          List peers

Advantage over REST:
  - Agent stays alive in SessionRuntime (no reconstruction per turn)
  - Zero-latency multi-turn via stdio (no HTTP overhead)
  - Event streaming via notifications (no SSE setup)
  - Perfect for IDE extensions and desktop apps
`);
  } finally {
    await client.close();
  }
}

main().catch(console.error);
