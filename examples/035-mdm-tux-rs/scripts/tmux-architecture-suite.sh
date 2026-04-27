#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
example_dir="$(cd "${script_dir}/.." && pwd)"
cd "$example_dir"

compose_file="${COMPOSE_FILE:-docker-compose.tux.yml}"
session_name="${TUX_TMUX_SESSION:-mdm-tux-architecture-suite}"
run_id="${TUX_ARCH_RUN_ID:-$(date +%Y%m%d%H%M%S)}"

phase_wait="${TUX_ARCH_PHASE_WAIT_SECONDS:-150}"
schedule_wait="${TUX_ARCH_SCHEDULE_WAIT_SECONDS:-180}"
peer_request_wait="${TUX_ARCH_PEER_REQUEST_WAIT_SECONDS:-150}"
restart_wait="${TUX_ARCH_RESTART_WAIT_SECONDS:-90}"

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
      echo "unsupported provider for architecture suite: $1" >&2
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
    echo "docker-architecture-suite needs a real ${key_var} for ${label} (${provider})" >&2
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

claim_selected() {
  local target="$1"
  local deadline=$((SECONDS + 45))
  local pane
  send_prompt "/claim"
  while (( SECONDS < deadline )); do
    pane="$(tmux capture-pane -t "$session_name" -p 2>/dev/null || true)"
    if [[ "$pane" == *"[${target}] >"* ]]; then
      wait_for_target_ready "$target"
      return 0
    fi
    sleep 2
  done
  tmux capture-pane -t "$session_name" -p -S -120 2>/dev/null || true
  echo "TUX did not claim ${target}" >&2
  exit 1
}

press_keys() {
  local key
  for key in "$@"; do
    tmux send-keys -t "$session_name" "$key"
    sleep 0.15
  done
}

wait_for_target_ready() {
  local target="$1"
  local deadline=$((SECONDS + 90))
  while (( SECONDS < deadline )); do
    if target_ready_now "$target"; then
      return 0
    fi
    sleep 2
  done
  tmux capture-pane -t "$session_name" -p -S -120 2>/dev/null || true
  echo "TUX did not become ready for ${target}" >&2
  exit 1
}

target_ready_now() {
  local target="$1"
  tmux capture-pane -t "$session_name" -p 2>/dev/null | grep -q "Ready to send to ${target}"
}

select_with_keys() {
  local target="$1"
  shift
  local deadline=$((SECONDS + 90))

  while (( SECONDS < deadline )); do
    press_keys "$@"
    sleep 1
    if target_ready_now "$target"; then
      return 0
    fi
    sleep 2
  done

  tmux capture-pane -t "$session_name" -p -S -120 2>/dev/null || true
  echo "TUX did not become ready for ${target}" >&2
  exit 1
}

select_hive() {
  select_with_keys "hive" Up Up Up Up
}

select_target_a() {
  select_with_keys "target-a" Up Up Up Up Down
}

select_target_b() {
  select_with_keys "target-b" Up Up Up Up Down Down
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

wait_for_log() {
  local service="$1"
  local pattern="$2"
  local timeout_sec="${3:-90}"
  local start
  local logs
  start="$(date +%s)"

  while true; do
    logs="$("${compose[@]}" logs --no-color "$service" 2>&1 || true)"
    if [[ "$logs" =~ $pattern ]]; then
      return 0
    fi
    if (( "$(date +%s)" - start >= timeout_sec )); then
      echo "timed out waiting for ${service} log pattern: ${pattern}" >&2
      printf '%s\n' "$logs" >&2
      return 1
    fi
    sleep 2
  done
}

restart_target_b() {
  echo "restarting target-b to force kennel/peer churn"
  "${compose[@]}" restart target-b
  wait_for_log target-b 'session ready' "$restart_wait"
  wait_for_log target-b 'added hive as trusted peer' "$restart_wait"
}

require docker
require tmux
default_live_provider_matrix
require_live_provider_keys

echo "Starting Docker TUX architecture suite run $run_id"
echo "Provider matrix:"
echo "  hive=${MDM_HIVE_PROVIDER}/${MDM_HIVE_MODEL}"
echo "  target-a=${MDM_TARGET_A_PROVIDER}/${MDM_TARGET_A_MODEL}"
echo "  target-b=${MDM_TARGET_B_PROVIDER}/${MDM_TARGET_B_MODEL}"
TUX_TMUX_SESSION="$session_name" ./scripts/docker-smoke.sh smoke

tmux kill-session -t "$session_name" 2>/dev/null || true
start_tux_tmux
wait_for_tux
sleep 8

relay_a_file="/tmp/tux-arch-relay-a-${run_id}.txt"
relay_b_file="/tmp/tux-arch-relay-b-${run_id}.txt"
post_restart_file="/tmp/tux-arch-post-restart-b-${run_id}.txt"
cascade_a_file="/tmp/tux-arch-cascade-a-${run_id}.txt"
cascade_b_file="/tmp/tux-arch-cascade-b-${run_id}.txt"
race_file="/tmp/tux-arch-claim-race-${run_id}.txt"
debate_a_file="/tmp/tux-arch-debate-a-${run_id}.txt"
debate_b_file="/tmp/tux-arch-debate-b-${run_id}.txt"
debate_hive_file="/tmp/tux-arch-debate-hive-${run_id}.txt"
relay_nonce="relay-churn-${run_id}"
cascade_nonce="cascade-wake-${run_id}"
race_nonce="claim-race-${run_id}"
debate_nonce="debate-loop-${run_id}"

echo "Test 1/4: peer relay and target restart"
select_target_b
claim_selected "target-b"
send_prompt "ARCH SUITE ${run_id} TEST 1: use shell to write ${relay_b_file} containing relay source ok ${run_id}, hostname, and pwd. Then call peers and send_request to target-a with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${relay_a_file} containing relay target ok ${run_id}, the nonce ${relay_nonce}, hostname, and the fact that target-b delegated this work\"}. Report the file content and send status."
sleep "$phase_wait"
verify_file_contains target-b "$relay_b_file" "relay source ok ${run_id}"
verify_file_contains target-a "$relay_a_file" "$relay_nonce"
restart_target_b
select_hive
send_prompt "ARCH SUITE ${run_id} TEST 1B: call peers after the target-b restart, then send_request to target-b with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${post_restart_file} containing post restart ok ${run_id}, hostname, pwd, and current date\"}. Report the send status."
sleep "$phase_wait"
verify_file_contains target-b "$post_restart_file" "post restart ok ${run_id}"

echo "Test 2/4: scheduler cascade with peer wake"
select_target_a
claim_selected "target-a"
send_prompt "ARCH SUITE ${run_id} TEST 2: use scheduling/reminder capability to wake yourself in about 20 seconds. The wake instruction is: write ${cascade_a_file} containing cascade scheduled ok ${run_id} and current date, then call peers and send_request to target-b with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${cascade_b_file} containing cascade received ok ${run_id}, nonce ${cascade_nonce}, sender target-a, hostname, and current date\"}. After scheduling, report only the schedule status."
sleep "$schedule_wait"
verify_file_contains target-a "$cascade_a_file" "cascade scheduled ok ${run_id}"
sleep "$peer_request_wait"
verify_file_contains target-b "$cascade_b_file" "$cascade_nonce"

echo "Test 3/4: claim/release pressure while hive comms stays live"
select_hive
send_prompt "ARCH SUITE ${run_id} TEST 3: call peers, then send_request to target-a with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${race_file} containing hive during claim pressure ok ${run_id}, nonce ${race_nonce}, hostname, and current date\"}. Report status only."
sleep 10
select_target_a
send_prompt "/claim"
sleep 3
send_prompt "/release"
sleep 3
send_prompt "/claim"
sleep "$phase_wait"
verify_file_contains target-a "$race_file" "$race_nonce"

echo "Test 4/4: multi-provider arbitration and transcript files"
select_hive
send_prompt "ARCH SUITE ${run_id} TEST 4A: call peers, then send_request to target-a with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${debate_a_file} containing debate target-a fact ${run_id}, provider openai, nonce ${debate_nonce}, hostname, and pwd\"}. Then send_request to target-b with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${debate_b_file} containing debate target-b fact ${run_id}, provider anthropic, nonce ${debate_nonce}, hostname, and pwd\"}. Report both send statuses."
sleep "$phase_wait"
verify_file_contains target-a "$debate_a_file" "$debate_nonce"
sleep "$phase_wait"
verify_file_contains target-b "$debate_b_file" "$debate_nonce"
select_hive
send_prompt "ARCH SUITE ${run_id} TEST 4B: call peers, then send_request to target-a with handling_mode steer, intent \"execute_task\", params {\"instruction\":\"write ${debate_hive_file} containing hive arbitration ok ${run_id}, nonce ${debate_nonce}, and one line naming target-a and target-b\"}. Report final status."
sleep "$phase_wait"
verify_file_contains target-a "$debate_hive_file" "$debate_nonce"

echo "Architecture TUX suite passed for $run_id"
echo "tmux session left open: $session_name"
