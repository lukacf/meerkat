import { defineConfig } from "vite";

export default defineConfig({
  define: {
    __ANTHROPIC_API_KEY__: JSON.stringify(process.env.ANTHROPIC_API_KEY ?? ""),
    __OPENAI_API_KEY__: JSON.stringify(process.env.OPENAI_API_KEY ?? ""),
    __GEMINI_API_KEY__: JSON.stringify(process.env.GEMINI_API_KEY ?? ""),
  },
  server: {
    host: "127.0.0.1",
    port: 4032,
    headers: {
      "Cross-Origin-Opener-Policy": "same-origin",
      "Cross-Origin-Embedder-Policy": "require-corp",
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 4032,
  },
  optimizeDeps: {
    exclude: ["webcm"],
  },
});
