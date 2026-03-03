/**
 * Type-level compilation tests for @rkat/web.
 *
 * These tests verify that the public API types compile correctly.
 * They are checked with `tsc --noEmit` — no runtime execution needed.
 */

import type {
  RuntimeConfig,
  SessionConfig,
  MobDefinition,
  SpawnSpec,
  AgentEvent,
  ToolCallback,
  MobLifecycleAction,
} from '../src/index.js';

// ─── RuntimeConfig ──────────────────────────────────────────────

const minimalConfig: RuntimeConfig = {
  anthropicApiKey: 'sk-test',
};

const fullConfig: RuntimeConfig = {
  apiKey: 'sk-fallback',
  anthropicApiKey: 'sk-ant',
  openaiApiKey: 'sk-oai',
  geminiApiKey: 'sk-gem',
  model: 'claude-sonnet-4-5',
  maxSessions: 16,
  baseUrl: 'https://proxy.example.com',
  anthropicBaseUrl: 'https://proxy.example.com/anthropic',
  openaiBaseUrl: 'https://proxy.example.com/openai',
  geminiBaseUrl: 'https://proxy.example.com/gemini',
};

// ─── SessionConfig ──────────────────────────────────────────────

const sessionConfig: SessionConfig = {
  model: 'claude-sonnet-4-5',
  apiKey: 'sk-test',
  systemPrompt: 'You are helpful.',
  maxTokens: 1024,
  anthropicBaseUrl: 'https://proxy.example.com/anthropic',
  labels: { env: 'test' },
  additionalInstructions: ['Be concise.'],
};

// ─── MobDefinition ──────────────────────────────────────────────

const mobDef: MobDefinition = {
  id: 'test-mob',
  profiles: {
    worker: {
      model: 'claude-sonnet-4-5',
      system_prompt: 'You are a worker.',
      tools: { builtins: false, shell: false, comms: true, memory: false },
    },
  },
  wiring: [{ a: 'w1', b: 'w2' }],
};

// ─── SpawnSpec ───────────────────────────────────────────────────

const spawnSpec: SpawnSpec = {
  profile: 'worker',
  meerkat_id: 'w1',
  runtime_mode: 'autonomous_host',
  initial_message: 'Hello',
  labels: { role: 'worker' },
};

// ─── Event narrowing ────────────────────────────────────────────

function handleEvent(event: AgentEvent): string {
  switch (event.type) {
    case 'text_delta':
      return event.text;
    case 'text_complete':
      return event.text;
    case 'tool_use_start':
      return `${event.name}:${event.tool_use_id}`;
    case 'tool_result':
      return event.content;
    case 'turn_complete':
      return `${event.usage.input_tokens}+${event.usage.output_tokens}`;
    case 'turn_error':
      return event.error;
    case 'comms_received':
      return `${event.from}: ${event.body}`;
  }
}

// ─── ToolCallback ───────────────────────────────────────────────

const myTool: ToolCallback = async (args: string) => {
  const parsed = JSON.parse(args) as { input: string };
  return { content: parsed.input.toUpperCase(), is_error: false };
};

// ─── MobLifecycleAction ─────────────────────────────────────────

const actions: MobLifecycleAction[] = ['stop', 'resume', 'complete', 'destroy'];

// ─── Ensure all exports type-check (suppress unused warnings) ───

void minimalConfig;
void fullConfig;
void sessionConfig;
void mobDef;
void spawnSpec;
void handleEvent;
void myTool;
void actions;
