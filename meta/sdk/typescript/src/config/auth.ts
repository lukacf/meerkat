/**
 * Auth configuration for Meta runtime.
 */

// -- Google auth ----------------------------------------------------------

export interface GoogleAuthConfig {
  readonly clientId: string;
  readonly discoveryUrl: string;
  readonly audience: string | null;
  readonly leewaySeconds: number;
  toDict(): Record<string, unknown>;
}

export function google(
  clientId: string,
  options?: {
    discoveryUrl?: string;
    audience?: string;
    leewaySeconds?: number;
  },
): GoogleAuthConfig {
  const config = {
    clientId,
    discoveryUrl:
      options?.discoveryUrl ??
      "https://accounts.google.com/.well-known/openid-configuration",
    audience: options?.audience ?? null,
    leewaySeconds: options?.leewaySeconds ?? 60,
    toDict() {
      return googleAuthConfigToDict(config);
    },
  };
  return config;
}

export function googleAuthConfigToDict(
  config: GoogleAuthConfig,
): Record<string, unknown> {
  return {
    provider: "google",
    client_id: config.clientId,
    discovery_url: config.discoveryUrl,
    audience: config.audience ?? config.clientId,
    leeway_seconds: config.leewaySeconds,
  };
}

// -- JWT auth -------------------------------------------------------------

export interface JwtAuthConfig {
  readonly sharedSecret: string;
  readonly issuer: string | null;
  readonly audience: string | null;
  readonly leewaySeconds: number;
  toDict(): Record<string, unknown>;
}

export function jwt(
  sharedSecret: string,
  options?: {
    issuer?: string;
    audience?: string;
    leewaySeconds?: number;
  },
): JwtAuthConfig {
  const config = {
    sharedSecret,
    issuer: options?.issuer ?? null,
    audience: options?.audience ?? null,
    leewaySeconds: options?.leewaySeconds ?? 60,
    toDict() {
      return jwtAuthConfigToDict(config);
    },
  };
  return config;
}

export function jwtAuthConfigToDict(
  config: JwtAuthConfig,
): Record<string, unknown> {
  return {
    provider: "jwt",
    shared_secret: config.sharedSecret,
    issuer: config.issuer,
    audience: config.audience,
    leeway_seconds: config.leewaySeconds,
  };
}
