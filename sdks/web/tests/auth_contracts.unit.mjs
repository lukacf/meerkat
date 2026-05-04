import assert from 'node:assert/strict';
import test from 'node:test';

import { Auth } from '../dist/auth.js';
import {
  parseWireAuthError,
  parseWireAuthProfile,
  parseWireBackendProfile,
  parseWireRealmConnectionSet,
} from '../dist/generated/auth.js';

const authBinding = { realm: 'dev', binding: 'default_openai' };

const authProfile = {
  id: 'prod_env_key',
  provider: 'openai',
  auth_method: 'api_key',
  source_kind: 'env',
};

const backendProfile = {
  id: 'openai_api',
  provider: 'openai',
  backend_kind: 'openai_api',
  base_url: 'https://api.openai.com',
  options: { region: 'us-east-1' },
};

const binding = {
  id: 'default_openai',
  backend_profile: 'openai_api',
  auth_profile: 'prod_env_key',
  default_model: 'gpt-5.2',
};

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function authWithResponse(response) {
  const calls = [];
  const auth = new Auth({
    async request(method, params) {
      calls.push({ method, params });
      return clone(response);
    },
  });
  return { auth, calls };
}

test('Auth.listProfiles parses valid generated auth/connection payloads and round-trips', async () => {
  const payload = {
    realm_id: 'dev',
    auth_profiles: [authProfile],
    backend_profiles: [backendProfile],
    bindings: [binding],
  };
  const { auth, calls } = authWithResponse(payload);

  const result = await auth.listProfiles('dev');

  assert.deepEqual(result, payload);
  assert.deepEqual(JSON.parse(JSON.stringify(result)), payload);
  assert.deepEqual(calls, [
    { method: 'auth/profile/list', params: { realm_id: 'dev' } },
  ]);
});

test('generated realm connection parser round-trips valid connection payloads', () => {
  const payload = {
    realm_id: 'dev',
    backends: { openai_api: backendProfile },
    auth_profiles: { prod_env_key: authProfile },
    bindings: { default_openai: binding },
    default_binding: 'default_openai',
  };

  const result = parseWireRealmConnectionSet(clone(payload));

  assert.deepEqual(result, payload);
  assert.deepEqual(JSON.parse(JSON.stringify(result)), payload);
});

test('Auth helpers fail closed before transport for unknown provider and auth method strings', async () => {
  let called = false;
  const auth = new Auth({
    async request() {
      called = true;
      return {};
    },
  });

  await assert.rejects(
    () =>
      auth.loginStart({
        provider: 'not_a_provider',
        redirect_uri: 'http://localhost:1455/callback',
        realm_id: 'dev',
        binding_id: 'default_openai',
      }),
    /provider/,
  );
  assert.equal(called, false);

  await assert.rejects(
    () =>
      auth.createProfile({
        realm_id: 'dev',
        binding_id: 'default_openai',
        auth_method: 'password',
        secret: 'sk-test',
      }),
    /auth_method/,
  );
  assert.equal(called, false);
});

test('Auth response parsers reject unknown backend, source, provider, auth, and state strings', async () => {
  await assert.rejects(
    () =>
      authWithResponse({
        realm_id: 'dev',
        auth_profiles: [authProfile],
        backend_profiles: [{ ...backendProfile, backend_kind: 'mystery_backend' }],
        bindings: [binding],
      }).auth.listProfiles('dev'),
    /backend_kind/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        realm_id: 'dev',
        auth_profiles: [{ ...authProfile, source_kind: 'browser_local_storage' }],
        backend_profiles: [backendProfile],
        bindings: [binding],
      }).auth.listProfiles('dev'),
    /source_kind/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        ...identityPayload(),
        provider: 'provider_from_the_future',
        auth_method: 'api_key',
        stored: true,
      }).auth.createProfile({
        realm_id: 'dev',
        binding_id: 'default_openai',
        auth_method: 'api_key',
        secret: 'sk-test',
      }),
    /provider/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        ...identityPayload(),
        provider: 'openai',
        auth_method: 'magic_token',
        stored: true,
      }).auth.createProfile({
        realm_id: 'dev',
        binding_id: 'default_openai',
        auth_method: 'api_key',
        secret: 'sk-test',
      }),
    /auth_method/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        ...identityPayload(),
        profile_id: 'prod_env_key',
        provider: 'openai',
        auth_method: 'api_key',
        state: 'credential_present',
        has_refresh_token: false,
      }).auth.status('dev', 'default_openai'),
    /state/,
  );
});

test('generated auth parsers reject provider-specific auth and backend mismatches', async () => {
  assert.throws(
    () => parseWireAuthProfile({ ...authProfile, auth_method: 'google_oauth' }),
    /auth_method/,
  );
  assert.throws(
    () => parseWireBackendProfile({ ...backendProfile, backend_kind: 'vertex_ai' }),
    /backend_kind/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        realm_id: 'dev',
        auth_profiles: [{ ...authProfile, auth_method: 'google_oauth' }],
        backend_profiles: [backendProfile],
        bindings: [binding],
      }).auth.listProfiles('dev'),
    /auth_method/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        realm_id: 'dev',
        auth_profiles: [authProfile],
        backend_profiles: [{ ...backendProfile, backend_kind: 'vertex_ai' }],
        bindings: [binding],
      }).auth.listProfiles('dev'),
    /backend_kind/,
  );
});

test('generated auth list and realm parsers reject unresolved or cross-provider bindings', async () => {
  const geminiAuthProfile = {
    id: 'gemini_oauth',
    provider: 'gemini',
    auth_method: 'google_oauth',
    source_kind: 'external_resolver',
  };
  const crossProviderBinding = {
    ...binding,
    id: 'bad_binding',
    auth_profile: 'gemini_oauth',
  };

  await assert.rejects(
    () =>
      authWithResponse({
        realm_id: 'dev',
        auth_profiles: [geminiAuthProfile],
        backend_profiles: [backendProfile],
        bindings: [crossProviderBinding],
      }).auth.listProfiles('dev'),
    /auth_profile/,
  );

  assert.throws(
    () =>
      parseWireRealmConnectionSet({
        realm_id: 'dev',
        backends: { openai_api: backendProfile },
        auth_profiles: { gemini_oauth: geminiAuthProfile },
        bindings: { bad_binding: crossProviderBinding },
      }),
    /auth_profile/,
  );

  assert.throws(
    () =>
      parseWireRealmConnectionSet({
        realm_id: 'dev',
        backends: { openai_api: backendProfile },
        auth_profiles: { prod_env_key: authProfile },
        bindings: {
          default_openai: { ...binding, backend_profile: 'missing_backend' },
        },
      }),
    /backend_profile/,
  );

  await assert.rejects(
    () =>
      authWithResponse({
        realm_id: 'dev',
        auth_profiles: [authProfile],
        backend_profiles: [backendProfile],
        bindings: [{ ...binding, auth_profile: 'missing_auth' }],
      }).auth.listProfiles('dev'),
    /auth_profile/,
  );
});

test('Auth.status and device-complete ready parse generated terminal payloads', async () => {
  const status = {
    ...identityPayload(),
    profile_id: 'prod_env_key',
    provider: 'openai',
    auth_method: 'api_key',
    state: 'valid',
    has_refresh_token: false,
  };
  assert.deepEqual(await authWithResponse(status).auth.status('dev', 'default_openai'), status);

  const ready = {
    realm_id: 'dev',
    binding_id: 'anthropic_main',
    auth_binding: { realm: 'dev', binding: 'anthropic_main' },
    profile_id: 'console',
    state: 'ready',
    provider: 'anthropic',
    expires_at: null,
    has_refresh_token: true,
    scopes: ['org:create_api_key'],
  };
  const { auth, calls } = authWithResponse(ready);

  assert.deepEqual(
    await auth.loginDeviceComplete({
      provider: 'anthropic',
      device_code: 'device-1',
      realm_id: 'dev',
      binding_id: 'anthropic_main',
    }),
    ready,
  );
  assert.equal(calls[0].method, 'auth/login/device_complete');
});

test('generated auth error parser preserves typed freshness failures', () => {
  for (const kind of [
    'stale_credential',
    'refresh_required',
    'lease_absent',
    'user_reauth_required',
  ]) {
    assert.deepEqual(parseWireAuthError({ kind }), { kind });
  }

  assert.throws(
    () => parseWireAuthError({ kind: 'provider_local_fallback' }),
    /kind/,
  );
});

function identityPayload() {
  return {
    realm_id: 'dev',
    binding_id: 'default_openai',
    auth_binding: authBinding,
    profile_id: 'prod_env_key',
  };
}
