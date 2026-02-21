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
 * - Reading the WireRunResult
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import { MeerkatClient } from "@rkat/sdk";

async function main() {
  const client = new MeerkatClient();
  await client.connect();

  try {
    const result = await client.createSession({
      prompt: "What makes Rust's ownership model unique? Answer in two sentences.",
      model: "claude-sonnet-4-5",
    });

    console.log(result.text);
    console.log("\n--- Stats ---");
    console.log(`Session:  ${result.session_id}`);
    console.log(`Turns:    ${result.turns}`);
    console.log(`Tokens:   ${result.usage.input_tokens + result.usage.output_tokens}`);
  } finally {
    await client.close();
  }
}

main().catch(console.error);
