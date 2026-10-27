import { implementationIsCurrent } from "./implementation-panel.mjs";

export function captureSavedBuildNavigation(observed, token, guide) {
  if (!observed || observed.guideId !== guide?.id || !Number.isSafeInteger(guide.version) ||
      typeof observed.buildId !== "string" || !observed.buildId ||
      !Number.isSafeInteger(observed.guideVersion) || observed.guideVersion < 1 ||
      observed.currentGuideVersion !== null && observed.currentGuideVersion !== guide.version) return null;
  return { ...observed, token, selectedVersion: guide.version };
}

// Read-only navigation follows the selected build while its source can evolve.
// Build, install and run operations retain their own stronger admission checks.
export function savedBuildNavigationMatches(intent, guide, snapshot, token) {
  const receipt = snapshot?.receipt;
  if (!intent || intent.token !== token || guide?.id !== intent.guideId ||
      guide.version !== intent.selectedVersion || snapshot?.guideId !== intent.guideId ||
      snapshot.selectionToken !== token || snapshot.observation !== "known" ||
      receipt?.guide_id !== intent.guideId || receipt.build_id !== intent.buildId ||
      receipt.guide_version !== intent.guideVersion) return false;
  return intent.currentGuideVersion === null ? receipt.current === false
    : implementationIsCurrent(receipt, guide);
}
