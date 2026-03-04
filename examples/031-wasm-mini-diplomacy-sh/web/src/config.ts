// ═══════════════════════════════════════════════════════════
// Provider / Model Configuration
// ═══════════════════════════════════════════════════════════

export type Provider = "anthropic" | "openai" | "gemini";

export const MODEL_PROVIDER: Record<string, Provider> = {
  "claude-sonnet-4-5": "anthropic",
  "claude-opus-4-6": "anthropic",
  "gpt-5.2": "openai",
  "gemini-3-flash-preview": "gemini",
  "gemini-3-pro-preview": "gemini",
};

export const ALL_MODELS = Object.keys(MODEL_PROVIDER);

const $ = <T extends Element>(id: string) => document.getElementById(id) as unknown as T;

// ── Server mode (proxy) detection ──────────────────────────

const PARAMS = new URLSearchParams(window.location.search);

export interface ProxyConfig {
  proxyUrl: string;
  models: { france?: string; prussia?: string; russia?: string; narrator?: string };
}

export function getProxyConfig(): ProxyConfig | null {
  const proxy = PARAMS.get("proxy");
  if (!proxy) return null;
  return {
    proxyUrl: proxy.replace(/\/+$/, ""),
    models: {
      france: PARAMS.get("france") ?? undefined,
      prussia: PARAMS.get("prussia") ?? undefined,
      russia: PARAMS.get("russia") ?? undefined,
      narrator: PARAMS.get("narrator") ?? undefined,
    },
  };
}

const proxyConfig = getProxyConfig();

export function isServerMode(): boolean {
  return proxyConfig !== null;
}

export function getServerProxyConfig(): ProxyConfig | null {
  return proxyConfig;
}

// ── API keys ───────────────────────────────────────────────

const DUMMY_KEY = "proxy-provided";

export function getApiKeys(): Record<Provider, string> {
  if (proxyConfig) return { anthropic: DUMMY_KEY, openai: DUMMY_KEY, gemini: DUMMY_KEY };
  return {
    anthropic: $<HTMLInputElement>("keyAnthropic").value.trim(),
    openai: $<HTMLInputElement>("keyOpenai").value.trim(),
    gemini: $<HTMLInputElement>("keyGemini").value.trim(),
  };
}

export function availableModels(): string[] {
  if (proxyConfig) return ALL_MODELS;
  const keys = getApiKeys();
  return ALL_MODELS.filter(m => keys[MODEL_PROVIDER[m]]);
}

export function updateModelDropdowns(): void {
  const models = availableModels();
  for (const id of ["modelFrance", "modelPrussia", "modelRussia", "modelNarrator"]) {
    const sel = $<HTMLSelectElement>(id);
    const current = sel.value;
    sel.innerHTML = models.map(m => `<option value="${m}"${m === current ? " selected" : ""}>${m}</option>`).join("");
    if (!models.includes(current) && models.length > 0) sel.value = models[0];
  }
}

export function initApiKeyInputs(): void {
  if (proxyConfig) {
    // Server mode: fill dummy keys, populate dropdowns, apply URL model overrides
    for (const inputId of ["keyAnthropic", "keyOpenai", "keyGemini"]) {
      $<HTMLInputElement>(inputId).value = DUMMY_KEY;
    }
    updateModelDropdowns();
    const m = proxyConfig.models;
    if (m.france) $<HTMLSelectElement>("modelFrance").value = m.france;
    if (m.prussia) $<HTMLSelectElement>("modelPrussia").value = m.prussia;
    if (m.russia) $<HTMLSelectElement>("modelRussia").value = m.russia;
    if (m.narrator) $<HTMLSelectElement>("modelNarrator").value = m.narrator;
    return;
  }

  for (const [provider, envKey, inputId] of [
    ["anthropic", "VITE_ANTHROPIC_API_KEY", "keyAnthropic"],
    ["openai", "VITE_OPENAI_API_KEY", "keyOpenai"],
    ["gemini", "VITE_GEMINI_API_KEY", "keyGemini"],
  ] as const) {
    const input = $<HTMLInputElement>(inputId);
    const stored = sessionStorage.getItem(`api_key_${provider}`);
    const envVal = (import.meta as any).env?.[envKey] ?? "";
    input.value = stored || envVal;
    input.addEventListener("change", () => {
      sessionStorage.setItem(`api_key_${provider}`, input.value);
      updateModelDropdowns();
    });
  }
  updateModelDropdowns();
}
