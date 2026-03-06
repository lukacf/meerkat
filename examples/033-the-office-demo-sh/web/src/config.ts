// ═══════════════════════════════════════════════════════════
// Provider / Model Configuration
// ═══════════════════════════════════════════════════════════

export type Provider = "anthropic" | "openai" | "gemini";

export const MODEL_PROVIDER: Record<string, Provider> = {
  "claude-sonnet-4-5": "anthropic",
  "claude-sonnet-4-6": "anthropic",
  "claude-opus-4-6": "anthropic",
  "gpt-5.2": "openai",
  "gemini-3-flash-preview": "gemini",
  "gemini-3.1-pro-preview": "gemini",
};

export const ALL_MODELS = Object.keys(MODEL_PROVIDER);

const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;

// ── Server mode (proxy) detection ──

const PARAMS = new URLSearchParams(window.location.search);

export interface ProxyConfig {
  proxyUrl: string;
  model?: string;
}

export function getProxyConfig(): ProxyConfig | null {
  const proxy = PARAMS.get("proxy");
  if (!proxy) return null;
  return {
    proxyUrl: proxy.replace(/\/+$/, ""),
    model: PARAMS.get("model") ?? undefined,
  };
}

const proxyConfig = getProxyConfig();

export function isServerMode(): boolean {
  return proxyConfig !== null;
}

export function getServerProxyConfig(): ProxyConfig | null {
  return proxyConfig;
}

// ── API keys ──

const DUMMY_KEY = "proxy-provided";

export function getApiKeys(): Record<Provider, string> {
  if (proxyConfig) return { anthropic: DUMMY_KEY, openai: DUMMY_KEY, gemini: DUMMY_KEY };
  return {
    anthropic: $<HTMLInputElement>("keyAnthropic").value.trim(),
    openai: $<HTMLInputElement>("keyOpenai").value.trim(),
    gemini: $<HTMLInputElement>("keyGemini").value.trim(),
  };
}

export function getSelectedModel(): string {
  if (proxyConfig?.model) return proxyConfig.model;
  return $<HTMLSelectElement>("modelSelect").value || "claude-sonnet-4-6";
}

export function availableModels(): string[] {
  if (proxyConfig) return ALL_MODELS;
  const keys = getApiKeys();
  return ALL_MODELS.filter(m => keys[MODEL_PROVIDER[m]]);
}

export function updateModelDropdown(): void {
  const models = availableModels();
  const sel = $<HTMLSelectElement>("modelSelect");
  if (!sel) return;
  const current = sel.value;
  sel.innerHTML = models.map(m => `<option value="${m}"${m === current ? " selected" : ""}>${m}</option>`).join("");
  if (!models.includes(current) && models.length > 0) sel.value = models[0];
}

export function initApiKeyInputs(): void {
  if (proxyConfig) {
    for (const inputId of ["keyAnthropic", "keyOpenai", "keyGemini"]) {
      const el = document.getElementById(inputId) as HTMLInputElement | null;
      if (el) el.value = DUMMY_KEY;
    }
    updateModelDropdown();
    return;
  }

  // Migration cleanup: remove legacy browser-stored keys from earlier builds.
  for (const provider of ["anthropic", "openai", "gemini"] as const) {
    sessionStorage.removeItem(`api_key_${provider}`);
  }

  for (const [provider, inputId] of [
    ["anthropic", "keyAnthropic"],
    ["openai", "keyOpenai"],
    ["gemini", "keyGemini"],
  ] as const) {
    const input = $<HTMLInputElement>(inputId);
    if (!input) continue;
    const envVal = envApiKey(provider);
    input.value = envVal;
    input.addEventListener("change", () => {
      updateModelDropdown();
    });
  }
  updateModelDropdown();
}

export function setApiKeys(keys: Partial<Record<Provider, string>>): void {
  const mapping: Record<Provider, string> = {
    anthropic: "keyAnthropic",
    openai: "keyOpenai",
    gemini: "keyGemini",
  };
  for (const provider of ["anthropic", "openai", "gemini"] as const) {
    const value = keys[provider]?.trim() ?? "";
    const input = document.getElementById(mapping[provider]) as HTMLInputElement | null;
    if (input) input.value = value;
  }
  updateModelDropdown();
}

export function hasAnyApiKey(): boolean {
  const keys = getApiKeys();
  return !!(keys.anthropic || keys.openai || keys.gemini);
}

function envApiKey(provider: Provider): string {
  const env = (import.meta as any).env ?? {};
  if (provider === "anthropic") return env.VITE_RKAT_ANTHROPIC_API_KEY ?? env.VITE_ANTHROPIC_API_KEY ?? "";
  if (provider === "openai") return env.VITE_RKAT_OPENAI_API_KEY ?? env.VITE_OPENAI_API_KEY ?? "";
  return env.VITE_RKAT_GEMINI_API_KEY ?? env.VITE_GEMINI_API_KEY ?? "";
}
