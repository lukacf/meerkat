#!/usr/bin/env bash
set -euo pipefail

pack_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo_root="$(cd "${pack_dir}/../.." && pwd)"
env_file="${MDM_GCP_ENV_FILE:-${pack_dir}/.env.gcp}"
state_dir="${pack_dir}/.state"
bindings_dir="${state_dir}/bindings"
targets_file="${MDM_TARGET_BINDINGS_FILE:-${state_dir}/target-bindings.json}"

if [[ -f "$env_file" ]]; then
  set -a
  # shellcheck disable=SC1090
  . "$env_file"
  set +a
fi

usage() {
  cat >&2 <<'USAGE'
Usage:
  gcp-target.sh start --id target-b [--name gcp-target-b]
  gcp-target.sh fetch --id target-b
  gcp-target.sh stop --id target-b

Required environment or .env.gcp:
  GCP_PROJECT
  GCP_ZONE

Optional:
  GCP_TARGET_PREFIX     default meta-mdm-target
  GCP_MACHINE_TYPE      default e2-medium
  GCP_IMAGE_FAMILY      default debian-12
  GCP_IMAGE_PROJECT     default debian-cloud
  GCP_SSH_USER
  MDM_TARGET_MODEL      default gpt-5.5
  MDM_TARGET_PROVIDER   default openai
  MDM_GCP_FORWARD_ENV   space-separated env names to copy to the VM
USAGE
}

command="${1:-start}"
if [[ $# -gt 0 ]]; then shift; fi

id=""
name=""
listen=""
advertise=""
site="gcp-live"
platform="linux-gcp-vm"
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

[[ -n "$id" ]] || { echo "--id is required" >&2; usage; exit 2; }
name="${name:-$id}"
: "${GCP_PROJECT:?GCP_PROJECT is required}"
: "${GCP_ZONE:?GCP_ZONE is required}"

prefix="${GCP_TARGET_PREFIX:-meta-mdm-target}"
machine_type="${GCP_MACHINE_TYPE:-e2-medium}"
image_family="${GCP_IMAGE_FAMILY:-debian-12}"
image_project="${GCP_IMAGE_PROJECT:-debian-cloud}"
instance="${prefix}-${id}"
ssh_target="$instance"
if [[ -n "${GCP_SSH_USER:-}" ]]; then
  ssh_target="${GCP_SSH_USER}@${instance}"
fi

gcloud_base=(gcloud --project "$GCP_PROJECT" compute)
ssh_base=("${gcloud_base[@]}" ssh "$ssh_target" --zone "$GCP_ZONE" --quiet)
scp_base=("${gcloud_base[@]}" scp --zone "$GCP_ZONE" --quiet)

shell_quote() {
  printf "'"
  printf "%s" "$1" | sed "s/'/'\\\\''/g"
  printf "'"
}

ensure_instance() {
  if "${gcloud_base[@]}" instances describe "$instance" --zone "$GCP_ZONE" >/dev/null 2>&1; then
    return
  fi
  "${gcloud_base[@]}" instances create "$instance" \
    --zone "$GCP_ZONE" \
    --machine-type "$machine_type" \
    --image-family "$image_family" \
    --image-project "$image_project" \
    --tags meta-mdm-target
}

internal_ip() {
  "${gcloud_base[@]}" instances describe "$instance" \
    --zone "$GCP_ZONE" \
    --format='get(networkInterfaces[0].networkIP)'
}

sync_repo() {
  tar -C "$repo_root" \
    --exclude ./target \
    --exclude ./node_modules \
    --exclude ./examples/node_modules \
    --exclude ./examples/004-mdm-console-pack/.state \
    -czf - . | "${ssh_base[@]}" --command "
      set -euo pipefail
      rm -rf ~/meerkat-meta.next
      mkdir -p ~/meerkat-meta.next
      tar -xzf - -C ~/meerkat-meta.next
      rm -rf ~/meerkat-meta
      mv ~/meerkat-meta.next ~/meerkat-meta
    "
}

sync_remote_env() {
  local env_tmp
  env_tmp="$(mktemp)"
  chmod 600 "$env_tmp"
  local names="${MDM_GCP_FORWARD_ENV:-OPENAI_API_KEY ANTHROPIC_API_KEY GOOGLE_API_KEY GEMINI_API_KEY OPENAI_BASE_URL OPENAI_ORG_ID}"
  for env_name in $names; do
    if [[ -n "${!env_name:-}" ]]; then
      printf "%s=" "$env_name" >>"$env_tmp"
      shell_quote "${!env_name}" >>"$env_tmp"
      printf "\n" >>"$env_tmp"
    fi
  done
  "${ssh_base[@]}" --command "mkdir -p ~/.cache/mdm-mob-target && chmod 700 ~/.cache/mdm-mob-target"
  "${scp_base[@]}" "$env_tmp" "${ssh_target}:~/.cache/mdm-mob-target/${id}.env"
  rm -f "$env_tmp"
}

remote_start() {
  local target_listen="$1"
  local target_advertise="$2"
  "${ssh_base[@]}" --command "
    set -euo pipefail
    set -a
    . ~/.cache/mdm-mob-target/${id}.env
    set +a
    if ! command -v cargo >/dev/null 2>&1; then
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
      . \"\$HOME/.cargo/env\"
    else
      . \"\$HOME/.cargo/env\" 2>/dev/null || true
    fi
    if ! dpkg -s build-essential pkg-config libssl-dev >/dev/null 2>&1; then
      sudo apt-get update
      sudo apt-get install -y build-essential pkg-config libssl-dev
    fi
    cd ~/meerkat-meta
    mkdir -p ~/.cache/mdm-mob-target /tmp/mdm-mob-target
    if [[ -f /tmp/mdm-${id}.pid ]]; then kill \"\$(cat /tmp/mdm-${id}.pid)\" 2>/dev/null || true; fi
    rm -f /tmp/mdm-mob-target/${id}.json
    ./scripts/repo-cargo build -p meerkat-meta --example mdm_mob_target
    target_bin=\"\$(./scripts/repo-cargo --print-env | awk -F= '\$1 == \"CARGO_TARGET_DIR\" { print \$2 }')/debug/examples/mdm_mob_target\"
    nohup \"\$target_bin\" \
      --id '${id}' \
      --name '${name}' \
      --listen '${target_listen}' \
      --advertise '${target_advertise}' \
      --site '${site}' \
      --platform '${platform}' \
      --data-dir ~/.cache/mdm-mob-target/${id} \
      --binding-out /tmp/mdm-mob-target/${id}.json \
      --model '${model}' \
      --provider '${provider}' \
      > /tmp/mdm-${id}.log 2>&1 &
    echo \$! > /tmp/mdm-${id}.pid
    for i in \$(seq 1 180); do
      test -s /tmp/mdm-mob-target/${id}.json && exit 0
      sleep 1
    done
    tail -100 /tmp/mdm-${id}.log >&2
    exit 1
  "
}

fetch_binding() {
  mkdir -p "$bindings_dir"
  local binding_file="${bindings_dir}/${id}.json"
  "${scp_base[@]}" "${ssh_target}:/tmp/mdm-mob-target/${id}.json" "$binding_file"
  node "${pack_dir}/scripts/merge-bindings.mjs" "$targets_file" "$binding_file" >/dev/null
  echo "[mdm-gcp-target] binding: $binding_file"
  echo "[mdm-gcp-target] targets: $targets_file"
}

case "$command" in
  start)
    ensure_instance
    sync_repo
    sync_remote_env
    if [[ -n "$listen" && -z "$advertise" ]]; then
      advertise="tcp://${listen}"
    fi
    remote_start "${listen:-0.0.0.0:5791}" "${advertise:-tcp://$(internal_ip):5791}"
    fetch_binding
    ;;
  fetch)
    fetch_binding
    ;;
  stop)
    "${ssh_base[@]}" --command "if [[ -f /tmp/mdm-${id}.pid ]]; then kill \$(cat /tmp/mdm-${id}.pid) 2>/dev/null || true; rm -f /tmp/mdm-${id}.pid; fi"
    ;;
  *)
    usage
    exit 2
    ;;
esac
