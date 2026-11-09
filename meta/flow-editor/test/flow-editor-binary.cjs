"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const repoRoot = path.join(__dirname, "..", "..");

// The repo builds through scripts/repo-cargo, which isolates CARGO_TARGET_DIR
// outside the repo tree, so target/debug/ may not exist locally.
function resolveFlowEditorBinary() {
  if (process.env.META_FLOW_EDITOR_BIN) return process.env.META_FLOW_EDITOR_BIN;
  const candidates = [];
  if (process.env.CARGO_TARGET_DIR) {
    candidates.push(path.join(process.env.CARGO_TARGET_DIR, "debug", "meta_flow_editor"));
  }
  candidates.push(path.join(repoRoot, "target", "debug", "meta_flow_editor"));
  const wrapperTargetDir = repoCargoTargetDir();
  if (wrapperTargetDir) {
    candidates.push(path.join(wrapperTargetDir, "debug", "meta_flow_editor"));
  }
  return candidates.find((candidate) => fs.existsSync(candidate)) || candidates[0];
}

function repoCargoTargetDir() {
  const wrapper = path.join(repoRoot, "scripts", "repo-cargo");
  if (!fs.existsSync(wrapper)) return "";
  try {
    const output = execFileSync(wrapper, ["--print-env"], { encoding: "utf8" });
    return output.match(/^CARGO_TARGET_DIR=(.+)$/m)?.[1] || "";
  } catch {
    return "";
  }
}

module.exports = { resolveFlowEditorBinary };
