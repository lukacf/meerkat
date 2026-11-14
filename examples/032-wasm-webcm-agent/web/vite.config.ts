import { defineConfig } from "vite";

// The key fields are pre-filled from the environment only by the local dev
// server (`vite`, bound to 127.0.0.1). A build never contains a key: its
// output is static files that get copied and hosted, and anything inlined
// there is readable by whoever can fetch them.
const devKey = (command: string, name: string): string =>
  JSON.stringify(command === "serve" ? (process.env[name] ?? "") : "");

export default defineConfig(({ command }) => ({
  define: {
    __ANTHROPIC_API_KEY__: devKey(command, "ANTHROPIC_API_KEY"),
    __OPENAI_API_KEY__: devKey(command, "OPENAI_API_KEY"),
    __GEMINI_API_KEY__: devKey(command, "GEMINI_API_KEY"),
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
}));
