import test from "node:test";
import assert from "node:assert/strict";
import { artifactIsCurrent } from "./artifact-current.mjs";
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
async function loadModule(path) {
  const built = await build({ entryPoints: [fileURLToPath(new URL(path, import.meta.url))],
    bundle: true, write: false, format: "esm", platform: "node", logLevel: "silent" });
  return import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].text).toString("base64")}`);
}
const { implementationIsCurrent } = await loadModule("implementation-panel.mjs");
const { applicationTrialAvailable, rehearsalCurrent } = await loadModule("rehearsal-panel.mjs");
import { deploymentCanPrepare, deploymentOptionsCurrent, deploymentUpdateAction } from "./deployment-panel.mjs";

test("host-proven unchanged inputs retain original artifact provenance across a harmless turn", () => {
  const guide = { id: "guide", version: 4, proposal: { target: "mac" } };
  const build = { guide_id: "guide", guide_version: 3, current_guide_version: 4, current: true,
    build_id: "build", status: "completed", source_digest: "a".repeat(64), source_verified: true,
    ready_to_package: true, acceptance_current: true, acceptance_trusted: true, acceptance: { status: "pass" } };
  assert.equal(artifactIsCurrent(build, guide), true);
  assert.equal(implementationIsCurrent(build, guide), true);
  assert.equal(applicationTrialAvailable(build, guide), true);
  assert.equal(rehearsalCurrent(build, guide), true);
  assert.equal(deploymentCanPrepare(build, guide), true);
  const options = { build_id: "build", guide_version: 3, current_guide_version: 4, current: true, target: "mac" };
  assert.equal(deploymentOptionsCurrent(options, build, guide), true);
  const receipt = { guide_id: "guide", deployment_id: "installed", status: "stopped",
    update: { update_id: "transaction", guide_version: 3, to_build_id: "build", source_digest: build.source_digest,
      status: "staged", blockers: [] } };
  assert.deepEqual(deploymentUpdateAction(receipt, build, guide, "apply-update"),
    { deployment_id: "installed", action: "apply-update", update_id: "transaction" });
  assert.equal(build.guide_version, 3);
  for (const current_guide_version of [3, null, "4", true]) {
    const stale = { ...build, current_guide_version };
    assert.equal(artifactIsCurrent(stale, guide), false);
    assert.equal(applicationTrialAvailable(stale, guide), false);
    assert.equal(deploymentCanPrepare(stale, guide), false);
    assert.equal(deploymentUpdateAction(receipt, stale, guide, "apply-update"), null);
  }
  assert.equal(deploymentOptionsCurrent({ ...options, current_guide_version: 3 }, build, guide), false);
  assert.equal(deploymentOptionsCurrent({ ...options, guide_version: 4 }, build, guide), false);
});

test("legacy same-version display remains valid without inventing cross-version authority", () => {
  const guide = { id: "guide", version: 3 };
  const receipt = { guide_id: "guide", guide_version: 3, current: true };
  assert.equal(artifactIsCurrent(receipt, guide), true);
  assert.equal(artifactIsCurrent(receipt, { ...guide, version: 4 }), false);
  assert.equal(artifactIsCurrent({ ...receipt, guide_id: "other", current_guide_version: 3 }, guide), false);
  assert.equal(artifactIsCurrent({ ...receipt, current: false, current_guide_version: 3 }, guide), false);
  assert.equal(artifactIsCurrent({ ...receipt, guide_version: true, current_guide_version: 3 }, guide), false);
});
