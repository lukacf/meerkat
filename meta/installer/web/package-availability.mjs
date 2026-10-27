import { artifactIsCurrent } from "./artifact-current.mjs";

/** Current compiler support belongs only to the saved proposal in this response. */
export function packageAvailability(guide, plan = guide?.proposal || guide?.blueprint) {
  const selected = guide?.proposal || guide?.blueprint;
  const value = guide?.package_availability;
  if (!selected || plan !== selected || !value || value.guide_id !== guide.id ||
      value.guide_version !== guide.version || value.source !== (guide.proposal ? "proposal" : "blueprint") ||
      !/^[a-f0-9]{64}$/.test(value.blueprint_hash || "") || typeof value.available !== "boolean") return null;
  return value;
}

export function packagePreparationReason(guide, plan) {
  const value = packageAvailability(guide, plan);
  return value?.available === false && typeof value.reason === "string" && value.reason.trim()
    ? value.reason : null;
}

/** A continuing candidate uses the host's existing exact-source observation. */
export function retainsCandidateFramework(receipt, guide) {
  const candidate = receipt?.framework?.runtime_candidate;
  return artifactIsCurrent(receipt, guide) && typeof receipt.build_id === "string" && !!receipt.build_id &&
    /^[a-f0-9]{64}$/.test(receipt.source_digest || "") && candidate != null &&
    typeof candidate === "object" && !Array.isArray(candidate);
}

/** A host-selected product refreshes its runtime only from its exact current, independently accepted build. */
export function candidateRuntimeRefreshReady(receipt, guide) {
  return retainsCandidateFramework(receipt, guide) && receipt.status === "completed" &&
    ["current", "source_verified", "acceptance_trusted", "acceptance_current", "ready_to_package"]
      .every((key) => receipt[key] === true);
}

export const PACKAGE_REVISION_PROMPT = "Review the current package preparation limitation with me. Preserve my requirements and choices. Explain which installation support is missing and what would enable my current plan. Ask before changing the plan.";
