import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

/** The gateway `npm run dev` forwards the console's backend routes to. */
const GATEWAY = "http://127.0.0.1:63220";

/**
 * An alias for exactly `specifier`, as vitest.config.mjs writes them. A
 * plain string alias also rewrites `specifier/<sub-path>`, which works only
 * for a package directory: under an alias to one file, the MCP client's and
 * core's imports of their own sub-paths ("@modelcontextprotocol/client/_shims",
 * "@modelcontextprotocol/core/internal") pointed below that file, and the
 * dependency pre-bundle the dev server runs at startup failed.
 */
function exactly(specifier: string, replacement: string) {
  return { find: new RegExp(`^${specifier.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`), replacement };
}

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: [
      exactly("@console-core", path.resolve(__dirname, "../packages/console-core/src/index.ts")),
      exactly("@console-components/styles", path.resolve(__dirname, "../packages/console-components/src/styles/index.ts")),
      exactly("@console-components", path.resolve(__dirname, "../packages/console-components/src/index.ts")),
      exactly("@modelcontextprotocol/ext-apps/app-bridge", path.resolve(__dirname, "node_modules/@modelcontextprotocol/ext-apps/dist/src/app-bridge.js")),
      exactly("@modelcontextprotocol/core", path.resolve(__dirname, "node_modules/@modelcontextprotocol/core/dist/index.mjs")),
      exactly("@modelcontextprotocol/client", path.resolve(__dirname, "node_modules/@modelcontextprotocol/client/dist/index.mjs")),
      // Package directories: their sub-paths resolve inside them.
      { find: "clsx", replacement: path.resolve(__dirname, "node_modules/clsx") },
      // The kit's force layout (packages/console-components/src/viz/layout).
      { find: "d3-force", replacement: path.resolve(__dirname, "node_modules/d3-force") },
      { find: "react-markdown", replacement: path.resolve(__dirname, "node_modules/react-markdown") },
      { find: "remark-gfm", replacement: path.resolve(__dirname, "node_modules/remark-gfm") },
    ],
  },
  optimizeDeps: {
    include: ["clsx", "d3-force", "react", "react-dom"],
  },
  server: {
    port: 5199,
    // The backend routes the console calls (the REST, RPC and blob paths of
    // packages/console-core/src/contract.ts; dev-server.test.cjs checks
    // them). Vite forwards every request whose path starts with a key.
    proxy: {
      "/console/events": GATEWAY,
      "/console/experience": GATEWAY,
      "/console/identities": GATEWAY,
      "/console/identity": GATEWAY,
      "/console/mcp-apps": GATEWAY,
      "/console/modules": GATEWAY,
      "/console/rpc": GATEWAY,
      "/console/send": GATEWAY,
      "/console/timeline": GATEWAY,
      "/blobs": GATEWAY,
      "/healthz": GATEWAY,
      "/interactions": GATEWAY,
    },
  },
});
