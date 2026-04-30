/**
 * Web SDK auth bindings — plan §4c.10 + §4d.wasm.3.
 *
 * This module provides:
 *   - `Auth` — TypeScript wrapper for the `auth/*` RPC methods
 *     (`auth/profile/create`, `auth/login/*`,
 *     `auth/status/get`, `auth/logout`). Surfaces targeting the JSON-RPC
 *     stdio server or the REST API over fetch should instantiate this
 *     class with the appropriate transport.
 *   - `registerExternalAuthResolver` — wraps the WASM-bundled
 *     `register_external_auth_resolver` binding so a browser host page
 *     can install an OAuth-backed resolver callback that hands Meerkat a
 *     typed lease envelope per structural connection reference.
 *   - `withConnectionRef` — convenience helper that wires an existing
 *     session config with a connection reference for `createSession`.
 *
 * The WASM runtime's session-creation path (plan §4d.wasm.2) takes
 * credentials either from bootstrap-time realm config (populated via
 * `initRuntimeFromConfig`) or from a host-registered external-auth
 * resolver. Per-session `apiKey` is deleted.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */

import type { ConnectionRef, SessionConfig } from './types.js';

/** Canonical WASM external-auth resolver handle for host-owned browser auth. */
export const WASM_EXTERNAL_AUTH_RESOLVER_HANDLE = 'wasm_host' as const;

/** Non-secret metadata attached to a resolved host-owned auth lease. */
export interface ExternalAuthMetadata {
  account_id?: string;
  workspace_id?: string;
  organization_id?: string;
  user_id?: string;
  plan?: string;
  route_hints?: unknown;
  provider_metadata?: unknown;
}

/** Typed auth lease shape accepted by the WASM external-auth resolver. */
export type ExternalAuthLease =
  | {
      kind: 'inline_secret';
      secret: string;
      metadata: ExternalAuthMetadata;
      expires_at?: string | null;
    }
  | {
      kind: 'static_headers';
      headers: Array<[string, string]>;
      metadata: ExternalAuthMetadata;
      expires_at?: string | null;
    }
  | {
      kind: 'dynamic_authorizer';
      metadata: ExternalAuthMetadata;
      expires_at?: string | null;
    }
  | {
      kind: 'none';
      metadata: ExternalAuthMetadata;
    };

/** Structured auth failure shape that host resolvers may reject with. */
export type ExternalAuthFailure =
  | { kind: 'missing_secret' }
  | { kind: 'unsupported_combination'; backend: string; auth: string }
  | { kind: 'missing_required_metadata'; field: string }
  | { kind: 'workspace_mismatch' }
  | { kind: 'expired' }
  | { kind: 'refresh_failed'; detail: string }
  | { kind: 'interactive_login_required' }
  | { kind: 'host_owned_unavailable' }
  | { kind: 'io'; detail: string }
  | { kind: 'other'; detail: string };

/** Successful resolver result. Leases preserve expiration and metadata across
 * the WASM boundary by using a structured envelope. */
export type ExternalAuthResolverResult = ExternalAuthLease;

/** Host-page resolver callback that the WASM runtime invokes when the
 * selected binding's credential source is `external_resolver`. Takes a
 * structural connection reference, and returns a typed lease envelope. Reject
 * the returned promise with `ExternalAuthFailure` to preserve stable failure
 * truth. */
export type ExternalAuthResolver = (
  connectionRef: ConnectionRef,
) => ExternalAuthResolverResult | Promise<ExternalAuthResolverResult>;

/** JSON-RPC-style transport used by the `Auth` class. Minimal: just
 * async request/response of (method, params) -> result. */
export interface AuthTransport {
  request<P = unknown, R = unknown>(method: string, params?: P): Promise<R>;
}

/** Wire projection of a configured auth profile. */
export interface AuthProfile {
  id: string;
  provider: string;
  auth_method: string;
  source_kind: string;
}

/** Binding-scoped identity returned by auth write/status calls. */
export interface AuthBindingIdentity {
  realm_id: string;
  binding_id: string;
  connection_ref: ConnectionRef;
  profile_id: string;
}

/** Auth profile list result returned by `auth/profile/list`. */
export interface AuthProfilesList {
  realm_id: string;
  auth_profiles: AuthProfile[];
}

/** Result returned by `auth/profile/get`. */
export interface AuthProfileDetail extends AuthBindingIdentity {
  auth_profile: AuthProfile;
}

/** Result returned by `auth/profile/create`. */
export interface AuthProfileCreated extends AuthBindingIdentity {
  provider: string;
  auth_method: string;
  stored: boolean;
}

/** Result returned by `auth/profile/delete` / `auth/logout`. */
export interface AuthCredentialsCleared extends AuthBindingIdentity {
  cleared: boolean;
}

/** OAuth credential persistence result returned by login completion calls. */
export interface AuthLoginReady extends AuthBindingIdentity {
  provider: string;
  expires_at: string | null;
  has_refresh_token: boolean;
  scopes: string[];
}

/** Auth status returned by `auth/status/get`. */
export interface AuthStatus extends AuthBindingIdentity {
  provider: string;
  auth_method: string;
  state:
    | 'valid'
    | 'expiring'
    | 'expired'
    | 'reauth_required'
    | 'refresh_failed'
    | 'unknown';
  expires_at?: string;
  last_refresh_at?: string;
  account_id?: string;
  has_refresh_token: boolean;
}

/** OAuth login-start payload returned by `auth/login/start`. */
export interface OAuthLoginStart {
  authorize_url: string;
  state: string;
  redirect_uri: string;
  provider: string;
}

/**
 * Typed wrapper over the meerkat RPC `auth/*` method family. Works
 * against any transport that speaks `(method, params) -> result`
 * (the stdio RPC binary, a REST-backed adapter, an in-process shim,
 * etc.).
 */
export class Auth {
  constructor(private readonly transport: AuthTransport) {}

  /** `auth/profile/create` — persist credentials for a managed-store binding. */
  async createProfile(params: {
    realm_id: string;
    binding_id: string;
    auth_method: 'api_key' | 'static_bearer' | string;
    secret: string;
  }): Promise<AuthProfileCreated> {
    return this.transport.request<typeof params, AuthProfileCreated>(
      'auth/profile/create',
      params,
    );
  }

  /** `auth/profile/list` — enumerate configured auth profiles for a realm. */
  async listProfiles(realm_id: string): Promise<AuthProfilesList> {
    return this.transport.request<{ realm_id: string }, AuthProfilesList>(
      'auth/profile/list',
      { realm_id },
    );
  }

  /** `auth/profile/get` — fetch the profile resolved by a binding. */
  async getProfile(realm_id: string, binding_id: string): Promise<AuthProfileDetail> {
    return this.transport.request<
      { realm_id: string; binding_id: string },
      AuthProfileDetail
    >(
      'auth/profile/get',
      { realm_id, binding_id },
    );
  }

  /** `auth/profile/delete` — clear credentials for a binding. */
  async deleteProfile(
    realm_id: string,
    binding_id: string,
  ): Promise<AuthCredentialsCleared> {
    return this.transport.request<
      { realm_id: string; binding_id: string },
      AuthCredentialsCleared
    >(
      'auth/profile/delete',
      { realm_id, binding_id },
    );
  }

  /** `auth/login/start` — begin an OAuth browser flow. */
  async loginStart(params: {
    provider: string;
    redirect_uri: string;
  }): Promise<OAuthLoginStart> {
    return this.transport.request<typeof params, OAuthLoginStart>(
      'auth/login/start',
      params,
    );
  }

  /** `auth/login/complete` — finalize an OAuth browser flow. */
  async loginComplete(params: {
    provider: string;
    code: string;
    state: string;
    redirect_uri: string;
    realm_id?: string;
    binding_id?: string;
  }): Promise<AuthLoginReady> {
    return this.transport.request<typeof params, AuthLoginReady>(
      'auth/login/complete',
      params,
    );
  }

  /** `auth/login/device_start` — device-code flow for keyboardless
   * hosts. */
  async loginDeviceStart(params: {
    provider: string;
  }): Promise<{
    device_code: string;
    user_code: string;
    verification_uri: string;
    verification_uri_complete?: string;
    expires_in: number;
    interval: number;
    provider: string;
  }> {
    return this.transport.request('auth/login/device_start', params);
  }

  /** `auth/login/device_complete` — single-poll completion leg for the
   * device-code flow. Call on the cadence returned by
   * `loginDeviceStart` (interval seconds). Returns `{ state: "pending"
   * | "slow_down" | "access_denied" | "expired" }` while the user has
   * not yet approved, and `{ state: "ready", binding_id, ... }` once
   * tokens are persisted. */
  async loginDeviceComplete(params: {
    provider: string;
    device_code: string;
    realm_id?: string;
    binding_id?: string;
  }): Promise<
    | { state: 'pending' | 'slow_down' | 'access_denied' | 'expired' }
    | ({ state: 'ready' } & AuthLoginReady)
  > {
    return this.transport.request('auth/login/device_complete', params);
  }

  /** `auth/login/provision_api_key` — Anthropic Console-OAuth to API-key provisioning. */
  async loginProvisionApiKey(params: {
    access_token: string;
    realm_id?: string;
    binding_id?: string;
  }): Promise<
    {
      provider: 'anthropic';
      auth_mode: 'oauth_to_api_key';
      has_api_key: boolean;
      scopes: string[];
    } & AuthBindingIdentity
  > {
    return this.transport.request('auth/login/provision_api_key', params);
  }

  /** `auth/status/get` — current status for a binding's stored credentials. */
  async status(
    realm_id: string,
    binding_id: string,
    profile_id?: string,
  ): Promise<AuthStatus> {
    const params: { realm_id: string; binding_id: string; profile_id?: string } = {
      realm_id,
      binding_id,
    };
    if (profile_id !== undefined) params.profile_id = profile_id;
    return this.transport.request<
      typeof params,
      AuthStatus
    >(
      'auth/status/get',
      params,
    );
  }

  /** `auth/logout` — revoke + delete credentials for a binding. */
  async logout(
    realm_id: string,
    binding_id: string,
    profile_id?: string,
  ): Promise<AuthCredentialsCleared> {
    const params: { realm_id: string; binding_id: string; profile_id?: string } = {
      realm_id,
      binding_id,
    };
    if (profile_id !== undefined) params.profile_id = profile_id;
    return this.transport.request<
      typeof params,
      AuthCredentialsCleared
    >(
      'auth/logout',
      params,
    );
  }
}

/**
 * Install a host-page external-auth resolver in the loaded WASM
 * runtime. The resolver is consulted per-session when the binding's
 * credential source is `external_resolver`.
 *
 * Plan §4d.wasm.1 + §4d.wasm.3: browser OAuth flows run in the host
 * page; the resolver hands Meerkat a structured `ExternalAuthLease` on
 * demand. Meerkat never touches the host's refresh token. Reject with
 * `ExternalAuthFailure` to preserve denial, refresh, or missing-credential
 * truth across the WASM boundary.
 */
export function registerExternalAuthResolver(
  wasm: { register_external_auth_resolver: (cb: unknown) => void },
  resolver: ExternalAuthResolver,
): void {
  const adapter = (
    connectionRef: ConnectionRef,
  ): Promise<ExternalAuthResolverResult> =>
    Promise.resolve(resolver(connectionRef));
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
