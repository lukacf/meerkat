#!/usr/bin/env bash
# Run in the extracted workspace: no Git worktree or worktree-only wrapper.
# Print only the executable path; keep Cargo's output on stderr.
set -euo pipefail
root=$(cd "$(dirname "$0")/../../../.." && pwd)
cd "$root"
host=$(rustc -vV | sed -n 's/^host: //p')
[[ -n "$host" ]] || { echo 'cannot determine Rust host target' >&2; exit 1; }
cargo build --locked --target "$host" --target-dir target -p meerkat-meta --example mdm_mob_target >&2
binary="$root/target/$host/debug/examples/mdm_mob_target"
[[ -x "$binary" ]] || { echo "Cargo did not produce $binary" >&2; exit 1; }
printf '%s\n' "$binary"
