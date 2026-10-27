#!/usr/bin/env bash
set -euo pipefail

pack_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "${pack_dir}/../.." && pwd)"

# Meerkat 0.7's machine-authority code allocates huge debug-build stack
# frames. The mdm_mob_target binary sizes its own threads; this export is
# belt and braces because this script launches the prebuilt debug binary
# directly (bypassing the workspace .cargo/config.toml [env] section).
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
state_dir="${pack_dir}/.state"
bindings_dir="${state_dir}/bindings"
targets_file="${MDM_TARGET_BINDINGS_FILE:-${state_dir}/target-bindings.json}"

usage() {
  cat >&2 <<'USAGE'
Usage:
  local-target.sh start [--id target-a] [--name this-mac] [--listen 127.0.0.1:5791] [--advertise tcp://127.0.0.1:5791]
  local-target.sh foreground [same options]
  local-target.sh stop [--id target-a]
  local-target.sh status [--id target-a]

Environment:
  MDM_TARGET_BINDINGS_FILE   Aggregated console binding file.
  MDM_TARGET_MODEL           Target model, default gpt-5.5.
  MDM_TARGET_PROVIDER        Target provider, default openai.
USAGE
}

command="${1:-start}"
if [[ $# -gt 0 ]]; then shift; fi

id="target-a"
name="$(hostname -s 2>/dev/null || hostname)"
listen="127.0.0.1:5791"
advertise=""
site="local"
platform="$(uname -s | tr '[:upper:]' '[:lower:]')-local"
model="${MDM_TARGET_MODEL:-gpt-5.5}"
provider="${MDM_TARGET_PROVIDER:-openai}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --id) id="$2"; shift 2 ;;
    --name) name="$2"; shift 2 ;;
    --listen) listen="$2"; shift 2 ;;
    --advertise) advertise="$2"; shift 2 ;;
    --site) site="$2"; shift 2 ;;
    --platform) platform="$2"; shift 2 ;;
    --model) model="$2"; shift 2 ;;
    --provider) provider="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage; exit 2 ;;
  esac
done

mkdir -p "$bindings_dir" "${state_dir}/targets/${id}"
pid_file="${state_dir}/targets/${id}.pid"
log_file="${state_dir}/targets/${id}.log"
binding_file="${bindings_dir}/${id}.json"

target_bin_args=(
  --id "$id"
  --name "$name"
  --listen "$listen"
  --site "$site"
  --platform "$platform"
  --data-dir "${state_dir}/targets/${id}"
  --binding-out "$binding_file"
  --model "$model"
  --provider "$provider"
)
if [[ -n "$advertise" ]]; then
  target_bin_args+=(--advertise "$advertise")
fi

target_bin() {
  local cargo_target_dir
  cargo_target_dir="$(cd "$repo_root" && ./scripts/repo-cargo --print-env | awk -F= '$1 == "CARGO_TARGET_DIR" { print $2 }')"
  printf "%s/debug/examples/mdm_mob_target" "$cargo_target_dir"
}

case "$command" in
  start)
    if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      echo "[mdm-local-target] already running: $id pid=$(cat "$pid_file")"
      exit 0
    fi
    rm -f "$binding_file"
    (cd "$repo_root" && ./scripts/repo-cargo build -p meerkat-meta --example mdm_mob_target) >"$log_file" 2>&1
    nohup "$(target_bin)" "${target_bin_args[@]}" >>"$log_file" 2>&1 </dev/null &
    echo $! >"$pid_file"
    for _ in {1..120}; do
      [[ -s "$binding_file" ]] && break
      sleep 0.5
    done
    if [[ ! -s "$binding_file" ]]; then
      echo "[mdm-local-target] target did not write binding; log: $log_file" >&2
      exit 1
    fi
    node "${pack_dir}/scripts/merge-bindings.mjs" "$targets_file" "$binding_file" >/dev/null
    echo "[mdm-local-target] started $id pid=$(cat "$pid_file")"
    echo "[mdm-local-target] binding: $binding_file"
    echo "[mdm-local-target] targets: $targets_file"
    ;;
  foreground)
    cd "$repo_root"
    exec ./scripts/repo-cargo run -p meerkat-meta --example mdm_mob_target -- "${target_bin_args[@]}"
    ;;
  stop)
    if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      kill "$(cat "$pid_file")"
      rm -f "$pid_file"
      echo "[mdm-local-target] stopped $id"
    else
      echo "[mdm-local-target] not running: $id"
    fi
    ;;
  status)
    if [[ -f "$pid_file" ]] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      echo "[mdm-local-target] running $id pid=$(cat "$pid_file")"
      echo "[mdm-local-target] binding: $binding_file"
    else
      echo "[mdm-local-target] stopped $id"
    fi
    ;;
  *)
    usage
    exit 2
    ;;
esac
