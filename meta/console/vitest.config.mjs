import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const consoleRoot = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(consoleRoot, "..");
const require = createRequire(import.meta.url);
const consoleDependencies = [
  "@modelcontextprotocol/ext-apps/app-bridge",
  "@modelcontextprotocol/core",
  "@modelcontextprotocol/client",
  "@testing-library/react",
  "clsx",
  "d3-force",
  "react-markdown",
  "remark-gfm",
  "react",
  "react/jsx-runtime",
  "react/jsx-dev-runtime",
  "react-dom",
  "react-dom/client",
  "react-dom/server",
  "react-dom/test-utils",
];

export default defineConfig({
  root: repoRoot,
  esbuild: { jsx: "automatic" },
  server: {
    fs: { allow: [repoRoot, consoleRoot] },
  },
  resolve: {
    alias: [
      {
        find: /^@console-core$/,
        replacement: path.resolve(consoleRoot, "../packages/console-core/src/index.ts"),
      },
      {
        find: /^@console-core\/runtime-types$/,
        replacement: path.resolve(
          consoleRoot,
          "../packages/console-core/src/runtime-types.ts",
        ),
      },
      {
        find: /^@console-components\/styles$/,
        replacement: path.resolve(
          consoleRoot,
          "../packages/console-components/src/styles/index.ts",
        ),
      },
      {
        find: /^@console-components$/,
        replacement: path.resolve(
          consoleRoot,
          "../packages/console-components/src/index.ts",
        ),
      },
      ...consoleDependencies.map((dependency) => ({
        find: new RegExp(`^${dependency.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
        replacement: require.resolve(dependency),
      })),
    ],
  },
  test: {
    environment: "jsdom",
    globals: true,
    include: [
      "console/src/lib/custom-panels.test.tsx",
      "console/src/mcp-apps/host.test.tsx",
      "console/src/mcp-apps/native-host.test.ts",
      "console/src/mcp-apps/projection.test.ts",
      "console/src/mcp-apps/stock-host.test.tsx",
      "packages/console-components/src/custom-panels.test.tsx",
      "packages/console-components/src/copy-button.test.tsx",
      "packages/console-core/src/send-attempt.test.ts",
      "packages/console-core/src/context-record.test.ts",
      "console/src/lib/send-attempt-storage.test.ts",
      "console/src/lib/send-queue-integration.test.tsx",
      "console/src/lib/pending-row-language.test.tsx",
      "console/src/lib/markdown-url-policy-integration.test.tsx",
      "console/src/panels/GatingInboxPanel.test.tsx",
      "console/src/panels/AccessPanel.security.test.tsx",
      "console/src/panels/PendingStack.test.tsx",
      "console/src/panels/transcript-window.test.ts",
      "console/src/panels/Sidebar.activity.test.tsx",
      "console/src/panels/OperatorPanel.test.tsx",
      "console/src/lib/operator-panel-integration.test.tsx",
      "packages/console-components/src/conversation/conversation-message-group.test.tsx",
      "packages/console-components/src/conversation/context-selection.test.tsx",
      "packages/console-components/src/conversation/approval-card.test.tsx",
      "packages/console-components/src/conversation/presentation-policy.test.tsx",
      "packages/console-components/src/conversation/row-state.test.tsx",
      "packages/console-components/src/conversation/member-kickoff.test.tsx",
      "packages/console-core/src/member-kickoff.test.ts",
      "packages/console-components/src/conversation/transport-status.test.tsx",
      "packages/console-components/src/conversation/scroll-controller.test.tsx",
      "packages/console-components/src/conversation/conversation-pane.test.tsx",
      "console/src/lib/voice-session.test.ts",
      "console/src/lib/use-voice-readiness.test.tsx",
      "console/src/lib/use-voice-controller.test.tsx",
      "console/src/panels/VoiceBar.test.tsx",
      "console/src/perf/typing-lag.bench.test.tsx",
      "console/src/lib/panel-refresh.test.tsx",
      "console/src/lib/identity-log.test.ts",
      "console/src/lib/history-paging-notices.test.tsx",
      "packages/console-core/src/dock.test.ts",
      "packages/console-components/src/conversation/council-card.test.tsx",
      "packages/console-components/src/conversation/conversation-markdown.test.tsx",
      "packages/console-components/src/conversation/assistant-render-identity.test.tsx",
      "packages/console-components/src/conversation/tool-render-identity.test.tsx",
      "packages/console-components/src/conversation/user-render-identity.test.tsx",
      "packages/console-components/src/conversation/conversation-message-view.test.tsx",
      "packages/console-components/src/conversation/conversation-transcript.test.tsx",
      "packages/console-components/src/conversation/work-graph-card.test.tsx",
      "packages/console-components/src/topology/data.test.ts",
      "packages/console-components/src/dock/browser-dock-target-host.test.tsx",
      "packages/console-components/src/topology/dense-graph-map.test.ts",
      "packages/console-components/src/topology/connection-picker.test.tsx",
      "packages/console-components/src/topology/topology-panel.test.tsx",
    ],
    restoreMocks: true,
    setupFiles: [path.resolve(consoleRoot, "vitest.setup.ts")],
  },
});
