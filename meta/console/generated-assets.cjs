const DIST_GENERATED_FILES = [
  "console-app.css",
  "console-app.js",
  "index.css",
  "index.cjs",
  "index.html",
];

// Bundle outputs the build copies from console/dist into the gateway crate.
const EMBEDDED_SHARED_FILES = [
  "console-app.css",
  "console-app.js",
  "index.html",
];

// Console sources the build copies into the gateway crate unchanged: the
// MCP Apps sandbox proxy and the page script it inlines. The gateways embed
// them so `--write-mcp-apps-sandbox <dir>` can write the proxy that matches
// their Console. `source` is relative to console/, `embedded` to
// crates/meerkat-meta/console-dist/.
const EMBEDDED_SOURCE_FILES = [
  { source: "mcp-apps-sandbox.cjs", embedded: "mcp-apps-sandbox.cjs" },
  { source: "src/mcp-apps/sandbox.js", embedded: "mcp-apps-sandbox.js" },
];

const EMBEDDED_GENERATED_FILES = [
  ...EMBEDDED_SHARED_FILES,
  ...EMBEDDED_SOURCE_FILES.map(({ embedded }) => embedded),
];

module.exports = {
  DIST_GENERATED_FILES,
  EMBEDDED_GENERATED_FILES,
  EMBEDDED_SHARED_FILES,
  EMBEDDED_SOURCE_FILES,
};
