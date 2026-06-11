const logNode = document.querySelector('#log');
const statusNode = document.querySelector('#status');
const LIVE_LLM_TIMEOUT_MS = Number.parseInt(
  globalThis.MEERKAT_BROWSER_LIVE_SMOKE_TIMEOUT_MS ?? '180000',
  10,
);

function write(message) {
  const line = typeof message === 'string' ? message : JSON.stringify(message);
  logNode.textContent += `${line}\n`;
  logNode.scrollTop = logNode.scrollHeight;
  console.log(`[browser-smoke] ${line}`);
}

function setStatus(text, kind = '') {
  statusNode.textContent = text;
  statusNode.className = kind ? `status ${kind}` : 'status';
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

async function pollUntil(action, predicate, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  let lastValue;
  while (Date.now() < deadline) {
    lastValue = await action();
    if (predicate(lastValue)) {
      return lastValue;
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`${label} timed out after ${timeoutMs}ms; last value: ${JSON.stringify(lastValue)}`);
}

async function withTimeout(promise, timeoutMs, label) {
  let timeoutId;
  const timeoutPromise = new Promise((_, reject) => {
    timeoutId = setTimeout(
      () => reject(new Error(`${label} timed out after ${timeoutMs}ms`)),
      timeoutMs,
    );
  });
  try {
    return await Promise.race([promise, timeoutPromise]);
  } finally {
    clearTimeout(timeoutId);
  }
}

// Trusted signer for the browser-safe mobpack fixture. The fixture is signed
// with the repo dev key (hex '09' * 32); the committed
// fixtures/browser_safe_mobpack/signature.toml carries this public key, and
// the runtime config below registers it so the fixture verifies under the
// default strict trust policy (no permissive opt-out).
const FIXTURE_TRUSTED_SIGNERS = {
  'browser-smoke': 'fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618',
};

function makeRuntimeConfig() {
  return {
    api_key: 'sk-browser-smoke',
    anthropic_api_key: 'sk-browser-smoke',
    model: 'claude-sonnet-4-5',
    base_url: `${window.location.origin}/anthropic`,
    anthropic_base_url: `${window.location.origin}/anthropic`,
    mobpack_trust: {
      policy: 'strict',
      trusted_signers: { signers: FIXTURE_TRUSTED_SIGNERS },
    },
  };
}

// WASM content-bearing exports take the tagged content-input wire shape
// ({"text": ...} | {"blocks": [...]}); raw strings fail closed with
// INVALID_PARAMS.
function taggedPrompt(text) {
  return JSON.stringify({ text });
}

// `start_turn` resolves with the canonical WireRunResult (same shape as RPC
// `turn/start`); failures reject with a typed error envelope — there is no
// fabricated `status` field on success.
function assertCompletedTurn(turn, label) {
  assert(
    typeof turn.session_id === 'string' && turn.session_id.length > 0,
    `${label}: WireRunResult must carry session_id: ${JSON.stringify(turn)}`,
  );
  assert(
    typeof turn.text === 'string' && turn.text.length > 0,
    `${label}: WireRunResult must carry non-empty text: ${JSON.stringify(turn)}`,
  );
}

async function loadRawWasm() {
  const module = await import('/runtime/meerkat_web_runtime.js');
  await module.default();
  return module;
}

async function fetchMobpackBytes() {
  const response = await fetch('/fixtures/browser-safe.mobpack');
  assert(response.ok, `failed to fetch fixture mobpack: ${response.status}`);
  return new Uint8Array(await response.arrayBuffer());
}

function collectEventTypes(items) {
  return items.map(
    (item) => item?.payload?.type ?? item?.event?.type ?? item?.type ?? 'unknown',
  );
}

async function scenarioRawSession001({ wasm }) {
  write('BROWSER-RAW-SESSION-001: init runtime from config');
  const init = JSON.parse(wasm.init_runtime_from_config(JSON.stringify(makeRuntimeConfig())));
  assert(init.status === 'initialized', `unexpected init status: ${JSON.stringify(init)}`);
  assert(typeof wasm.runtime_version() === 'string' && wasm.runtime_version().length > 0, 'runtime_version missing');

  const handle = wasm.create_session_simple(
    JSON.stringify({
      model: 'claude-sonnet-4-5',
      api_key: 'sk-browser-smoke',
      base_url: `${window.location.origin}/anthropic`,
      anthropic_base_url: `${window.location.origin}/anthropic`,
    }),
  );
  assert(Number.isInteger(handle) && handle > 0, `unexpected session handle: ${handle}`);

  const staged = JSON.parse(
    await wasm.append_system_context(
      handle,
      JSON.stringify({
        text: 'Remember the browser smoke control plane.',
        source: 'browser-live-smoke',
        idempotency_key: 'browser-live-smoke-ctx-1',
      }),
    ),
  );
  assert(staged.status === 'staged', `append_system_context failed: ${JSON.stringify(staged)}`);

  const turn = JSON.parse(await wasm.start_turn(handle, taggedPrompt('Say raw browser smoke ok.')));
  assertCompletedTurn(turn, 'raw session turn');

  const events = JSON.parse(wasm.poll_events(handle));
  const eventTypes = collectEventTypes(events);
  assert(eventTypes.some((type) => type === 'text_complete'), `expected text_complete event, got ${eventTypes.join(', ')}`);

  const state = JSON.parse(wasm.get_session_state(handle));
  assert(state.model === 'claude-sonnet-4-5', `unexpected session model: ${JSON.stringify(state)}`);
  if (Object.hasOwn(state, 'run_counter')) {
    assert(Number.isInteger(state.run_counter) && state.run_counter >= 0, `unexpected canonical run_counter: ${JSON.stringify(state)}`);
  }
  assert(state.message_count >= 1, `expected canonical message_count >= 1, got ${JSON.stringify(state)}`);

  // destroy_session retires the browser-local handle (fail-closed): later
  // calls on the stale handle must fail with invalid_session_handle rather
  // than resolving an addressable archived projection.
  wasm.destroy_session(handle);
  let stateRetired = false;
  try {
    wasm.get_session_state(handle);
  } catch (error) {
    stateRetired = String(error).includes('invalid_session_handle');
  }
  assert(stateRetired, 'expected get_session_state() on a destroyed handle to fail closed with invalid_session_handle');

  let turnRetired = false;
  try {
    await wasm.start_turn(handle, taggedPrompt('stale browser handle must not control archived session'));
  } catch (error) {
    turnRetired = String(error).includes('invalid_session_handle');
  }
  assert(turnRetired, 'expected start_turn() on a destroyed handle to fail closed with invalid_session_handle');
}

async function scenarioRawRecall002({ wasm }) {
  write('BROWSER-RAW-RECALL-002: follow-up recall through a live browser session');
  const init = JSON.parse(wasm.init_runtime_from_config(JSON.stringify(makeRuntimeConfig())));
  assert(init.status === 'initialized', `unexpected init status: ${JSON.stringify(init)}`);

  const handle = wasm.create_session_simple(
    JSON.stringify({
      model: 'claude-sonnet-4-5',
      api_key: 'sk-browser-smoke',
      base_url: `${window.location.origin}/anthropic`,
      anthropic_base_url: `${window.location.origin}/anthropic`,
    }),
  );
  const staged = JSON.parse(
    await wasm.append_system_context(
      handle,
      JSON.stringify({
        text: 'Always include the marker BROWSER_CTX_OK when asked to recall state.',
        source: 'browser-live-smoke',
        idempotency_key: 'browser-live-smoke-ctx-2',
      }),
    ),
  );
  assert(staged.status === 'staged', `append_system_context failed: ${JSON.stringify(staged)}`);

  const firstTurn = JSON.parse(
    await wasm.start_turn(handle, taggedPrompt('Remember the codename BrowserNebula and reply briefly.')),
  );
  assertCompletedTurn(firstTurn, 'first recall turn');

  const secondTurn = JSON.parse(
    await wasm.start_turn(
      handle,
      taggedPrompt('What codename did I ask you to remember, and what marker should you include?'),
    ),
  );
  assertCompletedTurn(secondTurn, 'second recall turn');
  const textLower = secondTurn.text.toLowerCase();
  assert(textLower.includes('browsernebula') || textLower.includes('browser nebula'), `unexpected recall text: ${secondTurn.text}`);
  assert(textLower.includes('browser_ctx_ok'), `expected browser context marker in ${secondTurn.text}`);

  wasm.destroy_session(handle);
}

async function scenarioMobpackSession003({ wasm }) {
  write('BROWSER-MOBPACK-SESSION-003: inspect browser-safe mobpack fixture');
  const mobpackBytes = await fetchMobpackBytes();
  const inspect = JSON.parse(wasm.inspect_mobpack(mobpackBytes));
  assert(inspect.manifest.name === 'Browser Smoke Pack', `unexpected manifest: ${JSON.stringify(inspect)}`);
  assert(inspect.definition.id === 'browser-smoke-pack', `unexpected definition id: ${JSON.stringify(inspect)}`);
  assert(Array.isArray(inspect.skills) && inspect.skills.length === 1, `unexpected skills: ${JSON.stringify(inspect.skills)}`);
  assert(!inspect.capabilities || inspect.capabilities.includes('comms'), `unexpected capabilities: ${JSON.stringify(inspect.capabilities)}`);

  const init = JSON.parse(wasm.init_runtime(mobpackBytes, JSON.stringify(makeRuntimeConfig())));
  assert(init.status === 'initialized', `mobpack init failed: ${JSON.stringify(init)}`);

  const handle = wasm.create_session(
    mobpackBytes,
    JSON.stringify({
      model: 'claude-sonnet-4-5',
      api_key: 'sk-browser-smoke',
      base_url: `${window.location.origin}/anthropic`,
      anthropic_base_url: `${window.location.origin}/anthropic`,
      system_prompt: 'Append a browser-safe verification line.',
    }),
  );
  const turn = JSON.parse(await wasm.start_turn(handle, taggedPrompt('Summarize the browser-safe mobpack.')));
  assertCompletedTurn(turn, 'mobpack session turn');

  const state = JSON.parse(wasm.get_session_state(handle));
  assert(state.mob_id === 'browser-smoke-pack', `unexpected mobpack session state: ${JSON.stringify(state)}`);
  wasm.destroy_session(handle);
}

async function scenarioRawMob004({ wasm }) {
  write('BROWSER-RAW-MOB-004: create mob and drive runtime-backed member lifecycle');
  const init = JSON.parse(wasm.init_runtime_from_config(JSON.stringify(makeRuntimeConfig())));
  assert(init.status === 'initialized', `unexpected init status for mob scenario: ${JSON.stringify(init)}`);

  const mobId = await wasm.mob_create(
    JSON.stringify({
      id: 'browser-live-smoke-mob',
      profiles: {
        worker: {
          model: 'claude-sonnet-4-5',
          peer_description: 'Browser smoke worker',
          external_addressable: true,
          tools: {
            builtins: false,
            shell: false,
            comms: true,
            memory: false,
            mob: false,
          },
        },
      },
      wiring: {
        auto_wire_orchestrator: false,
        role_wiring: [],
      },
    }),
  );
  assert(mobId === 'browser-live-smoke-mob', `unexpected mob id: ${mobId}`);

  const spawn = JSON.parse(
    await withTimeout(
      wasm.mob_spawn(
        mobId,
        JSON.stringify([
          {
            profile: 'worker',
            agent_identity: 'worker-1',
            runtime_mode: 'turn_driven',
          },
        ]),
      ),
      LIVE_LLM_TIMEOUT_MS,
      'mob_spawn',
    ),
  );
  assert(
    Array.isArray(spawn) &&
      spawn[0]?.status === 'spawned' &&
      typeof spawn[0]?.result?.member_ref === 'string',
    `spawn failed: ${JSON.stringify(spawn)}`,
  );

  const initialMembers = JSON.parse(await wasm.mob_list_members(mobId));
  assert(initialMembers.length === 1, `expected one member after spawn: ${JSON.stringify(initialMembers)}`);

  const subscriptionHandle = await wasm.mob_member_subscribe(mobId, 'worker-1');
  await withTimeout(
    wasm.mob_member_send(
      mobId,
      'worker-1',
      JSON.stringify({
        content: 'Reply with a browser mob smoke acknowledgement.',
        handling_mode: 'queue',
      }),
    ),
    LIVE_LLM_TIMEOUT_MS,
    'mob_member_send',
  );
  const seenSubscriptionItems = [];
  const subscriptionItems = await pollUntil(
    async () => {
      const freshItems = JSON.parse(wasm.poll_subscription(subscriptionHandle));
      if (Array.isArray(freshItems)) {
        seenSubscriptionItems.push(...freshItems);
      }
      return seenSubscriptionItems;
    },
    (items) => {
      const eventTypes = collectEventTypes(items);
      return (
        eventTypes.includes('text_complete') &&
        (eventTypes.includes('turn_completed') || eventTypes.includes('run_completed'))
      );
    },
    LIVE_LLM_TIMEOUT_MS,
    'member subscription terminal completion',
  );
  const payloadTypes = collectEventTypes(subscriptionItems);
  assert(payloadTypes.includes('text_complete'), `expected text_complete payload, got ${payloadTypes.join(', ')}`);
  wasm.close_subscription(subscriptionHandle);

  await withTimeout(
    wasm.mob_respawn(mobId, 'worker-1', taggedPrompt('Reset the browser smoke worker.')),
    LIVE_LLM_TIMEOUT_MS,
    'mob_respawn',
  );
  await withTimeout(wasm.mob_retire(mobId, 'worker-1'), LIVE_LLM_TIMEOUT_MS, 'mob_retire');

  const finalMembers = JSON.parse(await wasm.mob_list_members(mobId));
  assert(finalMembers.length === 0, `expected no members after retire: ${JSON.stringify(finalMembers)}`);
}

const SCENARIOS = {
  'BROWSER-RAW-SESSION-001': scenarioRawSession001,
  'BROWSER-RAW-RECALL-002': scenarioRawRecall002,
  'BROWSER-MOBPACK-SESSION-003': scenarioMobpackSession003,
  'BROWSER-RAW-MOB-004': scenarioRawMob004,
};

async function runScenarios(requestedIds = []) {
  const selectedIds =
    requestedIds.length > 0 ? requestedIds : Object.keys(SCENARIOS);
  const unknownIds = selectedIds.filter((id) => !SCENARIOS[id]);
  assert(unknownIds.length === 0, `unknown scenario ids: ${unknownIds.join(', ')}`);

  setStatus('running', 'running');
  logNode.textContent = '';
  const wasm = await loadRawWasm();
  const results = [];

  for (const id of selectedIds) {
    const startedAt = performance.now();
    write(`>>> ${id} starting`);
    try {
      await SCENARIOS[id]({ wasm });
      const durationMs = Math.round(performance.now() - startedAt);
      const result = { id, status: 'passed', duration_ms: durationMs };
      results.push(result);
      write(`<<< ${id} passed in ${durationMs}ms`);
    } catch (error) {
      const durationMs = Math.round(performance.now() - startedAt);
      const message = error instanceof Error ? error.stack ?? error.message : String(error);
      const result = { id, status: 'failed', duration_ms: durationMs, error: message };
      results.push(result);
      write(`<<< ${id} failed in ${durationMs}ms`);
      write(message);
      setStatus('failed', 'fail');
      return {
        ok: false,
        results,
      };
    }
  }

  setStatus('passed', 'pass');
  return {
    ok: true,
    results,
  };
}

window.__liveSmoke = {
  run: runScenarios,
};

write('browser live smoke page ready');
