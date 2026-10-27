const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const root = path.join(__dirname, "..");
const zipPath = process.env.META_EDITOR_HANDOFF_ZIP ||
  path.join(os.homedir(), "Downloads", "Meerkat-Meta Editor-handoff.zip");
const zipRoot = "meerkat-meta-editor/project";

function fromZip(entry) {
  const result = spawnSync("unzip", ["-p", zipPath, `${zipRoot}/${entry}`], { encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`failed to read ${entry} from handoff zip: ${result.stderr || result.stdout}`);
  }
  return result.stdout;
}

function local(entry) {
  return fs.readFileSync(path.join(root, "src", entry), "utf8");
}

function diff(entry) {
  const result = spawnSync("diff", [
    "-u",
    "-",
    path.join(root, "src", entry),
  ], {
    encoding: "utf8",
    input: fromZip(entry),
  });
  return result.stdout;
}

if (!fs.existsSync(zipPath)) {
  if (process.env.META_EDITOR_REQUIRE_HANDOFF === "1") {
    throw new Error(`handoff zip not found at ${zipPath}; set META_EDITOR_HANDOFF_ZIP to the designer handoff archive`);
  }
  process.stdout.write(`handoff zip not found at ${zipPath}; visual contract skipped (set META_EDITOR_REQUIRE_HANDOFF=1 to make this an error)\n`);
  process.exit(0);
}

assert.equal(local("tokens.css"), fromZip("tokens.css"), "design tokens must match the handoff exactly");

function allowedCssDelta(sign, line) {
  if (line.startsWith("---") || line.startsWith("+++")) return true;
  if (line.startsWith("@@")) return true;
  const text = line.slice(1);
  if (text.trim() === "}") return true;
  const shared = [
    /Source drawer/,
    /YAML drawer/,
    /source-drawer/,
    /yaml/,
    /toml-/,
    /y-key/,
    /y-comment/,
    /subagent/,
    /sub-mob/,
    /mob\.toml drawer/,
    /grid-template-columns: auto auto auto auto/,
    /minmax\(0, 1fr\)/,
    /min-width: 0/,
    /overflow: hidden/,
    /text-overflow: ellipsis/,
    /white-space: nowrap/,
    /\.crumbs \.crumb\.is-current/,
    /color: var\(--ink\)/,
    /actions-menu/,
    /drysim/,
    /Dry sim panel/,
    /Deploy plan trace panel/,
    /ds-in/,
    /deploy-plan-in/,
    /deploy-plan/,
    /max-width: 980px/,
    /max-width: 760px/,
    /grid-template-columns: auto auto minmax/,
    /grid-template-columns: auto auto minmax\(0, 1fr\) auto/,
    /grid-template-rows: minmax\(320px, 1fr\) minmax\(260px, 42vh\)/,
    /\.bld-opt:disabled/,
    /cursor: not-allowed/,
    /opacity: 1/,
    /padding: 0 8px/,
    /font-size: 0/,
    /gap: 0/,
    /max-width: 52px/,
    /left: 12px/,
    /right: 12px/,
    /bottom: 12px/,
    /top: 56px/,
    /width: auto/,
    /width: min\(100%, calc\(100vw - 24px\)\)/,
    /max-height: calc\(100vh - 72px\)/,
    /transform: none/,
    /align-items: flex-start/,
    /padding: 12px 14px/,
    /flex-wrap: wrap/,
    /justify-content: flex-end/,
    /padding: 8px 14px 14px/,
    /grid-template-columns: 16px minmax\(0, 1fr\)/,
    /grid-column: 2/,
    /overflow-wrap: anywhere/,
    /min-height: 320px/,
    /border-left: 0/,
    /border-top: 1px solid var\(--hair\)/,
    /\.(?:library__bullet|agents-list__bullet|add-menu__dot)\[data-role="(?:planner|coder|reviewer|critic|judge|publisher|illustrator|shell|schema)"\]/,
    /--role-color/,
    /agents-list__bullet/,
    /font-size: 10px; line-height: 1; color: var\(--muted\); text-align: center;/,
    /add-menu__dot \{ width: 8px; height: 8px; border-radius: 50%; background:/,
    // .node must not form a stacking context: it would trap the out-port
    // under .edges-svg and edge hit areas would swallow port drags.
    /z-index: 1/,
    /No z-index here/,
    /stacking context/,
    /swallow port drags/,
    /Breadcrumb quick-switcher/,
    /Settings section/,
    /empty tab strip/,
    /Embedded host chrome/,
    /is-embedded/,
    // Adaptive layer authoring (PR #181): block + panel section comments.
    /Adaptive layer/,
    /Adaptive panel/,
  ];
  if (shared.some((pattern) => pattern.test(text))) return true;
  if (sign === "+") {
    return [
      /tool-row--invalid/,
      /branch-cond-row/,
      /inline-skill/,
      /skill-chip em/,
      /skill-chip\.is-invalid/,
      /button\.node/,
      /a\.node/,
      /node--source-file/,
      /source-file-adornment/,
      /source-file__/,
      /agent-runtime/,
      /source-file-list/,
      /source-file-row/,
      /bld-toml--graph/,
      /agent-editor__confirm/,
      /actions-menu/,
      /grid-template-columns/,
      /min-width: 84px/,
      /min-width: 152px/,
      /max-width: 120px/,
    ].some((pattern) => pattern.test(text));
  }
  return false;
}

const styleDiff = diff("styles.css")
  .split("\n")
  .filter((line) => line.startsWith("+") || line.startsWith("-") || line.startsWith("@@"));

let allowedAddedBlock = false;
const unexpected = styleDiff.filter((line) => {
  const sign = line[0];
  if (line.trim() === "+") return false;
  if (line.startsWith("@@")) {
    allowedAddedBlock = false;
    return !allowedCssDelta(sign, line);
  }
  if (line.startsWith("+++") || line.startsWith("---")) return false;
  if (sign === "+" && allowedAddedBlock) {
    if (line.includes("}")) allowedAddedBlock = false;
    return false;
  }
  if (sign === "+" && /(\.inline-skill|\+\.skill-chip em|\.crumbs \.crumb\.is-current|button\.node|a\.node|\.source-file-adornment|\.node--source-file|\.source-file__|\.source-file-list|\.source-file-row|\.bld-toml--graph|\.agent-editor__confirm|\.agent-runtime|\.actions-menu|\.mob-status|\.toprail|\.brand|\.viewtabs|\.actions|\.stage|\.btn--sm|\.theme-toggle|\.crumb-switcher|\.settings-view|\.flows-view__empty|\.flows-list__section|\.deploy-plan|\.source-drawer|\.validate|\.builder|\.bld-stage|\.bld-panel|\.bld-adaptive|\.bld-aframe|\.bld-anode|\.bld-aconn|\.bld-afan|\.ap-)/.test(line)) {
    if (!line.includes("}")) allowedAddedBlock = true;
    return false;
  }
  if (sign !== "+" && sign !== "-") return !allowedCssDelta(sign, line);
  return !allowedCssDelta(sign, line);
});

assert.deepEqual(unexpected, [], "styles.css drifted from the handoff outside the allowed Meta-state additions");
