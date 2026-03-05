import { defineConfig } from "vite";

function envPreferringRkat(rkatKey: string, providerKey: string): string {
  const fromRkat = process.env[rkatKey]?.trim();
  if (fromRkat) return fromRkat;
  return process.env[providerKey]?.trim() ?? "";
}

export default defineConfig({
  define: {
    __ANTHROPIC_API_KEY__: JSON.stringify(envPreferringRkat("RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY")),
    __OPENAI_API_KEY__: JSON.stringify(envPreferringRkat("RKAT_OPENAI_API_KEY", "OPENAI_API_KEY")),
    __GEMINI_API_KEY__: JSON.stringify(envPreferringRkat("RKAT_GEMINI_API_KEY", "GEMINI_API_KEY")),
  },
  server: {
    host: "127.0.0.1",
    port: 4174,
  },
});
