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
#   tlc_run_cap_reap
# Ends a run still in flight (TLC and its watchdog); call it from the audit's
# EXIT trap so an interrupted or killed lane leaves nothing running.

tlc_run_cap_secs="${TLC_RUN_CAP_SECS:-900}"
if ! [[ "${tlc_run_cap_secs}" =~ ^[0-9]+$ ]] || (( tlc_run_cap_secs < 1 )); then
  echo "error: TLC_RUN_CAP_SECS must be whole seconds >= 1, got '${tlc_run_cap_secs}'" >&2
  exit 2
fi

tlc_run_cap_tlc_pid=""
tlc_run_cap_watchdog_pgid=""

tlc_run_cap_reap() {
  if [[ -n "${tlc_run_cap_tlc_pid}" ]]; then
    kill -TERM "${tlc_run_cap_tlc_pid}" 2>/dev/null || true
  fi
  if [[ -n "${tlc_run_cap_watchdog_pgid}" ]]; then
    kill -TERM -- "-${tlc_run_cap_watchdog_pgid}" 2>/dev/null || true
  fi
  tlc_run_cap_tlc_pid=""
  tlc_run_cap_watchdog_pgid=""
}

tlc_run_capped() {
  local slug="$1" config="$2" log="$3"
  shift 3
  local capped="${log}.capped"
  rm -f "${capped}"
  tlc "$@" > "${log}" 2>&1 &
  local tlc_pid=$!
  # The watchdog ends TLC (which execs java) when the cap elapses first, and
  # is itself ended, with its sleep, when TLC exits first. It starts as its
  # own process group (job control is on only while it is launched), so
  # ending the group ends the sleep too: there is no moment at which the
  # sleep runs but cannot be named. Its stdio is /dev/null, so nothing it
  # leaves behind can hold a caller's pipe (for example a `$(...)` capture)
  # open.
  local job_control_was_on=false
  [[ $- == *m* ]] && job_control_was_on=true
  set -m
  (
    sleep "${tlc_run_cap_secs}"
    : > "${capped}"
    kill -TERM "${tlc_pid}" 2>/dev/null
  ) </dev/null >/dev/null 2>&1 &
  local watchdog_pgid=$!
  [[ "${job_control_was_on}" == true ]] || set +m
  tlc_run_cap_tlc_pid="${tlc_pid}"
  tlc_run_cap_watchdog_pgid="${watchdog_pgid}"
  local status=0
  wait "${tlc_pid}" || status=$?
  kill -TERM -- "-${watchdog_pgid}" 2>/dev/null || true
  wait "${watchdog_pgid}" 2>/dev/null || true
  tlc_run_cap_tlc_pid=""
  tlc_run_cap_watchdog_pgid=""
  if [[ -e "${capped}" ]]; then
    rm -f "${capped}"
    echo "TLC INCOMPLETE for ${slug} (${config}): hit the ${tlc_run_cap_secs} s per-run cap and was killed; an unexhausted state space is not a pass" >&2
    exit 3
  fi
  return "${status}"
}
