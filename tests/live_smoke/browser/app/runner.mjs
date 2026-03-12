const logNode = document.querySelector('#log');
const statusNode = document.querySelector('#status');
const LIVE_LLM_TIMEOUT_MS = 15_000;

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

function makeRuntimeConfig() {
  return {
    api_key: 'sk-browser-smoke',
    anthropic_api_key: 'sk-browser-smoke',
    model: 'claude-sonnet-4-5',
    base_url: `${window.location.origin}/anthropic`,
    anthropic_base_url: `${window.location.origin}/anthropic`,
  };
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
    wasm.append_system_context(
      handle,
      JSON.stringify({
        text: 'Remember the browser smoke control plane.',
        source: 'browser-live-smoke',
        idempotency_key: 'browser-live-smoke-ctx-1',
      }),
    ),
  );
  assert(staged.status === 'staged', `append_system_context failed: ${JSON.stringify(staged)}`);

  const turn = JSON.parse(await wasm.start_turn(handle, 'Say raw browser smoke ok.', '{}'));
  assert(turn.status === 'completed', `raw session turn failed: ${JSON.stringify(turn)}`);
  assert(turn.text.length > 0, `unexpected raw session text: ${turn.text}`);

  const events = JSON.parse(wasm.poll_events(handle));
  const eventTypes = collectEventTypes(events);
  assert(eventTypes.some((type) => type === 'text_complete'), `expected text_complete event, got ${eventTypes.join(', ')}`);

  const state = JSON.parse(wasm.get_session_state(handle));
  assert(state.model === 'claude-sonnet-4-5', `unexpected session model: ${JSON.stringify(state)}`);
  assert(state.run_counter >= 1, `expected run_counter >= 1, got ${JSON.stringify(state)}`);

  wasm.destroy_session(handle);
  let destroyed = false;
  try {
    wasm.get_session_state(handle);
  } catch (_error) {
    destroyed = true;
  }
  assert(destroyed, 'expected get_session_state() to fail after destroy_session()');
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
    wasm.append_system_context(
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
    await wasm.start_turn(handle, 'Remember the codename BrowserNebula and reply briefly.', '{}'),
  );
  assert(firstTurn.status === 'completed', `first recall turn failed: ${JSON.stringify(firstTurn)}`);

  const secondTurn = JSON.parse(
    await wasm.start_turn(
      handle,
      'What codename did I ask you to remember, and what marker should you include?',
      '{}',
    ),
  );
  assert(secondTurn.status === 'completed', `second recall turn failed: ${JSON.stringify(secondTurn)}`);
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
  const turn = JSON.parse(await wasm.start_turn(handle, 'Summarize the browser-safe mobpack.', '{}'));
  assert(turn.status === 'completed', `mobpack session turn failed: ${JSON.stringify(turn)}`);
  assert(turn.text.length > 0, `unexpected mobpack text: ${turn.text}`);

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
            mob_tasks: false,
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
    await wasm.mob_spawn(
      mobId,
      JSON.stringify([
        {
          profile: 'worker',
          meerkat_id: 'worker-1',
          runtime_mode: 'turn_driven',
        },
      ]),
    ),
  );
  assert(Array.isArray(spawn) && spawn[0]?.status === 'ok', `spawn failed: ${JSON.stringify(spawn)}`);

  const initialMembers = JSON.parse(await wasm.mob_list_members(mobId));
  assert(initialMembers.length === 1, `expected one member after spawn: ${JSON.stringify(initialMembers)}`);

  const subscriptionHandle = await wasm.mob_member_subscribe(mobId, 'worker-1');
  await wasm.mob_send_message(mobId, 'worker-1', 'Reply with a browser mob smoke acknowledgement.');
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

  await wasm.mob_respawn(mobId, 'worker-1', 'Reset the browser smoke worker.');
  await wasm.mob_retire(mobId, 'worker-1');

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
