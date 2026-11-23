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

# Space readings on exit, pass or fail, before the cleanup: the free space of
# the filesystems the smoke writes to and the sizes of its build directory
# and of the unpacked packages, so a smoke that ends for lack of space says
# how much it had used. Observation only: nothing is removed beyond the
# cleanup that was always here, no check reads a reading, and a reading that
# cannot be taken prints "unavailable". The readings run in a subshell whose
# errors are not printed and whose status is discarded (space_reading_on_exit
# below), before the cleanup, so the script exits with the status it had when
# the trap began. No reading writes a file (a here-string needs temporary
# space, which a full disk does not have). Sizes come from `du -P`, which
# follows no symbolic link. Lines carry labels, not paths.

# "<free KiB> <total KiB> <filesystem id>" of the filesystem that holds $1
# (of its nearest existing ancestor while $1 does not exist), or nothing.
space_fs_reading() {
  local path="$1"
  while [[ ! -e "$path" && "$path" == */* && "$path" != / ]]; do
    path="${path%/*}"
    [[ -n "$path" ]] || path=/
  done
  [[ -e "$path" ]] || path=.
  df -Pk -- "$path" 2>/dev/null | awk '
    NR == 2 && NF >= 6 && $(NF - 2) ~ /^[0-9]+$/ && $(NF - 4) ~ /^[0-9]+$/ {
      print $(NF - 2), $(NF - 4), $1 ":" $NF
    }'
}

# One line per label and path pair after the boundary's name: the free and
# total space of the filesystem that holds the path. A filesystem an earlier
# pair already named is said to be the same one.
space_free_lines() {
  local boundary="$1" label="" reading="" free="" total="" id="" rest="" note="" index=0
  local -a seen_ids=() seen_labels=()
  shift
  while [[ "$#" -ge 2 ]]; do
    label="$1"
    reading="$(space_fs_reading "$2")" || reading=""
    shift 2
    free="${reading%% *}"
    rest="${reading#* }"
    total="${rest%% *}"
    id="${rest#* }"
    if [[ ! "$free" =~ ^[0-9]+$ || ! "$total" =~ ^[0-9]+$ ]]; then
      printf 'space reading [%s]: %s filesystem: unavailable\n' "$boundary" "$label"
      continue
    fi
    note=""
    index=0
    while [[ "$index" -lt "${#seen_ids[@]}" ]]; do
      if [[ "${seen_ids[$index]}" == "$id" ]]; then
        note=" (same filesystem as ${seen_labels[$index]})"
        break
      fi
      index=$((index + 1))
    done
    if [[ -z "$note" ]]; then
      seen_ids+=("$id")
      seen_labels+=("$label")
    fi
    printf 'space reading [%s]: %s filesystem: %d MiB free of %d MiB%s\n' \
      "$boundary" "$label" "$((free / 1024))" "$((total / 1024))" "$note"
  done
}

# The size of one directory of this run, under a label. `du -skP`: no
# symbolic link is followed, on the command line or below it.
space_dir_line() {
  local boundary="$1" label="$2" dir="$3" sizes="" size="" note=""
  if [[ -L "$dir" || ! -d "$dir" ]]; then
    printf 'space reading [%s]: %s: no such directory\n' "$boundary" "$label"
    return 0
  fi
  sizes="$(du -skP -- "$dir" 2>/dev/null)" || note=" (du reported an error: the figure may be partial)"
  size="${sizes%%$'\t'*}"
  if [[ ! "$size" =~ ^[0-9]+$ ]]; then
    printf 'space reading [%s]: %s: unavailable\n' "$boundary" "$label"
    return 0
  fi
  printf 'space reading [%s]: %s: %d MiB%s\n' "$boundary" "$label" "$((size / 1024))" "$note"
}

# $1: the status the script is exiting with.
space_exit_reading() {
  local boundary="link smoke exit, status $1" started="$SECONDS"
  space_free_lines "$boundary" "temporary directory" "$tmp_dir" "root" /
  space_dir_line "$boundary" "smoke target" "$tmp_dir/smoke-target"
  space_dir_line "$boundary" "unpacked registry directory" "$tmp_dir/registry-src"
  printf 'space reading [%s]: took %d s\n' "$boundary" "$((SECONDS - started))"
}

# The exit readings, guarded: in a subshell (a failure in it, a fatal one
# included, ends only the subshell), without its error output (a shell error
# message names a path), and with a status that is always 0. $1: the status
# the script is exiting with.
space_reading_on_exit() {
  (space_exit_reading "$1") 2>/dev/null ||
    echo "space reading [link smoke exit, status $1]: unavailable" || true
}

tmp_dir="$(mktemp -d)"
trap 'space_reading_on_exit "$?"; rm -rf "$tmp_dir"' EXIT

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
  "$ROOT/crates/meerkat/tests/fixtures/agent_builder_policy/downstream_public_facade_agentbuilder.rs" \
  "$smoke_dir/src/main.rs"

CARGO_TARGET_DIR="$tmp_dir/smoke-target" \
  "$CARGO" run --quiet --manifest-path "$smoke_dir/Cargo.toml" --config "$patch_cfg" \
  | grep -F "public facade AgentBuilder constructed an agent" >/dev/null

echo "Published-style facade AgentBuilder link smoke passed"
