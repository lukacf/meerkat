import { defineConfig } from "vite";

// The page pre-fills its provider keys from VITE_-prefixed variables
// (src/config.ts). Vite writes every variable with the exposed prefix into
// the bundle, so that prefix is exposed only to the local dev server (`vite`,
// bound to 127.0.0.1). A build exposes a prefix nobody sets by accident: its
// output is static files that get copied and hosted, and anything inlined
// there is readable by whoever can fetch them.
export default defineConfig(({ command }) => ({
  envPrefix: command === "serve" ? "VITE_" : "VITE_BUILD_PUBLIC_",
  // Relative asset URLs, so the built app works when served from a sub-path
  // (e.g. /demos/office/) as well as from a site root.
  base: "./",
  server: {
    host: "127.0.0.1",
    port: 4174,
  },
}));
