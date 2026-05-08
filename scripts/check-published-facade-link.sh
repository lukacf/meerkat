#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"

version="$(awk '
  /^\[workspace.package\]/ { in_workspace_package = 1; next }
  /^\[/ { in_workspace_package = 0 }
  in_workspace_package && $1 == "version" {
    gsub(/"/, "", $3)
    print $3
    exit
  }
' "$ROOT/Cargo.toml")"

if [[ -z "$version" ]]; then
  echo "failed to resolve workspace package version" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

package_target="${MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET:-$tmp_dir/package-target}"
registry_dir="$tmp_dir/registry-src"
smoke_dir="$tmp_dir/downstream-smoke"
patch_cfg="$tmp_dir/patch.toml"

mkdir -p "$registry_dir" "$smoke_dir/src"

release_crates=()
while IFS= read -r crate; do
  release_crates+=("$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

if [[ -z "${MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET:-}" ]]; then
  for crate in "${release_crates[@]}"; do
    package_cfg="$tmp_dir/$crate.package.toml"
    "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$package_cfg"
    CARGO_TARGET_DIR="$package_target" \
      "$CARGO" package -p "$crate" --locked --allow-dirty --no-verify --config "$package_cfg" \
      >/dev/null
  done
fi

for crate in "${release_crates[@]}"; do
  crate_archive="$package_target/package/$crate-$version.crate"
  if [[ ! -f "$crate_archive" ]]; then
    echo "missing packaged crate archive: $crate_archive" >&2
    exit 1
  fi
  if [[ "$crate" == "meerkat-core" ]]; then
    tar -xzf "$crate_archive" -C "$registry_dir"
    mv "$registry_dir/meerkat-core-$version" "$registry_dir/meerkat-core-0.0.0-stale"
  fi
  tar -xzf "$crate_archive" -C "$registry_dir"
done

{
  echo "[patch.crates-io]"
  for crate in "${release_crates[@]}"; do
    echo "$crate = { path = \"$registry_dir/$crate-$version\" }"
  done
} > "$patch_cfg"

cat > "$smoke_dir/Cargo.toml" <<EOF
[package]
name = "meerkat-published-facade-link-smoke"
version = "0.0.0"
edition = "2024"

[dependencies]
async-trait = "0.1"
futures = "0.3"
meerkat = { version = "=$version", default-features = false }
meerkat-core = "=$version"
EOF

cp \
  "$ROOT/meerkat/tests/fixtures/agent_builder_policy/downstream_public_facade_agentbuilder.rs" \
  "$smoke_dir/src/main.rs"

CARGO_TARGET_DIR="$tmp_dir/smoke-target" \
  "$CARGO" run --quiet --manifest-path "$smoke_dir/Cargo.toml" --config "$patch_cfg" \
  | grep -F "public facade AgentBuilder constructed an agent" >/dev/null

echo "Published-style facade AgentBuilder link smoke passed"
