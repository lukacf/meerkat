# shellcheck shell=bash
# Per-run wall-clock cap shared by the hand-written TLC audits in this
# directory. It matches the canonical machine-verify lane (`TLC_RUN_CAP_SECS`,
# default 900 s, `TlcRunError::Incomplete` in crates/xtask/src/machines.rs): a
# run that hits the cap is killed and fails closed as TLC INCOMPLETE (exit 3),
# never as a pass.
#
# usage, after `source`:
#   tlc_run_capped <slug> <config> <log> <tlc args...>
# Runs `tlc <tlc args...>` with its output in <log> and returns TLC's exit
# status; on the cap it prints the INCOMPLETE line and exits 3.

tlc_run_cap_secs="${TLC_RUN_CAP_SECS:-900}"
if ! [[ "${tlc_run_cap_secs}" =~ ^[0-9]+$ ]] || (( tlc_run_cap_secs < 1 )); then
  echo "error: TLC_RUN_CAP_SECS must be whole seconds >= 1, got '${tlc_run_cap_secs}'" >&2
  exit 2
fi

tlc_run_capped() {
  local slug="$1" config="$2" log="$3"
  shift 3
  local capped="${log}.capped"
  rm -f "${capped}"
  tlc "$@" > "${log}" 2>&1 &
  local tlc_pid=$!
  # The watchdog ends TLC (which execs java) when the cap elapses first, and
  # is itself ended, with its sleep, when TLC exits first.
  (
    trap 'kill "${sleep_pid}" 2>/dev/null; exit 0' TERM
    sleep "${tlc_run_cap_secs}" &
    sleep_pid=$!
    wait "${sleep_pid}"
    : > "${capped}"
    kill -TERM "${tlc_pid}" 2>/dev/null
  ) &
  local watchdog_pid=$!
  local status=0
  wait "${tlc_pid}" || status=$?
  kill -TERM "${watchdog_pid}" 2>/dev/null || true
  wait "${watchdog_pid}" 2>/dev/null || true
  if [[ -e "${capped}" ]]; then
    rm -f "${capped}"
    echo "TLC INCOMPLETE for ${slug} (${config}): hit the ${tlc_run_cap_secs} s per-run cap and was killed; an unexhausted state space is not a pass" >&2
    exit 3
  fi
  return "${status}"
}
