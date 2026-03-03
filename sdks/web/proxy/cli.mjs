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
    } else if (args[i] === '--cors-origin' && args[i + 1]) {
      opts.allowOrigin = args[i + 1];
      i++;
    } else if (args[i] === '--help' || args[i] === '-h') {
      console.log(`@rkat/web proxy — auth-injecting reverse proxy for LLM providers

Usage:
  npx @rkat/web proxy [options]

Options:
  --port <number>         Port to listen on (default: 3100, or PORT env var)
  --cors-origin <origin>  CORS allowed origin (default: *)
  -h, --help              Show this help

Environment variables:
  ANTHROPIC_API_KEY       Anthropic API key (enables /anthropic/* routes)
  OPENAI_API_KEY          OpenAI API key (enables /openai/* routes)
  GEMINI_API_KEY          Gemini API key (enables /gemini/* routes)
  PORT                    Port to listen on (overridden by --port)

The WASM client uses per-provider base URLs to point at this proxy:
  anthropicBaseUrl: 'http://localhost:3100/anthropic'
  openaiBaseUrl:    'http://localhost:3100/openai'
  geminiBaseUrl:    'http://localhost:3100/gemini'
`);
      process.exit(0);
    }
  }
  return opts;
}

const opts = parseArgs(process.argv.slice(2));
startProxy(opts);
