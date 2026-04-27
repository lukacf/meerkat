#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
example_dir="$(cd "${script_dir}/.." && pwd)"
cd "$example_dir"

compose_file="${COMPOSE_FILE:-docker-compose.tux.yml}"
session_name="${TUX_TMUX_SESSION:-mdm-tux-live-suite}"
run_id="${TUX_LIVE_RUN_ID:-$(date +%Y%m%d%H%M%S)}"

hive_wait="${TUX_HIVE_WAIT_SECONDS:-120}"
target_wait="${TUX_TARGET_WAIT_SECONDS:-120}"
receive_wait="${TUX_RECEIVE_WAIT_SECONDS:-120}"
schedule_wait="${TUX_SCHEDULE_WAIT_SECONDS:-150}"

compose=(docker compose -f "$compose_file")

require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required" >&2
    exit 1
  fi
}

api_key_var_for_provider() {
  case "$1" in
    openai) echo "OPENAI_API_KEY" ;;
    anthropic) echo "ANTHROPIC_API_KEY" ;;
    gemini) echo "GEMINI_API_KEY" ;;
    *)
      echo "unsupported provider for live suite: $1" >&2
      exit 2
      ;;
  esac
}

default_live_provider_matrix() {
  export MDM_HIVE_MODEL="${MDM_HIVE_MODEL:-gemini-3.1-pro-preview}"
  export MDM_HIVE_PROVIDER="${MDM_HIVE_PROVIDER:-gemini}"
  export MDM_TARGET_A_MODEL="${MDM_TARGET_A_MODEL:-gpt-5.5}"
  export MDM_TARGET_A_PROVIDER="${MDM_TARGET_A_PROVIDER:-openai}"
  export MDM_TARGET_B_MODEL="${MDM_TARGET_B_MODEL:-claude-opus-4-7}"
  export MDM_TARGET_B_PROVIDER="${MDM_TARGET_B_PROVIDER:-anthropic}"
}

require_provider_key() {
  local label="$1"
  local provider="$2"
  local key_var
  local key_value

  key_var="$(api_key_var_for_provider "$provider")"
  key_value="${!key_var:-}"
  if [[ -z "$key_value" || "$key_value" == "dummy-mdm-tux-smoke-key" ]]; then
    echo "docker-live-suite needs a real ${key_var} for ${label} (${provider})" >&2
    exit 2
  fi
}

require_live_provider_keys() {
  require_provider_key "hive" "$MDM_HIVE_PROVIDER"
  require_provider_key "target-a" "$MDM_TARGET_A_PROVIDER"
  require_provider_key "target-b" "$MDM_TARGET_B_PROVIDER"
}

wait_for_tux() {
  local deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if tmux capture-pane -t "$session_name" -p 2>/dev/null | grep -q "Ready to send"; then
      return 0
    fi
    sleep 2
  done
  tmux capture-pane -t "$session_name" -p -S -120 2>/dev/null || true
  echo "TUX did not become ready" >&2
  exit 1
}

send_prompt() {
  local prompt="$1"
  tmux send-keys -t "$session_name" -l "$prompt"
  tmux send-keys -t "$session_name" Enter
}

start_tux_tmux() {
  local cmd
  local env_args=()
  local name

  for name in \
    OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY \
    MDM_HIVE_MODEL MDM_HIVE_PROVIDER \
    MDM_TARGET_A_MODEL MDM_TARGET_A_PROVIDER \
    MDM_TARGET_B_MODEL MDM_TARGET_B_PROVIDER \
    RUST_LOG; do
    if [[ -n "${!name+x}" ]]; then
      env_args+=("-e" "${name}=${!name}")
    fi
  done

  printf -v cmd '%q ' "${compose[@]}" run --rm --no-deps tux
  tmux new-session -d -s "$session_name" -c "$example_dir" "${env_args[@]}" "$cmd"
}

verify_file_contains() {
  local container="$1"
  local path="$2"
  local needle="$3"
  echo "checking $container:$path"
  "${compose[@]}" exec -T "$container" sh -lc "test -f '$path' && grep -F '$needle' '$path' && cat '$path'"
}

require docker
require tmux
default_live_provider_matrix
require_live_provider_keys

echo "Starting Docker TUX topology for live suite run $run_id"
echo "Provider matrix:"
echo "  hive=${MDM_HIVE_PROVIDER}/${MDM_HIVE_MODEL}"
echo "  target-a=${MDM_TARGET_A_PROVIDER}/${MDM_TARGET_A_MODEL}"
echo "  target-b=${MDM_TARGET_B_PROVIDER}/${MDM_TARGET_B_MODEL}"
TUX_TMUX_SESSION="$session_name" ./scripts/docker-smoke.sh smoke

tmux kill-session -t "$session_name" 2>/dev/null || true
start_tux_tmux
wait_for_tux

hive_file="/tmp/tux-live-hive-${run_id}.txt"
target_b_file="/tmp/tux-live-target-b-${run_id}.txt"
received_file="/tmp/tux-live-received-${run_id}.txt"
scheduled_file="/tmp/tux-live-scheduled-${run_id}.txt"
nonce="target-b-to-a-live-suite-${run_id}"

echo "Hive -> target-a request"
send_prompt "LIVE SUITE ${run_id}: call peers, then send_request to target-a with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${hive_file} containing the exact words hive request ok ${run_id} plus hostname, then respond completed JSON\"}. Report the send status."
sleep "$hive_wait"
verify_file_contains target-a "$hive_file" "hive request ok ${run_id}"

echo "target-b direct command + peer message"
tmux send-keys -t "$session_name" Down Down
send_prompt "LIVE SUITE ${run_id}: use shell to write ${target_b_file} containing target-b direct ok ${run_id}, hostname, pwd, and current date. Then call peers and send_message to target-a with this exact nonce: ${nonce}. Report file content and send status."
sleep "$target_wait"
verify_file_contains target-b "$target_b_file" "target-b direct ok ${run_id}"

echo "target-a receive check"
tmux send-keys -t "$session_name" Up
send_prompt "LIVE SUITE ${run_id}: report whether you received a peer comms message from target-b containing nonce ${nonce}. If present, write ${received_file} containing the nonce and sender, then cat it. Keep the answer concise."
sleep "$receive_wait"
verify_file_contains target-a "$received_file" "$nonce"

echo "target-a scheduler wake"
send_prompt "LIVE SUITE ${run_id}: use the scheduling/reminder capability to wake yourself in about 20 seconds with an instruction to write ${scheduled_file} containing scheduled wake ok ${run_id} plus the current date. After scheduling, report only the schedule status."
sleep "$schedule_wait"
verify_file_contains target-a "$scheduled_file" "scheduled wake ok ${run_id}"

echo "Live TUX suite passed for $run_id"
echo "tmux session left open: $session_name"
