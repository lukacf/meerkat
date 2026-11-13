#!/usr/bin/env node

/**
 * Standalone CLI for the @rkat/web provider proxy.
 *
 * Usage:
 *   ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy
 *   ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
 *   ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100 --cors-origin http://localhost:5173
 */

import { startProxy } from './index.mjs';

function parseArgs(args) {
  const opts = {};
  for (let i = 0; i < args.length; i++) {
    if (args[i] === '--port' && args[i + 1]) {
      opts.port = parseInt(args[i + 1], 10);
      i++;
    } else if (args[i] === '--host' && args[i + 1]) {
      opts.host = args[i + 1];
      i++;
    } else if (args[i] === '--cors-origin' && args[i + 1]) {
      opts.allowOrigin = args[i + 1];
      i++;
    } else if (args[i] === '--help' || args[i] === '-h') {
      console.log(`@rkat/web proxy — auth-injecting reverse proxy for LLM providers

Usage:
  npx @rkat/web proxy [options]

Options:
  --port <number>         Port to listen on (default: 3100, or PORT env var)
  --host <address>        Address to listen on (default: 127.0.0.1, the loopback
                          interface, which every user and process of this
                          machine reaches). The proxy does not check who calls
                          it. On a loopback address it answers loopback Host
                          names only (localhost, 127.0.0.1, [::1]). On any other
                          address it does not check the Host, and a caller on
                          the network that sends no Origin header is forwarded:
                          put the proxy behind your own authentication.
  --cors-origin <origin>  The one origin whose pages may call the proxy, or *
                          for any origin (default: loopback origins only, such
                          as http://localhost:5173). A request or preflight
                          from another origin is refused before the provider
                          is called. A request without an Origin header (curl,
                          server code) is not affected by this option.
  -h, --help              Show this help

Environment variables:
  ANTHROPIC_API_KEY       Anthropic API key (enables /anthropic/* routes)
  OPENAI_API_KEY          OpenAI API key (enables /openai/* routes)
  GEMINI_API_KEY          Gemini API key (enables /gemini/* routes)
  PORT                    Port to listen on (overridden by --port)

The WASM client uses per-provider base URLs to point at this proxy:
  anthropicBaseUrl: 'http://127.0.0.1:3100/anthropic'
  openaiBaseUrl:    'http://127.0.0.1:3100/openai'
  geminiBaseUrl:    'http://127.0.0.1:3100/gemini'
`);
      process.exit(0);
    }
  }
  return opts;
}

const opts = parseArgs(process.argv.slice(2));
startProxy(opts);
