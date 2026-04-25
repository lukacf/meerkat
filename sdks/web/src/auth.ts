/**
 * Web SDK auth bindings — plan §4c.10 + §4d.wasm.3.
 *
 * This module provides:
 *   - `Auth` — TypeScript wrapper for the `auth.*` RPC methods
 *     (auth.profile.create/list/get/delete/test, auth.login.*,
 *     auth.status.get, auth.logout). Surfaces targeting the JSON-RPC
 *     stdio server or the REST API over fetch should instantiate this
 *     class with the appropriate transport.
 *   - `registerExternalAuthResolver` — wraps the WASM-bundled
 *     `register_external_auth_resolver` binding so a browser host page
 *     can install an OAuth-backed resolver callback that hands Meerkat
 *     a resolved bearer token per `realm:binding` request.
 *   - `withConnectionRef` — convenience helper that wires an existing
 *     session config with a connection reference for `createSession`.
 *
 * The WASM runtime's session-creation path (plan §4d.wasm.2) takes
 * credentials either from bootstrap-time realm config (populated via
 * `initRuntimeFromConfig`) or from a host-registered external-auth
 * resolver. Per-session `apiKey` is deleted.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */

import type { SessionConfig } from './types.js';

/** Realm-qualified auth binding reference: `realm:binding[:profile]`. */
export type ConnectionRef = string;

/** Host-page resolver callback that the WASM runtime invokes when the
 * selected binding's credential source is `external_resolver`. Takes a
 * `realm:binding` key, returns a bearer token string (or a promise). */
export type ExternalAuthResolver = (
  bindingKey: string,
) => string | Promise<string>;

/** JSON-RPC-style transport used by the `Auth` class. Minimal: just
 * async request/response of (method, params) -> result. */
export interface AuthTransport {
  request<P = unknown, R = unknown>(method: string, params?: P): Promise<R>;
}

/** Auth profile returned by `auth.profile.create` / `auth.profile.get`. */
export interface AuthProfile {
  id: string;
  provider: string;
  authMethod: string;
  sourceKind: string;
  storageKind: string;
}

/** Auth status returned by `auth.status.get`. */
export interface AuthStatus {
  profileId: string;
  provider: string;
  authMethod: string;
  state:
    | 'valid'
    | 'expiring'
    | 'expired'
    | 'reauth_required'
    | 'refresh_failed'
    | 'unknown';
  expiresAt?: string;
  lastRefreshAt?: string;
  accountId?: string;
  lastError?: { kind: string; [key: string]: unknown };
}

/** OAuth login-start payload returned by `auth.login.start`. */
export interface OAuthLoginStart {
  authorizeUrl: string;
  state: string;
  pkceVerifier?: string;
}

/**
 * Typed wrapper over the meerkat RPC `auth.*` method family. Works
 * against any transport that speaks `(method, params) -> result`
 * (the stdio RPC binary, a REST-backed adapter, an in-process shim,
 * etc.).
 */
export class Auth {
  constructor(private readonly transport: AuthTransport) {}

  /** `auth.profile.create` — persist a new auth profile. */
  async createProfile(params: {
    provider: string;
    backendKind: string;
    authMethod: string;
    source: { kind: string; [key: string]: unknown };
    storage?: { kind: string; [key: string]: unknown };
  }): Promise<AuthProfile> {
    return this.transport.request<typeof params, AuthProfile>(
      'auth.profile.create',
      params,
    );
  }

  /** `auth.profile.list` — enumerate persisted auth profiles. */
  async listProfiles(): Promise<AuthProfile[]> {
    return this.transport.request<undefined, AuthProfile[]>(
      'auth.profile.list',
    );
  }

  /** `auth.profile.get` — fetch a single profile by id. */
  async getProfile(profileId: string): Promise<AuthProfile> {
    return this.transport.request<{ profileId: string }, AuthProfile>(
      'auth.profile.get',
      { profileId },
    );
  }

  /** `auth.profile.delete` — revoke + remove a profile. */
  async deleteProfile(profileId: string): Promise<void> {
    await this.transport.request<{ profileId: string }, void>(
      'auth.profile.delete',
      { profileId },
    );
  }

  /** `auth.profile.test` — exercise a profile against its provider. */
  async testProfile(profileId: string): Promise<AuthStatus> {
    return this.transport.request<{ profileId: string }, AuthStatus>(
      'auth.profile.test',
      { profileId },
    );
  }

  /** `auth.login.start` — begin an OAuth browser flow. */
  async loginStart(params: {
    provider: string;
    authMethod: string;
  }): Promise<OAuthLoginStart> {
    return this.transport.request<typeof params, OAuthLoginStart>(
      'auth.login.start',
      params,
    );
  }

  /** `auth.login.complete` — finalize an OAuth browser flow. */
  async loginComplete(params: {
    provider: string;
    code: string;
    state: string;
    pkceVerifier?: string;
  }): Promise<AuthProfile> {
    return this.transport.request<typeof params, AuthProfile>(
      'auth.login.complete',
      params,
    );
  }

  /** `auth.login.device_start` — device-code flow for keyboardless
   * hosts. */
  async loginDeviceStart(params: {
    provider: string;
    authMethod: string;
  }): Promise<{ userCode: string; verificationUrl: string; interval: number }> {
    return this.transport.request(
      'auth.login.device_start',
      params,
    );
  }

  /** `auth.login.device_complete` — single-poll completion leg for the
   * device-code flow. Call on the cadence returned by
   * `loginDeviceStart` (interval seconds). Returns `{ state: "pending"
   * | "slow_down" | "access_denied" | "expired" }` while the user has
   * not yet approved, and `{ state: "ready", profileId, ... }` once
   * tokens are persisted. */
  async loginDeviceComplete(params: {
    provider: string;
    deviceCode: string;
    realmId?: string;
    profileId?: string;
  }): Promise<
    | { state: 'pending' | 'slow_down' | 'access_denied' | 'expired' }
    | {
        state: 'ready';
        realmId: string;
        profileId: string;
        provider: string;
        expiresAt: string | null;
        hasRefreshToken: boolean;
        scopes: string[];
      }
  > {
    return this.transport.request(
      'auth.login.device_complete',
      params,
    );
  }

  /** `auth.login.provision_api_key` — Anthropic Console-OAuth → API
   * key provisioning (plan §4b.5). The caller runs a Console-scope
   * OAuth flow first (`org:create_api_key user:profile`), hands the
   * resulting `access_token` to this method, and the server POSTs to
   * Anthropic's create_api_key endpoint + persists the returned key
   * as an `oauth_to_api_key`-mode entry under
   * `<realmId>:<profileId>`. */
  async loginProvisionApiKey(params: {
    accessToken: string;
    realmId?: string;
    profileId?: string;
  }): Promise<{
    realmId: string;
    profileId: string;
    provider: 'anthropic';
    authMode: 'oauth_to_api_key';
    hasApiKey: boolean;
    scopes: string[];
  }> {
    return this.transport.request(
      'auth.login.provision_api_key',
      params,
    );
  }

  /** `auth.status.get` — current status for a stored profile. */
  async status(profileId: string): Promise<AuthStatus> {
    return this.transport.request<{ profileId: string }, AuthStatus>(
      'auth.status.get',
      { profileId },
    );
  }

  /** `auth.logout` — revoke + delete an auth profile. */
  async logout(profileId: string): Promise<void> {
    await this.transport.request<{ profileId: string }, void>(
      'auth.logout',
      { profileId },
    );
  }
}

/**
 * Install a host-page external-auth resolver in the loaded WASM
 * runtime. The resolver is consulted per-session when the binding's
 * credential source is `external_resolver`.
 *
 * Plan §4d.wasm.1 + §4d.wasm.3: browser OAuth flows run in the host
 * page; the resolver hands Meerkat a resolved bearer token on
 * demand. Meerkat never touches the host's refresh token.
 */
export function registerExternalAuthResolver(
  wasm: { register_external_auth_resolver: (cb: unknown) => void },
  resolver: ExternalAuthResolver,
): void {
  const adapter = (bindingKey: string): Promise<string> =>
    Promise.resolve(resolver(bindingKey));
  wasm.register_external_auth_resolver(adapter);
}

/**
 * Clear the previously-registered external-auth resolver. Subsequent
 * session creations will fall back to the realm config's default
 * credential source.
 */
export function clearExternalAuthResolver(wasm: {
  register_external_auth_resolver: (cb: unknown) => void;
}): void {
  wasm.register_external_auth_resolver(undefined);
}

/** Return a session config with an explicit auth connection binding. */
export function withConnectionRef<T extends SessionConfig>(
  connectionRef: ConnectionRef,
  config: T,
): T & { connectionRef: ConnectionRef } {
  return {
    ...config,
    connectionRef,
  };
}
