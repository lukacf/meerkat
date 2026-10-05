import { defineConfig } from "vite";

export default defineConfig({
  // Relative asset URLs, so the built app works when served from a sub-path
  // (e.g. /demos/office/) as well as from a site root.
  base: "./",
  server: {
    host: "127.0.0.1",
    port: 4174,
  },
});
