// Generated contract-version compatibility helper for the Meerkat SDK
// Source: meerkat-contracts ContractVersion::is_compatible_with (version.rs)
//
// Compatibility: same major version (for 1.0+), or same major+minor (for 0.x). Mirrors `meerkat_contracts::ContractVersion::is_compatible_with`.

function coreParts(version: string): number[] {
  const core = version.split("-", 1)[0].split("+", 1)[0];
  return core.split(".").map(Number);
}

/**
 * True iff a server contract version is compatible with a client contract
 * version under the canonical rule (0.x: same major+minor; >=1.0: same major).
 */
export function isCompatibleWith(server: string, client: string): boolean {
  const s = coreParts(server);
  const c = coreParts(client);
  if (s.some(Number.isNaN) || c.some(Number.isNaN) || s.length < 2 || c.length < 2) {
    return false;
  }
  if (s[0] === 0 && c[0] === 0) {
    return s[1] === c[1];
  }
  return s[0] === c[0];
}
