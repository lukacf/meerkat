/**
 * Memory backend configuration for Meta runtime.
 *
 * The operational memory ledger (`meta/memory/*`) is persisted by the
 * gateway as local JSON under `persistent_state`, with an optional HTTP
 * health-check gate. `localJson()` configures that backend, the only one the
 * gateway accepts.
 */

export interface LocalJsonMemoryConfig {
  readonly healthCheckEndpoint: string | null;
  toDict(): Record<string, unknown>;
}

export function localJson(options?: {
  healthCheckEndpoint?: string;
}): LocalJsonMemoryConfig {
  const config = {
    healthCheckEndpoint: options?.healthCheckEndpoint ?? null,
    toDict() {
      const result: Record<string, unknown> = { backend: "local_json" };
      if (config.healthCheckEndpoint !== null) {
        result.health_check_endpoint = config.healthCheckEndpoint;
      }
      return result;
    },
  };
  return config;
}
