import assert from "node:assert/strict";
import test from "node:test";
import { build } from "esbuild";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { fileURLToPath } from "node:url";
import { deploymentUpdateDiagnostics, deploymentFailedAttempts } from "./deployment-panel.mjs";

// The actual failed-update/current-source recovery retains its earlier startup
// error after the host verifies recovery. Keep that evidence as transaction
// history, instead of presenting it as the verified result's current failure.
const recovered = { update_id: "exact-recovery-transaction", status: "completed",
  to_build_id: "accepted-candidate", source_digest: "a".repeat(64),
  activation_error: "The candidate did not finish its first Console startup check.",
  recovery_error: "The earlier recovery attempt stopped before readiness." };

test("verified update results retain earlier attempt diagnostics without changing their transaction binding", () => {
  const before = structuredClone(recovered);
  for (const status of ["completed", "rolled-back"]) {
    const value = deploymentUpdateDiagnostics({ ...recovered, status });
    assert.equal(value.historical, true);
    assert.deepEqual(value.entries.map((entry) => entry.detail), [recovered.activation_error, recovered.recovery_error]);
    assert.deepEqual(value.entries.map((entry) => entry.label), ["Earlier startup attempt", "Earlier recovery attempt"]);
  }
  for (const status of ["recovery-needed", "recovering-current", "applying", "failed"]) {
    const value = deploymentUpdateDiagnostics({ ...recovered, status });
    assert.equal(value.historical, false);
    assert.equal(value.entries[0].label, "Startup result");
  }
  assert.deepEqual(recovered, before);
  assert.deepEqual(deploymentUpdateDiagnostics(null), { historical: false, entries: [] });
});

// Render the actual production diagnostics component, not a second test-only
// layout. This is offline presentation evidence, not a new recovery operation.
const bundle = await build({ entryPoints: ["deployment-panel.jsx"],
  absWorkingDir: fileURLToPath(new URL(".", import.meta.url)), bundle: true,
  format: "esm", platform: "node", write: false, loader: { ".css": "empty" }, logLevel: "silent" });
const { DeploymentUpdateDiagnostics } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].contents).toString("base64")}`);
const render = (update) => renderToStaticMarkup(React.createElement(DeploymentUpdateDiagnostics, { update }));

test("the recovered production panel places old failures in a closed history disclosure", () => {
  const html = render(recovered);
  assert.match(html, /<details class="deployment-evidence">/);
  assert.doesNotMatch(html, /<details[^>]*\bopen\b|class="deployment-error"|Startup result:/);
  assert.match(html, /Earlier startup and recovery attempts/);
  assert.ok(html.includes(recovered.activation_error));
  assert.ok(html.includes(recovered.recovery_error));
  const escaped = render({ ...recovered, activation_error: "<script>earlier diagnostic</script>" });
  assert.ok(escaped.includes("&lt;script&gt;earlier diagnostic&lt;/script&gt;"));
  assert.doesNotMatch(escaped, /<script>/);
});

test("unresolved production recovery retains visible current diagnostics and empty history is absent", () => {
  const html = render({ ...recovered, status: "recovery-needed" });
  assert.match(html, /class="deployment-error">Startup result:/);
  assert.match(html, /class="deployment-error">Current application recovery result:/);
  assert.doesNotMatch(html, /<details|Earlier startup and recovery attempts/);
  assert.equal(render({ status: "completed" }), "");
});

const archivedUpdate = () => ({
  ...recovered, update_id: "89a6a5cf-2ddd-4ff6-a5b1-a4ecaf847457", status: "rolled-back",
  from_build_id: "previous-build", to_build_id: "candidate-build",
  package_sha256: "b".repeat(64), previous_package_sha256: "c".repeat(64),
  source_digest: "d".repeat(64), previous_source_digest: "e".repeat(64),
  failed_attempts: [{ attempt_id: "b70d45a5-2d22-4c0b-b37d-8bd71d204da3",
    update_id: "89a6a5cf-2ddd-4ff6-a5b1-a4ecaf847457", build_id: "candidate-build",
    package_sha256: "b".repeat(64), source_digest: "d".repeat(64), source: "candidate",
    service_instance_id: "d64a894e-9b7e-4391-9f77-c0b48b65da6b",
    startup_attempt_id: "c6b6e214-2834-4eb6-b28e-d40aef1ca0a5", service_cleanup_confirmed: false,
    captured_at: "2026-10-01T14:00:00Z", error_type: "RuntimeError",
    error: "The candidate startup did not pass.",
    logs: [{ at: "2026-10-01T14:00:00Z", phase: "runtime", text: "Exact earlier candidate output." }],
    logs_truncated: false,
    scope: "Historical redacted supervisor output, not current health or a typed cause." }],
  failed_attempts_truncated: 0,
});

test("archived attempts bind candidate and fallback build, package and checked source without promoting output claims", () => {
  const update = archivedUpdate();
  update.failed_attempts.push({ ...update.failed_attempts[0],
    attempt_id: "7b17b39f-15f0-4f59-89c5-2875c78d0748", source: "previous", build_id: update.from_build_id,
    package_sha256: update.previous_package_sha256, source_digest: update.previous_source_digest,
    service_instance_id: null, startup_attempt_id: null,
    private_settings: { PASSWORD: "must-not-project" }, provider_failed: true, ready: true,
    logs: [{ at: "2026-10-01T14:01:00Z", phase: "runtime", text: "Earlier previous-source output." }] });
  const before = structuredClone(update), projected = deploymentFailedAttempts(update);
  assert.equal(projected.attempts.length, 2);
  assert.equal(projected.unavailable, false);
  assert.deepEqual(projected.attempts.map((item) => [item.source, item.build_id, item.source_digest]), [
    ["candidate", update.to_build_id, update.source_digest], ["previous", update.from_build_id, update.previous_source_digest] ]);
  assert.doesNotMatch(JSON.stringify(projected), /must-not-project|provider_failed|service_cleanup_confirmed|"ready"/);
  projected.attempts[0].logs[0].text = "modified projection";
  assert.deepEqual(update, before);
});

test("another update or mismatched archive source never contaminates this transaction's history", () => {
  const update = archivedUpdate(), record = update.failed_attempts[0];
  for (const change of [{ update_id: "ddea2f22-5349-4e50-a379-b640f741edb6" },
    { build_id: update.from_build_id }, { package_sha256: update.previous_package_sha256 },
    { source_digest: update.previous_source_digest }, { source: "unknown" },
    { attempt_id: "not-an-owned-uuid" }, { startup_attempt_id: "not-an-owned-uuid" }]) {
    const value = deploymentFailedAttempts({ ...update, failed_attempts: [{ ...record, ...change }] });
    assert.equal(value.attempts.length, 0);
    assert.equal(value.unavailable, true);
  }
  const replaced = render({ ...update, update_id: "ddea2f22-5349-4e50-a379-b640f741edb6" });
  assert.doesNotMatch(replaced, /Exact earlier candidate output/);
  assert.match(replaced, /could not be matched to this update/);
  const previous = { ...record, source: "previous", build_id: update.from_build_id,
    package_sha256: update.previous_package_sha256, source_digest: update.previous_source_digest };
  assert.equal(deploymentFailedAttempts({ ...update, failed_attempts: [previous], previous_source_digest: undefined }).attempts.length, 0);
  assert.equal(deploymentFailedAttempts({ ...update, failed_attempts: [previous] }).attempts.length, 1);
});

test("archive presentation remains bounded and preserves exact Unicode text on a copied projection", () => {
  const update = archivedUpdate(), record = update.failed_attempts[0];
  const unicode = "Observing \u00d6land \ud83d\udc4d";
  assert.equal(deploymentFailedAttempts({ ...update, failed_attempts: [{ ...record, error: unicode }] }).attempts[0].error, unicode);
  for (const changed of [{ ...record, error: "x".repeat(1501) },
    { ...record, logs: [{ ...record.logs[0], text: "\ud83d\udc4d".repeat(4001) }] },
    { ...record, logs: Array.from({ length: 121 }, () => record.logs[0]) },
    { ...record, logs: Array.from({ length: 9 }, () => ({ ...record.logs[0], text: "x".repeat(4000) })) },
    { ...record, logs_truncated: "true" }]) {
    const projected = deploymentFailedAttempts({ ...update, failed_attempts: [changed] });
    assert.equal(projected.attempts.length, 0);
    assert.equal(projected.unavailable, true);
  }
  const duplicate = deploymentFailedAttempts({ ...update, failed_attempts: [record, record] });
  assert.equal(duplicate.attempts.length, 1);
  assert.equal(duplicate.unavailable, true);
  assert.equal(deploymentFailedAttempts({ ...update, failed_attempts: Array(9).fill(record) }).attempts.length, 0);
  const full = Array.from({ length: 8 }, (_, index) => ({ ...record,
    attempt_id: `b70d45a5-2d22-4c0b-b37d-8bd71d204d0${index}`,
    logs: Array.from({ length: 8 }, () => ({ ...record.logs[0], text: "x".repeat(3900) })) }));
  const bounded = deploymentFailedAttempts({ ...update, failed_attempts: full });
  assert.ok(bounded.attempts.length > 0 && bounded.attempts.length < 8);
  assert.equal(bounded.unavailable, true);
});

test("the recovered production disclosure keeps exact archived output optional and clearly historical", () => {
  const update = archivedUpdate();
  update.failed_attempts[0].error = "<script>historical error</script>";
  update.failed_attempts[0].logs[0].text = "<script>historical output</script>\n\u00d6land \ud83d\udc4d";
  update.failed_attempts[0].logs_truncated = true;
  update.failed_attempts_truncated = 2;
  const html = render(update);
  assert.match(html, /Earlier startup and recovery attempts/);
  assert.match(html, /Earlier candidate startup/);
  assert.match(html, new RegExp(update.failed_attempts[0].attempt_id));
  assert.match(html, /build <code>candidate-build<\/code>/);
  assert.match(html, /Redacted output \(1 entry\)/);
  assert.match(html, /earlier diagnostic output/);
  assert.match(html, /do not establish current health, cleanup or an application result/);
  assert.match(html, /2 older attempt records were omitted/);
  assert.match(html, /Some earlier output was omitted/);
  assert.match(html, /&lt;script&gt;historical output&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script>|<details[^>]*\bopen\b|class="deployment-error"/);
  assert.ok(html.includes("\u00d6land \ud83d\udc4d"));
  const archiveOnly = render({ ...update, activation_error: undefined, recovery_error: undefined });
  assert.match(archiveOnly, /Earlier candidate startup/);
  assert.match(archiveOnly, /historical output/);
  const unresolved = render({ ...update, status: "recovery-needed" });
  assert.match(unresolved, /class="deployment-error">Startup result:/);
  assert.match(unresolved, /Earlier startup and recovery attempts/);
  assert.doesNotMatch(unresolved, /<details[^>]*\bopen\b/);
});
