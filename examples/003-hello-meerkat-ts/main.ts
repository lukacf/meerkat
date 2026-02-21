/**
 * 003 — Hello Meerkat (TypeScript SDK)
 *
 * The simplest TypeScript agent: connect to the Meerkat runtime, create a
 * session, and print the response. Like the Python SDK, the TypeScript SDK
 * spawns `rkat-rpc` as a child process using JSON-RPC over stdio.
 *
 * What you'll learn:
 * - Connecting to the Meerkat runtime from Node.js
 * - Creating a session with typed options
 * - Reading the Session result
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import { MeerkatClient } from "@rkat/sdk";

async function main() {
  const client = new MeerkatClient();
  await client.connect();

  try {
    const session = await client.createSession(
      "What makes Rust's ownership model unique? Answer in two sentences.",
      { model: "claude-sonnet-4-5" },
    );

    console.log(session.text);
    console.log("\n--- Stats ---");
    console.log(`Session:  ${session.id}`);
    console.log(`Turns:    ${session.turns}`);
    console.log(`Tokens:   ${session.usage.inputTokens + session.usage.outputTokens}`);
  } finally {
    await client.close();
  }
}

main().catch(console.error);
