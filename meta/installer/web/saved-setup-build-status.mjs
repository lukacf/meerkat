import {
  implementationIsCurrent, implementationIsRunning, implementationCanDownload,
  implementationAcceptance, implementationPhase, implementationTiming,
} from "./implementation-panel.mjs";

const statuses = new Set(["queued", "running", "checking", "completed", "failed", "interrupted"]);
export const unknownSavedBuildStatus = () => ({
  kind: "unknown", label: "Build status unavailable",
  detail: "Check again to see the current saved application work.", action: null, poll: false,
});

/** The canonical GET proves current inputs; guide list rows have no version. */
export function savedSetupBuildStatus(setup, receipt) {
  if (!setup || typeof setup.id !== "string" || !setup.id || receipt?.guide_id !== setup.id ||
      typeof receipt.build_id !== "string" || !receipt.build_id ||
      !Number.isSafeInteger(receipt.guide_version) || receipt.guide_version < 1 ||
      !statuses.has(receipt.status) || typeof receipt.current !== "boolean") return unknownSavedBuildStatus();
  const version = Object.hasOwn(setup, "version") ? setup.version : receipt.current_guide_version;
  if (receipt.current && (!Number.isSafeInteger(version) || version < 1 ||
      !Number.isSafeInteger(receipt.current_guide_version) || receipt.current_guide_version < 1))
    return unknownSavedBuildStatus();
  const guide = { id: setup.id, version };
  const current = implementationIsCurrent(receipt, guide);
  const phase = implementationPhase(receipt);
  const common = {
    action: "Return to build", current, buildId: receipt.build_id,
    guideVersion: receipt.guide_version, currentGuideVersion: current ? version : null,
    timing: implementationTiming(receipt)?.summary || null,
    poll: implementationIsRunning(receipt) && receipt.cleanup_custody_lost !== true,
  };
  if (!current) return { ...common, kind: "stale", label: "Build for an earlier design",
    detail: "Revisit its saved work. It does not establish a checked build for your current design." };
  if (receipt.cleanup_custody_lost === true || receipt.cleanup_pending === true &&
      !["queued", "running", "checking"].includes(receipt.status) && receipt.cleanup_in_progress !== true)
    return { ...common, kind: "attention", label: "Build cleanup needs attention",
      detail: "Saved work is kept. Open the build to check its cleanup before continuing." };
  if (implementationIsRunning(receipt)) return { ...common, kind: "running", label: phase.label,
    detail: "You can close this tab while the Forge server keeps running. Server shutdown interrupts the build and preserves its saved work." };
  if (receipt.status === "interrupted") return { ...common, kind: "interrupted", label: "Build interrupted",
    detail: "Your saved work is kept. Open the build to see what needs finishing; it does not resume automatically." };
  if (receipt.status === "failed") return { ...common, kind: "attention", label: "Build needs attention",
    detail: "Revisit the saved work and findings before continuing." };
  const acceptance = implementationAcceptance(receipt, guide);
  if (implementationCanDownload(receipt, guide) && receipt.source_verified === true &&
      receipt.cleanup_in_progress !== true && acceptance?.passed === true)
    return { ...common, kind: "checked", label: "Checked build ready to review", action: "Review completed build",
      detail: "Independent offline checks passed. Build checks do not establish installation or live use." };
  return { ...common, kind: "attention", label: acceptance && !acceptance.passed
    ? "Build needs more work" : "Saved build needs verification",
    detail: "Open the build to inspect its checks and the next available step." };
}
