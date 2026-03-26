/**
 * Type-level compilation tests for @rkat/web.
 *
 * These tests verify that the public API types compile correctly.
 * They are checked with `tsc --noEmit` — no runtime execution needed.
 */

import type {
  Mob,
  RuntimeConfig,
  SessionConfig,
  SessionState,
  AppendSystemContextOptions,
  AppendSystemContextResult,
  MobAppendSystemContextResult,
  MobDefinition,
  SpawnSpec,
  AgentEvent,
  ToolCallback,
  MobLifecycleAction,
  MemberDeliveryReceipt,
  MobMemberSnapshot,
  MobHelperResult,
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

const appendSystemContextOptions: AppendSystemContextOptions = {
  text: 'Coordinate with the orchestrator.',
  source: 'mob',
  idempotencyKey: 'ctx-1',
};

const appendSystemContextResult: AppendSystemContextResult = {
  handle: 1,
  status: 'staged',
};

const sessionState: SessionState = {
  handle: 1,
  session_id: '00000000-0000-0000-0000-000000000001',
  mob_id: '',
  model: 'claude-sonnet-4-5',
  usage: { input_tokens: 1, output_tokens: 2 },
  run_counter: 0,
  message_count: 0,
  is_active: true,
  last_assistant_text: null,
};

const mobAppendSystemContextResult: MobAppendSystemContextResult = {
  mob_id: 'mob-1',
  meerkat_id: 'worker-1',
  session_id: '00000000-0000-0000-0000-000000000001',
  status: 'staged',
};

// ─── MobDefinition (matches Rust MobDefinition) ────────────────

const mobDef: MobDefinition = {
  id: 'test-mob',
  profiles: {
    worker: {
      model: 'claude-sonnet-4-5',
      peer_description: 'A worker agent.',
      skills: ['research'],
      tools: { builtins: false, shell: false, comms: true, memory: false, mob: false, mob_tasks: false },
    },
  },
  wiring: {
    auto_wire_orchestrator: false,
    role_wiring: [{ a: 'worker', b: 'reviewer' }],
  },
};

// ─── SpawnSpec ───────────────────────────────────────────────────

const spawnSpec: SpawnSpec = {
  profile: 'worker',
  meerkat_id: 'w1',
  runtime_mode: 'autonomous_host',
  initial_message: 'Hello',
  labels: { role: 'worker' },
};

// ─── Event narrowing (matches Rust AgentEvent serde) ────────────

function handleEvent(event: AgentEvent): string {
  switch (event.type) {
    case 'run_started':
      return event.prompt;
    case 'hook_started':
      return `${event.hook_id}:${event.point}`;
    case 'hook_completed':
      return `${event.hook_id}:${event.duration_ms}`;
    case 'hook_failed':
      return event.error;
    case 'hook_denied':
      return `${event.reason_code}:${event.message}`;
    case 'hook_rewrite_applied':
      return event.hook_id;
    case 'hook_patch_published':
      return event.hook_id;
    case 'text_delta':
      return event.delta;
    case 'text_complete':
      return event.content;
    case 'tool_call_requested':
      return `${event.name}:${event.id}`;
    case 'tool_result_received':
      return `${event.name}:${event.is_error}`;
    case 'turn_started':
      return `turn ${event.turn_number}`;
    case 'turn_completed':
      return `${event.stop_reason} ${event.usage.input_tokens}+${event.usage.output_tokens}`;
    case 'run_completed':
      return event.result;
    case 'run_failed':
      return event.error;
    case 'tool_execution_started':
      return `exec:${event.name}`;
    case 'tool_execution_completed':
      return `done:${event.name}:${event.duration_ms}ms`;
    case 'tool_execution_timed_out':
      return `timeout:${event.name}:${event.timeout_ms}`;
    case 'compaction_started':
      return `compact:${event.input_tokens}`;
    case 'compaction_completed':
      return `compact:${event.summary_tokens}`;
    case 'compaction_failed':
      return event.error;
    case 'budget_warning':
      return `${event.budget_type}:${event.percent}`;
    case 'retrying':
      return `${event.attempt}/${event.max_attempts}`;
    case 'skills_resolved':
      return `${event.skills.length}`;
    case 'skill_resolution_failed':
      return event.reference;
    case 'interaction_complete':
      return event.result;
    case 'interaction_failed':
      return event.error;
    case 'stream_truncated':
      return event.reason;
    case 'tool_config_changed':
      return event.payload.target;
    case 'reasoning_delta':
      return event.delta;
    case 'reasoning_complete':
      return event.content;
  }
}

// ─── ToolCallback ───────────────────────────────────────────────

const myTool: ToolCallback = async (args: string) => {
  const parsed = JSON.parse(args) as { input: string };
  return { content: parsed.input.toUpperCase(), is_error: false };
};

// ─── MobLifecycleAction ─────────────────────────────────────────

const actions: MobLifecycleAction[] = ['stop', 'resume', 'complete', 'reset', 'destroy'];

declare const mob: Mob;
const memberSendResult: Promise<MemberDeliveryReceipt> = mob.member('worker-1').send('hello');
const memberStatusResult: Promise<MobMemberSnapshot> = mob.memberStatus('worker-1');
const helperResult: Promise<MobHelperResult> = mob.spawnHelper('Summarize the latest findings.');
const forkedHelperResult: Promise<MobHelperResult> = mob.forkHelper(
  'worker-1',
  'Review the draft and suggest one improvement.',
);
const memberSubscription = mob.member('worker-1').subscribe();
const mobSubscription = mob.subscribeEvents();

// ─── Ensure all exports type-check (suppress unused warnings) ───

void minimalConfig;
void fullConfig;
void sessionConfig;
void sessionState;
void appendSystemContextOptions;
void appendSystemContextResult;
void mobAppendSystemContextResult;
void mobDef;
void spawnSpec;
void handleEvent;
void myTool;
void actions;
void memberSendResult;
void memberStatusResult;
void helperResult;
void forkedHelperResult;
void memberSubscription;
void mobSubscription;
