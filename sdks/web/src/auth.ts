/**
 * Web SDK auth bindings — plan §4c.10 + §4d.wasm.3.
 *
 * This module provides:
 *   - `Auth` — TypeScript wrapper for the generated `auth/*` RPC method
 *     family. Surfaces targeting the JSON-RPC stdio server or the REST API
 *     over fetch should instantiate this class with the appropriate
 *     transport.
 *   - `registerExternalAuthResolver` — wraps the WASM-bundled
 *     `register_external_auth_resolver` binding so a browser host page
 *     can install an OAuth-backed resolver callback that hands Meerkat a
 *     typed lease envelope per structural auth binding reference.
 *   - `withAuthBinding` — convenience helper that wires an existing
 *     session config with an auth binding reference for `createSession`.
 *
 * The WASM runtime's session-creation path (plan §4d.wasm.2) takes
 * credentials either from bootstrap-time realm config (populated via
 * `initRuntimeFromConfig`) or from a host-registered external-auth
 * resolver. Per-session `apiKey` is deleted.
 */

/* eslint-disable @typescript-eslint/no-explicit-any */

import {
  AUTH_RPC_METHODS,
  parseCreateProfileParams,
  parseDeviceCompleteParams,
  parseDeviceStartParams,
  parseLoginCompleteParams,
  parseLoginStartParams,
  parseProvisionApiKeyParams,
  parseWireAuthProfileCleared,
  parseWireAuthProfileCreated,
  parseWireAuthProfileDetail,
  parseWireAuthProfilesList,
  parseWireAuthStatusDetail,
  parseWireDeviceCompleteResult,
  parseWireDeviceStart,
  parseWireLoginReady,
  parseWireLoginStart,
  parseWireProvisionApiKeyResult,
} from './generated/auth.js';
import type {
  AuthRpcMethod,
  CreateProfileParams,
  DeviceCompleteParams,
  DeviceStartParams,
  LoginCompleteParams,
  LoginStartParams,
  ProvisionApiKeyParams,
  WireAuthMethod,
  WireAuthProfile,
  WireAuthProfileCleared,
  WireAuthProfileCreated,
  WireAuthProfileDetail,
  WireAuthProfilesList,
  WireAuthProvider,
  WireAuthStatusDetail,
  WireBackendKind,
  WireBindingIdentity,
  WireDeviceCompleteResult,
  WireDeviceStart,
  WireLoginReady,
  WireLoginStart,
  WireProvisionApiKeyResult,
} from './generated/auth.js';
import type { AuthBindingRef, SessionConfig } from './types.js';

export type {
  WireAuthMethod,
  WireAuthProvider,
  WireBackendKind,
  WireCredentialSourceKind,
  WireAuthStatusState,
} from './generated/auth.js';

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
  | { kind: 'unsupported_combination'; backend: WireBackendKind; auth: WireAuthMethod }
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
 * structural auth binding reference, and returns a typed lease envelope. Reject
 * the returned promise with `ExternalAuthFailure` to preserve stable failure
 * truth. */
export type ExternalAuthResolver = (
  authBinding: AuthBindingRef,
) => ExternalAuthResolverResult | Promise<ExternalAuthResolverResult>;

/** JSON-RPC-style transport used by the `Auth` class. Minimal: just
 * async request/response of (method, params) -> result. */
export interface AuthTransport {
  request<P = unknown, R = unknown>(method: AuthRpcMethod, params?: P): Promise<R>;
}

/** Wire projection of a configured auth profile. */
export type AuthProfile = WireAuthProfile;
/** Binding-scoped identity returned by auth write/status calls. */
export type AuthBindingIdentity = WireBindingIdentity & { profile_id: string };
/** Auth profile list result returned by `auth/profile/list`. */
export type AuthProfilesList = WireAuthProfilesList;
/** Result returned by `auth/profile/get`. */
export type AuthProfileDetail = WireAuthProfileDetail;
/** Result returned by profile creation. */
export type AuthProfileCreated = WireAuthProfileCreated;
/** Result returned by profile deletion / logout. */
export type AuthCredentialsCleared = WireAuthProfileCleared;
/** OAuth credential persistence result returned by login completion calls. */
export type AuthLoginReady = WireLoginReady;
/** Binding credential status. */
export type AuthStatus = WireAuthStatusDetail;
/** OAuth login-start payload. */
export type OAuthLoginStart = WireLoginStart;
/** Device-code login start payload returned by `auth/login/device_start`. */
export type AuthDeviceStart = WireDeviceStart;
/** Device-code login poll result returned by `auth/login/device_complete`. */
export type AuthDeviceCompleteResult = WireDeviceCompleteResult;
/** Anthropic Console-OAuth to API-key provisioning result. */
export type AuthProvisionApiKeyResult = WireProvisionApiKeyResult;

/**
 * Typed wrapper over the meerkat RPC `auth/*` method family. Works
 * against any transport that speaks `(method, params) -> result`
 * (the stdio RPC binary, a REST-backed adapter, an in-process shim,
 * etc.).
 */
export class Auth {
  constructor(private readonly transport: AuthTransport) {}

  /** Persist credentials for a managed-store binding. */
  async createProfile(params: CreateProfileParams): Promise<AuthProfileCreated> {
    const result = await this.transport.request<CreateProfileParams, unknown>(
      AUTH_RPC_METHODS.profileCreate,
      parseCreateProfileParams(params),
    );
    return parseWireAuthProfileCreated(result);
  }

  /** Enumerate configured auth profiles for a realm. */
  async listProfiles(realm_id: string): Promise<AuthProfilesList> {
    const result = await this.transport.request<{ realm_id: string }, unknown>(
      AUTH_RPC_METHODS.profileList,
      { realm_id },
    );
    return parseWireAuthProfilesList(result);
  }

  /** Fetch the profile resolved by a binding. */
  async getProfile(realm_id: string, binding_id: string): Promise<AuthProfileDetail> {
    const result = await this.transport.request<
      { realm_id: string; binding_id: string },
      unknown
    >(
      AUTH_RPC_METHODS.profileGet,
      { realm_id, binding_id },
    );
    return parseWireAuthProfileDetail(result);
  }

  /** Clear credentials for a binding. */
  async deleteProfile(
    realm_id: string,
    binding_id: string,
  ): Promise<AuthCredentialsCleared> {
    const result = await this.transport.request<
      { realm_id: string; binding_id: string },
      unknown
    >(
      AUTH_RPC_METHODS.profileDelete,
      { realm_id, binding_id },
    );
    return parseWireAuthProfileCleared(result);
  }

  /** Begin an OAuth browser flow. */
  async loginStart(params: LoginStartParams): Promise<OAuthLoginStart> {
    const result = await this.transport.request<LoginStartParams, unknown>(
      AUTH_RPC_METHODS.loginStart,
      parseLoginStartParams(params),
    );
    return parseWireLoginStart(result);
  }

  /** Finalize an OAuth browser flow. */
  async loginComplete(params: LoginCompleteParams): Promise<AuthLoginReady> {
    const result = await this.transport.request<LoginCompleteParams, unknown>(
      AUTH_RPC_METHODS.loginComplete,
      parseLoginCompleteParams(params),
    );
    return parseWireLoginReady(result);
  }

  /** Start a device-code flow for keyboardless hosts. */
  async loginDeviceStart(params: DeviceStartParams): Promise<AuthDeviceStart> {
    const result = await this.transport.request<DeviceStartParams, unknown>(
      AUTH_RPC_METHODS.loginDeviceStart,
      parseDeviceStartParams(params),
    );
    return parseWireDeviceStart(result);
  }

  /** Single-poll completion leg for the device-code flow. */
  async loginDeviceComplete(
    params: DeviceCompleteParams,
  ): Promise<AuthDeviceCompleteResult> {
    const result = await this.transport.request<DeviceCompleteParams, unknown>(
      AUTH_RPC_METHODS.loginDeviceComplete,
      parseDeviceCompleteParams(params),
    );
    return parseWireDeviceCompleteResult(result);
  }

  /** Anthropic Console-OAuth to API-key provisioning. */
  async loginProvisionApiKey(
    params: ProvisionApiKeyParams,
  ): Promise<AuthProvisionApiKeyResult> {
    const result = await this.transport.request<ProvisionApiKeyParams, unknown>(
      AUTH_RPC_METHODS.loginProvisionApiKey,
      parseProvisionApiKeyParams(params),
    );
    return parseWireProvisionApiKeyResult(result);
  }

  /** Current status for a binding's stored credentials. */
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
    const result = await this.transport.request<
      typeof params,
      unknown
    >(
      AUTH_RPC_METHODS.statusGet,
      params,
    );
    return parseWireAuthStatusDetail(result);
  }

  /** Revoke and delete credentials for a binding. */
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
    const result = await this.transport.request<
      typeof params,
      unknown
    >(
      AUTH_RPC_METHODS.logout,
      params,
    );
    return parseWireAuthProfileCleared(result);
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
    authBinding: AuthBindingRef,
  ): Promise<ExternalAuthResolverResult> =>
    Promise.resolve(resolver(authBinding));
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

/** Return a session config with an explicit auth auth binding. */
export function withAuthBinding<T extends SessionConfig>(
  authBinding: AuthBindingRef,
  config: T,
): T & { authBinding: AuthBindingRef } {
  return {
    ...config,
    authBinding,
  };
}
