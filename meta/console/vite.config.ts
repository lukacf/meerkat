import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@console-core": path.resolve(__dirname, "../packages/console-core/src/index.ts"),
      "@console-components/styles": path.resolve(__dirname, "../packages/console-components/src/styles/index.ts"),
      "@console-components": path.resolve(__dirname, "../packages/console-components/src/index.ts"),
      "@modelcontextprotocol/ext-apps/app-bridge": path.resolve(__dirname, "node_modules/@modelcontextprotocol/ext-apps/dist/src/app-bridge.js"),
      "@modelcontextprotocol/core": path.resolve(__dirname, "node_modules/@modelcontextprotocol/core/dist/index.mjs"),
      "@modelcontextprotocol/client": path.resolve(__dirname, "node_modules/@modelcontextprotocol/client/dist/index.mjs"),
      clsx: path.resolve(__dirname, "node_modules/clsx"),
      "react-markdown": path.resolve(__dirname, "node_modules/react-markdown"),
      "remark-gfm": path.resolve(__dirname, "node_modules/remark-gfm"),
    },
  },
  optimizeDeps: {
    include: ["clsx", "react", "react-dom"],
  },
  server: {
    port: 5199,
    proxy: {
      "/console/experience": "http://127.0.0.1:63220",
      "/console/events": "http://127.0.0.1:63220",
      "/console/identity": "http://127.0.0.1:63220",
      "/console/modules": "http://127.0.0.1:63220",
      "/console/rpc": "http://127.0.0.1:63220",
      "/interactions": "http://127.0.0.1:63220",
      "/healthz": "http://127.0.0.1:63220",
    },
  },
});
