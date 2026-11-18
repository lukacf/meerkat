// Only this standalone workspace has direct CI coverage. This is not a
// blanket examples/ exception. The complete manifest hash binds every feature,
// target and dependency declaration; changed semantics retain workspace coverage
// until this registration and its tests are reviewed together.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";

export const EXAMPLE_035 = "examples/035-mdm-tux-rs";
export const EXAMPLE_035_MANIFEST = `${EXAMPLE_035}/Cargo.toml`;
export const EXAMPLE_035_MANIFEST_SHA256 = "7b1698b9acef88ac519e10b18e2baba4e91c1dfad193c2760ae867e709b118e0";
// Derived from all normal/build/dev path dependencies in the hashed manifest.
export const EXAMPLE_035_DEPENDENCIES = [
  "meerkat",
  "meerkat-client",
  "meerkat-comms",
  "meerkat-core",
  "meerkat-machine-dsl",
  "meerkat-machine-schema",
  "meerkat-mcp",
  "meerkat-mob",
  "meerkat-mob-mcp",
  "meerkat-models",
  "meerkat-rpc",
  "meerkat-runtime",
  "meerkat-store"
];
export const EXAMPLE_035_COMMANDS = [
  `make -C ${EXAMPLE_035} lint`,
  `make -C ${EXAMPLE_035} test`,
];

export function ownsExample035(path) {
  return path.startsWith(`${EXAMPLE_035}/`);
}

// No Cargo metadata call or cache write. Include current types of tracked
// inputs and changed untracked paths; an unsupported symlink cannot justify
// narrowing, even when its target happens to exist inside the repository.
export function readExample035Inputs(root, changed = []) {
  for (const directory of ["examples", EXAMPLE_035]) {
    if (!lstatSync(resolve(root, directory)).isDirectory() || lstatSync(resolve(root, directory)).isSymbolicLink()) {
      throw new Error("standalone035 manifest parent must be a regular directory");
    }
  }
  const manifest = resolve(root, EXAMPLE_035, "Cargo.toml");
  if (!lstatSync(manifest).isFile() || lstatSync(manifest).isSymbolicLink()) {
    throw new Error("standalone035 manifest must be a regular file");
  }
  const indexed = execFileSync("git", ["ls-files", "--stage", "-z", "--", EXAMPLE_035], { cwd: root, encoding: "utf8" })
    .split("\0").filter(Boolean);
  const unsupported = new Set();
  const paths = indexed.map((entry) => {
    const match = entry.match(/^(\d{6}) [0-9a-f]+ [0-3]\t(.+)$/s);
    if (!match) throw new Error("unreadable standalone035 Git input");
    if (!["100644", "100755"].includes(match[1])) unsupported.add(match[2]);
    return match[2];
  });
  for (const path of [...paths, ...changed.filter(ownsExample035)]) {
    for (let at = path; at.startsWith(EXAMPLE_035); at = dirname(at)) {
      try {
        const st = lstatSync(resolve(root, at));
        if (st.isSymbolicLink() || (!st.isFile() && !st.isDirectory())) unsupported.add(at);
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
    }
  }
  return { manifestText: readFileSync(manifest, "utf8"), unsupported: [...unsupported].sort() };
}

// Optional and target-specific edges are intentionally not pruned. Include
// normal/build/dev edges throughout: wider than one compilation, preserving
// push coverage. This graph selects the standalone job, never workspace seeds.
export function example035Selection({ event, packages, changed, inputs, globalChange = false }) {
  // parseArgs already maps workflow_dispatch to push. Missing/unknown event
  // must not silently acquire the narrower pull-request policy.
  if (!["pull_request", "push"].includes(event)) throw new Error("standalone035 requires a normalized planner event");
  const byName = new Map(packages.map((pkg) => [pkg.name, pkg]));
  const mismatch = createHash("sha256").update(inputs.manifestText).digest("hex") !== EXAMPLE_035_MANIFEST_SHA256;
  const missing = EXAMPLE_035_DEPENDENCIES.filter((name) => !byName.has(name));
  const unsupported = inputs.unsupported ?? [];
  const fallback = mismatch ? "standalone035 manifest changed; registration requires review"
    : missing.length ? `standalone035 workspace dependency missing: ${missing.join(", ")}`
    : unsupported.length ? `standalone035 unsupported linked/special input: ${unsupported[0]}` : "";
  const closure = new Set(EXAMPLE_035_DEPENDENCIES.filter((name) => byName.has(name)));
  const queue = [...closure];
  for (let i = 0; i < queue.length; i += 1) {
    for (const dep of byName.get(queue[i]).dependencies) {
      if (dep.source === null && byName.has(dep.name) && !closure.has(dep.name)) {
        closure.add(dep.name);
        queue.push(dep.name);
      }
    }
  }
  const ownPaths = (changed ?? []).filter((path) => ownsExample035(path) && !/\.mdx?$/.test(path));
  const selection = {
    event,
    selected: Boolean(fallback || ownPaths.length),
    reasons: [
      ...(fallback ? [fallback] : []),
      ...(ownPaths.length ? [`standalone035 input: ${ownPaths[0]}`] : []),
    ],
    deferredReasons: [],
    fallback,
    // Only PRs gain the registered Rust exception. A push still plans its
    // original workspace coverage for unowned Rust sources/manifests.
    ownedPaths: event === "pull_request" && !fallback ? ownPaths : [],
    dependencyClosure: [...closure].sort(),
  };
  if (changed === null) broaderExample035Reason(selection, "unknown diff");
  if (globalChange) broaderExample035Reason(selection, "workspace build input or explicit workspace plan");
  return selection;
}

function broaderExample035Reason(selection, reason) {
  if (selection.event === "pull_request") {
    selection.deferredReasons.push(reason);
  } else {
    selection.selected = true;
    selection.reasons.push(reason);
  }
}

export function selectExample035Dependencies(selection, changedPackages, workspaceReason = "") {
  const dependencies = new Set(selection.dependencyClosure);
  const reached = changedPackages.filter((name) => dependencies.has(name));
  if (reached.length) broaderExample035Reason(selection, `standalone035 dependency: ${reached.sort().join(", ")}`);
  if (workspaceReason) broaderExample035Reason(selection, `workspace plan: ${workspaceReason}`);
  return selection;
}
