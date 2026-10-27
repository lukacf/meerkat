const SHA256 = /^[a-f0-9]{64}$/;

/** Export the installed receipt's source, independently of the current build. */
export function installedPackageBinding(receipt, guide) {
  if (!guide?.id || receipt?.guide_id !== guide.id || (receipt.target ?? "local") !== "local" ||
      receipt.source_kind != null && receipt.source_kind !== "application" ||
      !["deployment_id", "build_id"].every(key => typeof receipt[key] === "string" && receipt[key].length > 0) ||
      !SHA256.test(receipt.source_digest || "") || !SHA256.test(receipt.package_sha256 || "")) return null;
  return { deployment_id: receipt.deployment_id, build_id: receipt.build_id,
    source_digest: receipt.source_digest, package_sha256: receipt.package_sha256 };
}

export function installedPackageUrl(guideId, binding) {
  return `/api/guide/${encodeURIComponent(guideId)}/deployment/installed-package?${new URLSearchParams(binding)}`;
}

export async function verifyInstalledPackage(blob, expected) {
  if (!blob || blob.size > 64 * 1024 * 1024 || !SHA256.test(expected || ""))
    throw new Error("The installed package could not be checked. Recheck installation status before downloading again.");
  const digest = await crypto.subtle.digest("SHA-256", await blob.arrayBuffer());
  const actual = Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("");
  if (actual !== expected) throw new Error("The downloaded package differs from this installation. Recheck installation status before downloading again.");
}
