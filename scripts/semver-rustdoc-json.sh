#!/usr/bin/env bash
# Generate the rustdoc JSON that cargo-semver-checks compares, for every
# publishable library crate of one checkout.
#
# Two callers:
#   - the release workflow runs it on the release tag and attaches the result
#     to the GitHub release as `semver-rustdoc-<version>.tar.zst`; that is the
#     baseline the next release measures against (`--baseline-rustdoc`);
#   - scripts/check-semver-breaks.sh runs it on the candidate tree and hands
#     the files to `--current-rustdoc`, so neither side of a measurement ever
#     rebuilds a workspace inside cargo-semver-checks.
#
# Fidelity rules. The two sides of a comparison must come from the same
# method, so this script is the only producer of both the published baseline
# and the candidate JSON that scripts/check-semver-breaks.sh hands to
# `--current-rustdoc`:
#   - every publishable library crate is documented in ONE cargo invocation,
#     so Cargo unifies workspace features the same way on both sides (several
#     crates enable `__meerkat-*` features of meerkat-core and meerkat-live;
#     a per-crate build would toggle them depending on which crate is built);
#   - per crate, features follow the cargo-semver-checks heuristic: every
#     feature except `unstable`, `nightly`, `bench`, `no_std` and names
#     starting with `_`, `unstable_` or `unstable-`, unless a dependent turns
#     one on through unification;
#   - rustdoc flags match the tool's: private and hidden items documented,
#     lints capped, JSON output;
#   - proc-macro and binary-only crates are skipped, as the tool skips them.
#
# The manifest records the rustc that produced the JSON. A consumer on a
# different rustc must not use these files: the rustdoc JSON format is tied to
# the compiler version.
#
# Usage:
#   scripts/semver-rustdoc-json.sh --source-root DIR --out DIR [--crate NAME]...
set -euo pipefail

source_root=""
out_dir=""
crates=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --source-root) source_root="$2"; shift 2 ;;
        --out) out_dir="$2"; shift 2 ;;
        --crate) crates+=("$2"); shift 2 ;;
        -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
        *) echo "error: unknown argument $1" >&2; exit 2 ;;
    esac
done
[[ -n "$source_root" && -n "$out_dir" ]] || {
    echo "usage: $0 --source-root DIR --out DIR [--crate NAME]..." >&2
    exit 2
}
source_root="$(cd "$source_root" && pwd)"
mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"

PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"

cargo_bin=("cargo")
if [[ -x "$source_root/scripts/repo-cargo" ]]; then
    cargo_bin=("$source_root/scripts/repo-cargo")
fi

if [[ ${#crates[@]} -eq 0 ]]; then
    mapfile -t crates < <("$source_root/scripts/release-rust-crates.sh")
fi

cd "$source_root"
metadata_file="$(mktemp)"
plan_file="$(mktemp)"
trap 'rm -f "$metadata_file" "$plan_file"' EXIT
"${cargo_bin[@]}" metadata --no-deps --format-version 1 >"$metadata_file"

# One line per requested crate: name<TAB>lib-target-name<TAB>features (comma
# separated, may be empty) or name<TAB>-<TAB>skip reason.
"$PYTHON" - "$metadata_file" "${crates[@]}" >"$plan_file" <<'PY'
import json
import sys

EXCLUDED = {"unstable", "nightly", "bench", "no_std"}
PREFIXES = ("_", "unstable_", "unstable-")

metadata = json.load(open(sys.argv[1]))
packages = {package["name"]: package for package in metadata["packages"]}
for name in sys.argv[2:]:
    package = packages.get(name)
    if package is None:
        print(f"{name}\t-\tnot a workspace member")
        continue
    lib_targets = [
        target for target in package["targets"] if "lib" in target["kind"] or "rlib" in target["kind"]
    ]
    kinds = {kind for target in package["targets"] for kind in target["kind"]}
    if "proc-macro" in kinds:
        print(f"{name}\t-\tproc-macro crate")
        continue
    if not lib_targets:
        print(f"{name}\t-\tno library target")
        continue
    features = sorted(
        feature
        for feature in package["features"]
        if feature not in EXCLUDED and not feature.startswith(PREFIXES)
    )
    print(f"{name}\t{lib_targets[0]['name']}\t{','.join(features)}")
PY

workspace_version="$(
    "$PYTHON" -c 'import json,sys; m=json.load(open(sys.argv[1])); print(next(p["version"] for p in m["packages"] if p["name"]=="meerkat-core"))' "$metadata_file"
)"
rustc_version="$(rustc -V)"
rustc_commit="$(rustc -Vv | awk '/^commit-hash:/ {print $2}')"
commit_sha="$(git rev-parse HEAD 2>/dev/null || echo unknown)"

doc_root="$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["target_directory"])' "$metadata_file")/semver-rustdoc"
mkdir -p "$doc_root"

generated=()
skipped=()
doc_args=(doc --no-deps --lib --target-dir "$doc_root")
lib_names=()
while IFS=$'\t' read -r name lib_name features; do
    [[ -n "$name" ]] || continue
    if [[ "$lib_name" == "-" ]]; then
        echo "semver-rustdoc: skip ${name} (${features})"
        skipped+=("${name}=${features}")
        continue
    fi
    echo "semver-rustdoc: ${name} features=[${features}]"
    doc_args+=(-p "$name")
    IFS=',' read -r -a feature_list <<<"$features"
    for feature in "${feature_list[@]}"; do
        [[ -n "$feature" ]] || continue
        doc_args+=(--features "${name}/${feature}")
    done
    generated+=("${name}=${features}")
    lib_names+=("${name}=${lib_name}")
done <"$plan_file"

if [[ ${#generated[@]} -eq 0 ]]; then
    echo "error: no library crate to document" >&2
    exit 1
fi
echo "semver-rustdoc: documenting ${#generated[@]} crate(s) in one build"
RUSTC_BOOTSTRAP=1 \
RUSTDOCFLAGS='-Z unstable-options --output-format json --document-private-items --document-hidden-items --cap-lints allow' \
    "${cargo_bin[@]}" "${doc_args[@]}" >/dev/null
for entry in "${lib_names[@]}"; do
    name="${entry%%=*}"
    lib_name="${entry#*=}"
    cp "$doc_root/doc/${lib_name}.json" "$out_dir/${name}.json"
done

"$PYTHON" - "$out_dir" "$workspace_version" "$commit_sha" "$rustc_version" "$rustc_commit" \
    "${#generated[@]}" "${generated[@]}" "${skipped[@]}" <<'PY'
import hashlib
import json
import pathlib
import sys

out_dir = pathlib.Path(sys.argv[1])
version, commit_sha, rustc_version, rustc_commit = sys.argv[2:6]
generated_count = int(sys.argv[6])
entries = sys.argv[7:]
generated = entries[:generated_count]
skipped = entries[generated_count:]

crates = {}
for entry in generated:
    name, features = entry.split("=", 1)
    path = out_dir / f"{name}.json"
    data = json.loads(path.read_text())
    crates[name] = {
        "file": path.name,
        "features": [f for f in features.split(",") if f],
        "format_version": data.get("format_version"),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }
manifest = {
    "schema_version": 1,
    "version": version,
    "commit_sha": commit_sha,
    "rustc_version": rustc_version,
    "rustc_commit_hash": rustc_commit,
    "rustdoc_flags": [
        "--document-private-items",
        "--document-hidden-items",
        "--cap-lints allow",
    ],
    "crates": crates,
    "skipped": dict(entry.split("=", 1) for entry in skipped),
}
(out_dir / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
print(f"semver-rustdoc: wrote {len(crates)} crate(s) and manifest to {out_dir}")
PY
