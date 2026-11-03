#!/usr/bin/env bash
set -euo pipefail

pack_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
examples_dir="$(cd "${pack_dir}/.." && pwd)"
repo_root="$(cd "${examples_dir}/.." && pwd)"

# Meerkat 0.7's machine-authority code allocates huge debug-build stack
# frames. The mdm_mob_target binary sizes its own threads; this export is
# belt and braces because this script launches the prebuilt debug binary
# directly (bypassing the workspace .cargo/config.toml [env] section).
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"

pick_loopback_port() {
  node -e 'const net=require("node:net"); const s=net.createServer(); s.listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close();});'
}

supervisor_bind="${MDM_REAL_TARGET_SMOKE_SUPERVISOR_BIND:-127.0.0.1:$(pick_loopback_port)}"
real_target_e2e="${MDM_REAL_TARGET_E2E:-0}"
target_count="${MDM_REAL_TARGET_COUNT:-1}"
if [[ ! "$target_count" =~ ^[1-9][0-9]*$ ]]; then
  echo "MDM_REAL_TARGET_COUNT must be a positive integer" >&2
  exit 2
fi
if [[ "$target_count" -gt 1 && -n "${MDM_REAL_TARGET_SMOKE_LISTEN:-}" ]]; then
  echo "MDM_REAL_TARGET_SMOKE_LISTEN is only valid with one target" >&2
  exit 2
fi
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/mdm-real-target-smoke.XXXXXX")"
declare -a target_pids=()
declare -a binding_files=()
declare -a target_logs=()

cleanup() {
  for target_pid in "${target_pids[@]}"; do
    kill "$target_pid" 2>/dev/null || true
    wait "$target_pid" 2>/dev/null || true
  done
  if [[ "${MDM_KEEP_TMP:-0}" == "1" ]]; then
    echo "[mdm-real-target-smoke] retained state at ${tmp_dir}" >&2
  else
    rm -rf "$tmp_dir"
  fi
}
trap cleanup EXIT

targets_file="${tmp_dir}/targets.json"

cd "$repo_root"
./scripts/repo-cargo build -p meerkat-meta --example mdm_mob_target
target_bin="$(./scripts/repo-cargo --print-env | awk -F= '$1 == "CARGO_TARGET_DIR" { print $2 }')/debug/examples/mdm_mob_target"

for target_index in $(seq 1 "$target_count"); do
  target_id="target-smoke-${target_index}"
  target_listen="${MDM_REAL_TARGET_SMOKE_LISTEN:-127.0.0.1:$(pick_loopback_port)}"
  binding_file="${tmp_dir}/${target_id}.json"
  target_data_dir="${tmp_dir}/${target_id}-state"
  target_log="${tmp_dir}/${target_id}.log"
  "$target_bin" \
    --id "$target_id" \
    --name "$target_id" \
    --listen "$target_listen" \
    --data-dir "$target_data_dir" \
    --binding-out "$binding_file" \
    --model "${MDM_TARGET_MODEL:-gpt-5.5}" \
    --provider "${MDM_TARGET_PROVIDER:-openai}" \
    >"$target_log" 2>&1 &
  target_pids+=("$!")
  binding_files+=("$binding_file")
  target_logs+=("$target_log")
done

for binding_file in "${binding_files[@]}"; do
  for _ in {1..120}; do
    [[ -s "$binding_file" ]] && break
    sleep 0.5
  done
  if [[ ! -s "$binding_file" ]]; then
    echo "[mdm-real-target-smoke] target did not write ${binding_file}" >&2
    for target_log in "${target_logs[@]}"; do
      tail -100 "$target_log" >&2 || true
    done
    exit 1
  fi
done
node "${pack_dir}/scripts/merge-bindings.mjs" "$targets_file" "${binding_files[@]}" >/dev/null

cd "$examples_dir"
console_args=(
  --targets "$targets_file"
  --state-dir "${tmp_dir}/console-state"
  --real-target-smoke
  --smoke
)
if [[ "$real_target_e2e" == "1" ]]; then
  console_args+=(--real-target-e2e --real-llm)
else
  console_args+=(--demo-llm)
fi
set +e
MDM_SUPERVISOR_BIND_ADDRESS="$supervisor_bind" \
MDM_SUPERVISOR_ADVERTISED_ADDRESS="tcp://${supervisor_bind}" \
npx tsx 004-mdm-console-pack/run.ts \
  "${console_args[@]}"
run_status=$?
set -e

if [[ "$run_status" -ne 0 ]]; then
  echo "[mdm-real-target-smoke] console run failed; target logs follow" >&2
  for target_log in "${target_logs[@]}"; do
    tail -100 "$target_log" >&2 || true
  done
else
  for target_log in "${target_logs[@]}"; do
    peer_seen=""
    for _ in {1..60}; do
      if grep -q "\\[mdm-target\\] peer turn accepted:" "$target_log"; then
        peer_seen="yes"
        break
      fi
      sleep 0.5
    done
    if [[ -z "$peer_seen" ]]; then
      echo "[mdm-real-target-smoke] a target never observed a peer turn: ${target_log}" >&2
      tail -100 "$target_log" >&2 || true
      exit 1
    fi
  done
fi

exit "$run_status"
