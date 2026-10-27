import { fileURLToPath } from "node:url";

// Shared by the production build and its dependency-graph check.
export function forgeBuildOptions() {
  return {
    entryPoints: [fileURLToPath(new URL("app.jsx", import.meta.url))],
    outfile: fileURLToPath(new URL("dist/app.js", import.meta.url)),
    bundle: true,
    minify: true,
    sourcemap: true,
    jsx: "automatic",
    target: ["es2022"],
    format: "esm",
    nodePaths: [fileURLToPath(new URL("node_modules/", import.meta.url))],
    alias: {
      // Shared Console source must use this application's React instance.
      react: fileURLToPath(new URL("node_modules/react", import.meta.url)),
      "react-dom": fileURLToPath(new URL("node_modules/react-dom", import.meta.url)),
      "@console-core": fileURLToPath(new URL("../../packages/console-core/src", import.meta.url)),
    },
    define: { "process.env.NODE_ENV": '"production"' },
    // fonts/fonts.css names Forge's own woff2 fonts. Each lands in
    // dist/fonts beside app.css, which the Forge server serves, so the page
    // fetches only the subsets it shows and nothing from another origin.
    loader: { ".woff2": "file" },
    assetNames: "fonts/[name]-[hash]",
    logLevel: "info",
  };
}
