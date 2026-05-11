#!/usr/bin/env bash

set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
DRY_RUN="${MEERKAT_REGISTRY_DRY_RUN:-false}"
PUBLISH_TIMEOUT="${MEERKAT_CRATE_PUBLISH_TIMEOUT:-20m}"
NO_VERIFY="${MEERKAT_CARGO_PUBLISH_NO_VERIFY:-true}"
RUNNER_TEMP="${RUNNER_TEMP:-/tmp}"

case "$DRY_RUN" in
  1|true|TRUE|yes|YES|on|ON) DRY_RUN=true ;;
  *) DRY_RUN=false ;;
esac

case "$NO_VERIFY" in
  1|true|TRUE|yes|YES|on|ON) NO_VERIFY=true ;;
  *) NO_VERIFY=false ;;
esac

publish_one() {
  local crate="$1"
  local tmp_cfg
  local output_file
  local target_dir
  local rc

  tmp_cfg="$(mktemp)"
  output_file="$(mktemp)"
  target_dir="${RUNNER_TEMP}/meerkat-publish-target/${crate}"

  "$ROOT/scripts/generate-patch-config.sh" "$ROOT" "$crate" > "$tmp_cfg"
  mkdir -p "${target_dir}/package"
  ln -sfn "${ROOT}/meerkat-core" "${target_dir}/package/meerkat-core"

  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::group::Publish Rust crate ${crate}"
  fi

  echo "  ${crate}: starting"
  if [[ "$DRY_RUN" == "true" ]]; then
    echo "  ${crate}: cargo publish dry-run"
    set +e
    CARGO_TARGET_DIR="$target_dir" \
      cargo publish -p "$crate" --locked --dry-run --config "$tmp_cfg" \
      > >(tee "$output_file") 2> >(tee -a "$output_file" >&2)
    rc=$?
    set -e
  else
    local cmd=(cargo publish -p "$crate" --locked --config "$tmp_cfg")
    if [[ "$NO_VERIFY" == "true" ]]; then
      cmd+=(--no-verify)
      echo "  ${crate}: upload mode skips cargo's duplicate verifier; release validation already packaged and linked these crates"
    fi

    local attempt=1
    local max_attempts="${MEERKAT_CRATE_PUBLISH_ATTEMPTS:-5}"
    if ! [[ "$max_attempts" =~ ^[0-9]+$ ]] || ((max_attempts < 1)); then
      max_attempts=5
    fi

    set +e
    while true; do
      : > "$output_file"
      if command -v timeout >/dev/null 2>&1; then
        CARGO_TARGET_DIR="$target_dir" \
          timeout "$PUBLISH_TIMEOUT" "${cmd[@]}" \
          > >(tee "$output_file") 2> >(tee -a "$output_file" >&2)
        rc=$?
      else
        CARGO_TARGET_DIR="$target_dir" \
          "${cmd[@]}" \
          > >(tee "$output_file") 2> >(tee -a "$output_file" >&2)
        rc=$?
      fi

      if [[ $rc -eq 0 ]] || ! grep -q "Too Many Requests" "$output_file" || ((attempt >= max_attempts)); then
        break
      fi

      echo "  ${crate}: crates.io rate limited publish attempt ${attempt}/${max_attempts}; retrying in 15s" >&2
      sleep 15
      attempt=$((attempt + 1))
    done
    set -e
  fi

  if [[ $rc -eq 0 ]]; then
    echo "  ${crate}: published"
  elif grep -Eq "already exists|already uploaded" "$output_file"; then
    echo "  ${crate}: already published, skipping"
  elif [[ $rc -eq 124 ]]; then
    echo "  ${crate}: FAILED after ${PUBLISH_TIMEOUT}" >&2
    rm -f "$tmp_cfg" "$output_file"
    if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
      echo "::endgroup::"
    fi
    exit 124
  else
    echo "  ${crate}: FAILED" >&2
    rm -f "$tmp_cfg" "$output_file"
    if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
      echo "::endgroup::"
    fi
    exit "$rc"
  fi

  rm -f "$tmp_cfg" "$output_file"
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::endgroup::"
  fi
}

if [[ "$DRY_RUN" == "true" ]]; then
  echo "Running cargo publish in dry-run mode"
else
  echo "Publishing crates to crates.io"
fi

while IFS= read -r crate; do
  publish_one "$crate"
done < <("$ROOT/scripts/release-rust-crates.sh")
