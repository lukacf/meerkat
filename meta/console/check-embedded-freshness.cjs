#!/usr/bin/env node

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const {
  DIST_GENERATED_FILES,
  EMBEDDED_GENERATED_FILES,
  EMBEDDED_SHARED_FILES,
  EMBEDDED_SOURCE_FILES,
} = require("./generated-assets.cjs");

const rootDir = path.resolve(__dirname, "..");
const distDir = path.join(__dirname, "dist");
const embeddedDir = path.join(rootDir, "crates", "meerkat-meta", "console-dist");
// The Rust modules that embed console-dist assets with include_str!.
const rustEmbeddingPaths = [
  path.join(rootDir, "crates", "meerkat-meta", "src", "http_console.rs"),
  path.join(rootDir, "crates", "meerkat-meta", "src", "mcp_apps_sandbox.rs"),
];

function read(filePath) {
  try {
    return fs.readFileSync(filePath);
  } catch (error) {
    process.stderr.write(`missing generated console asset: ${filePath}\n`);
    process.stderr.write(`${error.message}\n`);
    process.exit(1);
  }
}

function assertDirectoryFiles(directory, expectedFiles, label) {
  const expected = new Set(expectedFiles);
  let actual;
  try {
    actual = fs.readdirSync(directory).filter((entry) => (
      fs.statSync(path.join(directory, entry)).isFile()
    ));
  } catch (error) {
    process.stderr.write(`missing generated console directory: ${directory}\n`);
    process.stderr.write(`${error.message}\n`);
    process.exit(1);
  }
  const unexpected = actual.filter((file) => !expected.has(file)).sort();
  const missing = expectedFiles.filter((file) => !actual.includes(file));
  if (unexpected.length > 0 || missing.length > 0) {
    process.stderr.write(
      `${label} generated asset set drifted; missing=[${missing.join(", ")}] unexpected=[${unexpected.join(", ")}]\n`,
    );
    process.exit(1);
  }
}

function assertRustEmbeddedAssets() {
  const actual = rustEmbeddingPaths
    .flatMap((rustPath) => [
      ...read(rustPath).toString("utf8").matchAll(/include_str!\(\s*"\.\.\/console-dist\/([^"]+)"\s*\)/g),
    ])
    .map((match) => match[1])
    .sort();
  const expected = [...EMBEDDED_GENERATED_FILES].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    process.stderr.write(
      `Rust embedded console asset list drifted; expected=[${expected.join(", ")}] actual=[${actual.join(", ")}]\n`,
    );
    process.exit(1);
  }
}

function assertGitTracksFiles(files, label) {
  const result = spawnSync("git", ["ls-files", "--error-unmatch", "--", ...files], {
    cwd: rootDir,
    stdio: "ignore",
  });
  if (result.status !== 0) {
    process.stderr.write(
      `${label} contains generated assets that are not tracked by git; add the generated console assets before building release binaries\n`,
    );
    process.exit(result.status || 1);
  }
}

// esbuild names each bundled module by its path in comments and module keys.
// Built from a worktree whose node_modules is a symlink into another
// checkout, those paths leave the repo ("../../<other-worktree>/..."); an
// absolute path leaks the builder's filesystem. Either one is also a build
// no other checkout reproduces.
const LOCAL_PATH_PATTERNS = [
  { label: "a path outside the repository", pattern: /(?:^|["'\s(])(?:\.\.\/){2,}[^\s"']*/m },
  { label: "an absolute local path", pattern: /(?:\/home\/|\/Users\/|\/tmp\/|\/private\/var\/|[A-Za-z]:\\\\Users\\\\)[^\s"']*/ },
];

function assertNoLocalPaths(directory, files, label) {
  for (const file of files) {
    const source = read(path.join(directory, file)).toString("utf8");
    for (const { label: kind, pattern } of LOCAL_PATH_PATTERNS) {
      const match = source.match(pattern);
      if (match) {
        process.stderr.write(
          `${label}/${file} contains ${kind} (${match[0].trim()}); rebuild from a checkout with its own node_modules\n`,
        );
        process.exit(1);
      }
    }
  }
}

assertDirectoryFiles(distDir, DIST_GENERATED_FILES, "console/dist");
assertNoLocalPaths(distDir, DIST_GENERATED_FILES, "console/dist");
assertNoLocalPaths(embeddedDir, EMBEDDED_GENERATED_FILES, "crates/meerkat-meta/console-dist");
assertDirectoryFiles(embeddedDir, EMBEDDED_GENERATED_FILES, "crates/meerkat-meta/console-dist");
assertRustEmbeddedAssets();
assertGitTracksFiles(
  DIST_GENERATED_FILES.map((file) => path.join("console", "dist", file)),
  "console/dist",
);
assertGitTracksFiles(
  EMBEDDED_GENERATED_FILES.map((file) => path.join("crates", "meerkat-meta", "console-dist", file)),
  "crates/meerkat-meta/console-dist",
);

for (const file of EMBEDDED_SHARED_FILES) {
  const distPath = path.join(distDir, file);
  const embeddedPath = path.join(embeddedDir, file);
  if (!read(distPath).equals(read(embeddedPath))) {
    process.stderr.write(
      `embedded console asset is stale: ${path.relative(rootDir, embeddedPath)} does not match ${path.relative(rootDir, distPath)}\n`,
    );
    process.exit(1);
  }
}

for (const { source, embedded } of EMBEDDED_SOURCE_FILES) {
  const sourcePath = path.join(__dirname, source);
  const embeddedPath = path.join(embeddedDir, embedded);
  if (!read(sourcePath).equals(read(embeddedPath))) {
    process.stderr.write(
      `embedded console source is stale: ${path.relative(rootDir, embeddedPath)} does not match ${path.relative(rootDir, sourcePath)}\n`,
    );
    process.exit(1);
  }
}

const diff = spawnSync(
  "git",
  ["diff", "--quiet", "HEAD", "--", "console/dist", "crates/meerkat-meta/console-dist"],
  { cwd: rootDir, stdio: "inherit" },
);
if (diff.status !== 0) {
  process.stderr.write(
    "console build left generated asset diffs; commit the refreshed console/dist and crates/meerkat-meta/console-dist assets before building release binaries\n",
  );
  process.exit(diff.status || 1);
}

process.stdout.write("embedded console assets are fresh\n");
