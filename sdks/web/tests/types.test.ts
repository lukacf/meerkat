/**
 * Type-level compilation tests for @rkat/web.
 *
 * These tests verify that the public API types compile correctly.
 * They are checked with `tsc --noEmit` — no runtime execution needed.
 */

import type {
  Mob,
  AuthBindingIdentity,
  AuthCredentialsCleared,
  AuthProfileCreated,
  AuthProfileDetail,
  AuthProfilesList,
  AuthLoginReady,
  AuthStatus,
  AuthTransport,
  ExternalAuthFailure,
  ExternalAuthLease,
  ExternalAuthResolver,
  ExternalAuthResolverResult,
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
  FlowStatus,
} from '../src/index.js';
import { Auth } from '../src/index.js';

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
  connectionRef: { realm: 'default', binding: 'anthropic' },
  systemPrompt: 'You are helpful.',
  maxTokens: 1024,
  labels: { env: 'test' },
  additionalInstructions: ['Be concise.'],
};

// ─── Auth RPC helpers ───────────────────────────────────────────

const authTransport: AuthTransport = {
  async request<P, R>(_method: string, _params?: P): Promise<R> {
    return {} as R;
  },
};
const auth = new Auth(authTransport);
const authList: Promise<AuthProfilesList> = auth.listProfiles('default');
const authGet: Promise<AuthProfileDetail> = auth.getProfile('default', 'openai');
const authCreate: Promise<AuthProfileCreated> = auth.createProfile({
  realm_id: 'default',
  binding_id: 'openai',
  auth_method: 'api_key',
  secret: 'sk-test',
});
const authDelete: Promise<AuthCredentialsCleared> = auth.deleteProfile(
  'default',
  'openai',
);
const authLogin: Promise<AuthLoginReady> = auth.loginComplete({
  provider: 'openai',
  code: 'oauth-code',
  state: 'oauth-state',
  redirect_uri: 'http://localhost:1455/callback',
  realm_id: 'default',
  binding_id: 'openai',
});
const authStatus: Promise<AuthStatus> = auth.status('default', 'openai');
const authLogout: Promise<AuthCredentialsCleared> = auth.logout('default', 'openai');
const authStatusWithProfile: Promise<AuthStatus> = auth.status(
  'default',
  'openai',
  'openai_apikey',
);
const authLogoutWithProfile: Promise<AuthCredentialsCleared> = auth.logout(
  'default',
  'openai',
  'openai_apikey',
);
const authBinding: AuthBindingIdentity = {
  realm_id: 'default',
  binding_id: 'openai',
  connection_ref: { realm: 'default', binding: 'openai' },
  profile_id: 'openai_apikey',
};

const externalAuthLease: ExternalAuthLease = {
  kind: 'inline_secret',
  secret: 'browser-access-token',
  metadata: {
    account_id: 'acct_browser',
    workspace_id: 'workspace_browser',
    user_id: 'user_browser',
  },
  expires_at: '2030-01-02T03:04:05Z',
};
const externalAuthFailure: ExternalAuthFailure = {
  kind: 'refresh_failed',
  detail: 'browser refresh token revoked',
};
const typedExternalAuthResolver: ExternalAuthResolver = async () => externalAuthLease;
const externalAuthResult: ExternalAuthResolverResult = externalAuthLease;
// @ts-expect-error bare bearer strings are not structured auth envelopes.
const rejectedStringExternalAuthResolver: ExternalAuthResolver = () => 'bearer-default-openai';
// @ts-expect-error resolver results must preserve lease metadata structurally.
const rejectedStringExternalAuthResult: ExternalAuthResolverResult = 'bearer-default-openai';

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
  agent_identity: 'worker-1',
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

const mobDefWithInternalToolBundle: MobDefinition = {
  id: 'test-mob-internal-tools',
  profiles: {
    worker: {
      model: 'claude-sonnet-4-5',
      tools: {
        // @ts-expect-error rust_bundles is runtime-internal and not a public web contract field.
        rust_bundles: ['internal-only'],
      },
    },
  },
};

void mobDefWithInternalToolBundle;

// ─── SpawnSpec ───────────────────────────────────────────────────

const spawnSpec: SpawnSpec = {
  profile: 'worker',
  agent_identity: 'w1',
  runtime_mode: 'autonomous_host',
  initial_message: 'Hello',
  labels: { role: 'worker' },
};

const spawnSpecWithoutGeneration: SpawnSpec = {
  profile: 'worker',
  agent_identity: 'w2',
};

// @ts-expect-error generation is runtime-owned and not a public spawn knob.
spawnSpecWithoutGeneration.generation = 1;

// ─── Event narrowing (matches Rust AgentEvent serde) ────────────

function handleEvent(event: AgentEvent): string {
  switch (event.type) {
    case 'run_started':
      return typeof event.prompt === 'string'
        ? event.prompt
        : event.prompt
            .map((block) => ('type' in block && block.type === 'text' ? block.text : ''))
            .join('');
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
      return event.reference ?? '';
    case 'interaction_complete':
      return event.result;
    case 'interaction_callback_pending':
      return `${event.tool_name}:${event.interaction_id}`;
    case 'interaction_failed':
      return event.error;
    case 'stream_truncated':
      return event.reason;
    case 'tool_config_changed':
      return event.payload.target;
    case 'background_job_completed':
      return `${event.display_name}:${event.status}`;
    case 'reasoning_delta':
      return event.delta;
    case 'reasoning_complete':
      return event.content;
    default: {
      const _exhaustive: never = event;
      return _exhaustive;
    }
  }
}

const typedSkillsResolved: AgentEvent = {
  type: 'skills_resolved',
  skills: [
    {
      source_uuid: '00000000-0000-4b11-8111-000000000001',
      skill_name: 'email-extractor',
    },
  ],
  injection_bytes: 128,
};
handleEvent(typedSkillsResolved);

// @ts-expect-error Legacy string-only skills_resolved payloads are not semantic AgentEvent data.
const legacyStringSkillsResolved: AgentEvent = { type: 'skills_resolved', skills: ['legacy/ref'], injection_bytes: 128 };

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
const helperWithConnectionResult: Promise<MobHelperResult> = mob.spawnHelper(
  'Summarize using the OpenAI binding.',
  { connectionRef: { realm: 'default', binding: 'openai', profile: 'work' } },
);
const forkedHelperResult: Promise<MobHelperResult> = mob.forkHelper(
  'worker-1',
  'Review the draft and suggest one improvement.',
  { connectionRef: { realm: 'default', binding: 'anthropic' } },
);
const flowStatusResult: Promise<FlowStatus | null> = mob.flowStatus('run-1');
const memberSubscription = mob.member('worker-1').subscribe();
const mobSubscription = mob.subscribeEvents();

// ─── Ensure all exports type-check (suppress unused warnings) ───

void minimalConfig;
void fullConfig;
void sessionConfig;
void authList;
void authGet;
void authCreate;
void authDelete;
void authLogin;
void authStatus;
void authLogout;
void authStatusWithProfile;
void authLogoutWithProfile;
void authBinding;
void externalAuthLease;
void externalAuthFailure;
void typedExternalAuthResolver;
void externalAuthResult;
void rejectedStringExternalAuthResolver;
void rejectedStringExternalAuthResult;
void sessionState;
void appendSystemContextOptions;
void appendSystemContextResult;
void mobAppendSystemContextResult;
void mobDef;
void spawnSpec;
void handleEvent;
void typedSkillsResolved;
void legacyStringSkillsResolved;
void myTool;
void actions;
void memberSendResult;
void memberStatusResult;
void helperResult;
void helperWithConnectionResult;
void forkedHelperResult;
void flowStatusResult;
void memberSubscription;
void mobSubscription;
