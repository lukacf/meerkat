import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const THIS_DIR = path.dirname(fileURLToPath(import.meta.url));
const COMPONENTS_ROOT = THIS_DIR;
const CORE_ROOT = path.resolve(THIS_DIR, "../../console-core/src");
const CORE_PACKAGE_ROOT = path.resolve(THIS_DIR, "../../console-core");
const COMPONENTS_PACKAGE_ROOT = path.resolve(THIS_DIR, "..");
const FIRST_PARTY_CONSUMERS = [
  path.resolve(THIS_DIR, "../../../desktop/renderer/src/app/App.tsx"),
  path.resolve(THIS_DIR, "../../../desktop/renderer/src/stories/console-components.stories.tsx"),
];

const BLOCKED_IMPORT_PATTERNS = [
  /from\s+["']@\//,
  /from\s+["'][^"']*desktop\/renderer/,
  /from\s+["']electron["']/,
  /from\s+["']zustand["']/,
];

const BLOCKED_CONSUMER_IMPORT_PATTERNS = [
  /from\s+["']@console-components\//,
];

const BLOCKED_CSS_PATTERNS = [
  /\[data-theme="light"\]/i,
  /--thread-[a-z0-9-]+/i,
  /--workspace-member-[a-z0-9-]+/i,
  /--titlebar-height/i,
  /--window-fit-scale/i,
  /--sidebar-(?:width-current|safe-top|pad-left|pad-right|motion-duration|motion-ease)/i,
  /--watch-rail-width-current/i,
  /--codex-[a-z0-9-]+/i,
  /--mk-[a-z0-9-]+/i,
  /var\(--font-mono/i,
  /var\(--text-sm/i,
  /var\(--text-xs/i,
];

function collectSourceFiles(root: string): string[] {
  const entries = fs.readdirSync(root, { withFileTypes: true });
  const files: string[] = [];

  for (const entry of entries) {
    const absolutePath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectSourceFiles(absolutePath));
      continue;
    }
    if (!/\.(ts|tsx)$/.test(entry.name) || /\.test\.(ts|tsx)$/.test(entry.name)) {
      continue;
    }
    files.push(absolutePath);
  }

  return files;
}

function collectCssFiles(root: string): string[] {
  const entries = fs.readdirSync(root, { withFileTypes: true });
  const files: string[] = [];

  for (const entry of entries) {
    const absolutePath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      files.push(...collectCssFiles(absolutePath));
      continue;
    }
    if (!entry.name.endsWith(".css")) {
      continue;
    }
    files.push(absolutePath);
  }

  return files;
}

describe("shared console portability", () => {
  test("shared console files stay vendorable and do not import app-only modules", () => {
    const sharedRoots = [
      COMPONENTS_ROOT,
      CORE_ROOT,
    ];

    const violations: string[] = [];

    for (const root of sharedRoots) {
      for (const filePath of collectSourceFiles(root)) {
        const source = fs.readFileSync(filePath, "utf8");
        for (const pattern of BLOCKED_IMPORT_PATTERNS) {
          if (pattern.test(source)) {
            violations.push(`${path.relative(COMPONENTS_ROOT, filePath)} matched ${pattern}`);
          }
        }
      }
    }

    expect(violations).toEqual([]);
  });

  test("shared console styles rely on documented cc tokens rather than host app variables", () => {
    const stylesRoot = path.join(COMPONENTS_ROOT, "styles");
    const violations: string[] = [];

    for (const filePath of collectCssFiles(stylesRoot)) {
      const source = fs.readFileSync(filePath, "utf8");
      for (const pattern of BLOCKED_CSS_PATTERNS) {
        if (pattern.test(source)) {
          violations.push(`${path.relative(COMPONENTS_ROOT, filePath)} matched ${pattern}`);
        }
      }
    }

    expect(violations).toEqual([]);
  });

  test("first-party consumers use the shared root barrel rather than deep-importing internals", () => {
    const violations: string[] = [];

    for (const filePath of FIRST_PARTY_CONSUMERS) {
      const source = fs.readFileSync(filePath, "utf8");
      for (const pattern of BLOCKED_CONSUMER_IMPORT_PATTERNS) {
        if (pattern.test(source)) {
          violations.push(`${path.relative(COMPONENTS_ROOT, filePath)} matched ${pattern}`);
        }
      }
    }

    expect(violations).toEqual([]);
  });

  test("shared package roots advertise explicit exports for vendoring", () => {
    const corePackage = JSON.parse(fs.readFileSync(path.join(CORE_PACKAGE_ROOT, "package.json"), "utf8"));
    const componentsPackage = JSON.parse(fs.readFileSync(path.join(COMPONENTS_PACKAGE_ROOT, "package.json"), "utf8"));

    expect(corePackage.name).toBe("@console-core");
    expect(corePackage.exports).toEqual({
      ".": "./src/index.ts",
      "./adapters": "./src/adapters.ts",
      "./contract": "./src/contract.ts",
      "./headless": "./src/headless.ts",
      "./network": "./src/network.ts",
      "./runtime-types": "./src/runtime-types.ts",
      "./runtime": "./index.js",
    });
    expect(componentsPackage.name).toBe("@console-components");
    expect(componentsPackage.exports).toEqual({
      ".": "./src/index.ts",
      "./conversation/console-conversation-panel": "./src/conversation/console-conversation-panel.tsx",
      "./pending/console-pending-stack": "./src/pending/console-pending-stack.tsx",
      "./topology/topology-panel": "./src/topology/topology-panel.tsx",
      "./topology/types": "./src/topology/types.ts",
      "./styles": "./src/styles/index.ts",
    });
    expect(componentsPackage.peerDependencies).toMatchObject({
      "@console-core": "0.0.0",
      react: "^19.2.4",
      "react-dom": "^19.2.4",
    });
  });
});
