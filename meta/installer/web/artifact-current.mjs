// The host proves current canonical inputs without rewriting receipt provenance.
export function artifactVersionCurrent(receipt, guide) {
  const version = receipt?.current_guide_version === undefined ? receipt?.guide_version : receipt.current_guide_version;
  return receipt?.guide_id === guide?.id && Number.isSafeInteger(receipt?.guide_version) &&
    Number.isInteger(version) && version === guide?.version;
}

export function artifactIsCurrent(receipt, guide) {
  return receipt?.current === true && artifactVersionCurrent(receipt, guide);
}
