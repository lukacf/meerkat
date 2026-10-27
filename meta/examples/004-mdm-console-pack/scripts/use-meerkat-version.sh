#!/usr/bin/env bash
set -euo pipefail

pack_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "${pack_dir}/../.." && pwd)"
version="${1:-}"

if [[ -z "$version" ]]; then
  echo "usage: use-meerkat-version.sh <version>" >&2
  echo "example: use-meerkat-version.sh 0.6.30" >&2
  exit 2
fi

crate_manifest="${repo_root}/crates/meerkat-meta/Cargo.toml"
crates=()

# Keep this helper aligned with the manifest instead of maintaining a second,
# inevitably stale list. This covers normal and dev dependencies (including
# memory, live, and schedule) and de-duplicates crates such as meerkat-mob that
# intentionally appear in both sections with different feature sets.
while IFS= read -r crate; do
  crates+=("$crate")
done < <(
  perl -ne '
    if (/^\[(dependencies|dev-dependencies)\]\s*$/) {
      $in_dependencies = 1;
      next;
    }
    if (/^\[/) {
      $in_dependencies = 0;
    }
    if ($in_dependencies && /^(meerkat(?:-[A-Za-z0-9_-]+)?)\s*=/) {
      print "$1\n";
    }
  ' "$crate_manifest" | LC_ALL=C sort -u
)

if [[ ${#crates[@]} -eq 0 ]]; then
  echo "no direct Meerkat dependencies found in ${crate_manifest}" >&2
  exit 1
fi

if ! cargo search meerkat --limit 1 | grep -F "meerkat = \"${version}\"" >/dev/null; then
  echo "meerkat ${version} is not visible on crates.io yet" >&2
  exit 1
fi

for crate in "${crates[@]}"; do
  perl -0pi -e "s/(\\Q${crate}\\E\\s*=\\s*\\{\\s*version\\s*=\\s*)\"[^\"]+\"/\${1}\"=${version}\"/g; s/(\\Q${crate}\\E\\s*=\\s*)\"[^\"]+\"/\${1}\"=${version}\"/g" "$crate_manifest"
done

# Exact pins are part of the console pack's reproducibility contract. Verify
# every direct Meerkat dependency declaration, including duplicate dev entries,
# before asking Cargo to resolve the new version.
MEERKAT_VERSION="$version" perl -ne '
  BEGIN {
    $expected = "=" . $ENV{"MEERKAT_VERSION"};
    $failed = 0;
  }
  if (/^\[(dependencies|dev-dependencies)\]\s*$/) {
    $in_dependencies = 1;
    next;
  }
  if (/^\[/) {
    $in_dependencies = 0;
  }
  next unless $in_dependencies;
  next unless /^(meerkat(?:-[A-Za-z0-9_-]+)?)\s*=\s*(.*)$/;

  $crate = $1;
  $spec = $2;
  if ($spec =~ /^"([^"]+)"/ || $spec =~ /^\{.*?\bversion\s*=\s*"([^"]+)"/) {
    $actual = $1;
    if ($actual ne $expected) {
      print STDERR "$crate must be pinned to $expected, found $actual\n";
      $failed = 1;
    }
  } else {
    print STDERR "$crate has no directly verifiable version pin\n";
    $failed = 1;
  }
  END {
    exit($failed ? 1 : 0);
  }
' "$crate_manifest"

cd "$repo_root"
for crate in "${crates[@]}"; do
  ./scripts/repo-cargo update -p "$crate" --precise "$version"
done

echo "[mdm] Meerkat dependencies pinned to ${version}"
