#!/usr/bin/env bash
# Resource readings for a CI step, and the runner's size.
#
#   scripts/ci-resource-readings.sh COMMAND [ARGUMENT...]
#   scripts/ci-resource-readings.sh --size
#
# The first form runs COMMAND and, while it runs, prints one "resource
# reading" line every 30 seconds into the calling step's own log: memory
# available, swap used, memory pressure, the root file system's free space
# (and the target directory's file system when it is another one) and
# resident memory summed under fixed labels. A process name is only compared
# with a fixed list and never printed; no line holds arguments, environment
# values or paths, and nothing of the environment is listed. At most 200
# samples (the first 100 minutes of a step), each command of the sampler
# under its own time limit. The readings go to the step's log and nowhere
# else: a step whose runner is lost leaves its last readings only where its
# log is kept. Readings only: nothing is cleaned, retried or resized, and no
# cause is claimed; a gap in the lines is missing evidence, not a diagnosis.
# The sampler's failures are ignored and the exit status is COMMAND's. The
# sampler waits in one-second slices and leaves as soon as its parent is no
# longer this script's shell, so it outlives that shell by about a second at
# most.
#
# The second form prints the runner's size once (processors, memory, disk,
# swap), the lines the steps that add swap print first. It adds no swap.
#
# One implementation for every step that wants the readings: Meta CI's unit
# build (meta-ci.yml, "Build the lib test archive") and the build steps of
# its extension-state and voice lib jobs. Each step starts its own sampler: a
# sampler started in one step does not print into a later step's log.
# meta/scripts/test_ci_workflow.py runs this script.
if [[ "$#" -eq 1 && "$1" == "--size" ]]; then
  show_resources() {
    nproc
    free -m
    if [[ -d /mnt ]]; then
      df -h / /mnt
    else
      df -h /
    fi
    swapon --show || true
  }
  show_resources
  exit
fi
if [[ "$#" -eq 0 || "$1" == -* ]]; then
  echo "usage: ci-resource-readings.sh COMMAND [ARGUMENT...] | --size" >&2
  exit 2
fi
set +e
(
  export TZ=UTC
  trap 'kill "${nap:-}" 2>/dev/null; exit 0' TERM
  place="${CARGO_TARGET_DIR}"
  while [[ "${place}" == */* && ! -e "${place}" ]]; do place="${place%/*}"; done
  if [[ ! -e "${place}" ]]; then place=/; fi
  root_device="$(exec timeout 10 stat -c %d /)"
  target_device="$(exec timeout 10 stat -c %d -- "${place}")"
  distinct=""
  if [[ "${root_device}" =~ ^[0-9]+$ && "${target_device}" =~ ^[0-9]+$ && "${root_device}" != "${target_device}" ]]; then
    distinct=yes
  fi
  for ((sample = 1; sample <= 200; sample++)); do
    total="" available="" swap_total="" swap_free=""
    while read -r key value _; do
      case "${key}" in
        MemTotal:) total="${value}" ;;
        MemAvailable:) available="${value}" ;;
        SwapTotal:) swap_total="${value}" ;;
        SwapFree:) swap_free="${value}" ;;
      esac
    done < /proc/meminfo
    memory="unavailable"
    if [[ "${total}" =~ ^[0-9]+$ && "${available}" =~ ^[0-9]+$ ]]; then
      memory="$((available / 1024)) MiB available of $((total / 1024)) MiB"
    fi
    swap="unavailable"
    if [[ "${swap_total}" =~ ^[0-9]+$ && "${swap_free}" =~ ^[0-9]+$ ]]; then
      swap="$(((swap_total - swap_free) / 1024)) MiB used of $((swap_total / 1024)) MiB"
    fi
    pressure="unavailable"
    while read -r kind average _; do
      if [[ "${kind}" == full && "${average}" =~ ^avg10=[0-9.]+$ ]]; then pressure="${average#avg10=}"; fi
    done < /proc/pressure/memory
    root="unavailable"
    if [[ "$(exec timeout 10 stat -f -c '%a %b %S' /)" =~ ^([0-9]+)\ ([0-9]+)\ ([0-9]+)$ ]]; then
      root="$((BASH_REMATCH[1] * BASH_REMATCH[3] / 1048576)) MiB free of $((BASH_REMATCH[2] * BASH_REMATCH[3] / 1048576)) MiB"
    fi
    target=""
    if [[ -n "${distinct}" ]]; then
      target="; target filesystem unavailable"
      if [[ "$(exec timeout 10 stat -f -c '%a %b %S' -- "${place}")" =~ ^([0-9]+)\ ([0-9]+)\ ([0-9]+)$ ]]; then
        target="; target filesystem $((BASH_REMATCH[1] * BASH_REMATCH[3] / 1048576)) MiB free of $((BASH_REMATCH[2] * BASH_REMATCH[3] / 1048576)) MiB"
      fi
    fi
    compiler=0 linker=0 cache=0 build=0 runner=0 other=0 counted=""
    for entry in /proc/[0-9]*/status; do
      name="" resident=""
      while read -r key value rest; do
        case "${key}" in
          Name:) name="${value}${rest:+ ${rest}}" ;;
          VmRSS:)
            resident="${value}"
            break
            ;;
        esac
      done < "${entry}"
      if [[ ! "${resident}" =~ ^[0-9]+$ ]]; then continue; fi
      counted=yes
      case "${name}" in
        rustc) compiler=$((compiler + resident)) ;;
        ld | mold | ld.lld | lld | collect2 | cc | gcc) linker=$((linker + resident)) ;;
        sccache) cache=$((cache + resident)) ;;
        cargo) build=$((build + resident)) ;;
        cargo-nextest) runner=$((runner + resident)) ;;
        *) other=$((other + resident)) ;;
      esac
    done
    sums="unavailable"
    if [[ -n "${counted}" ]]; then
      sums="compiler $((compiler / 1024)) MiB, linker $((linker / 1024)) MiB, compile cache $((cache / 1024)) MiB"
      sums+=", build tool $((build / 1024)) MiB, test runner $((runner / 1024)) MiB, other $((other / 1024)) MiB"
    fi
    printf -v now '%(%H:%M:%SZ)T' -1
    printf 'resource reading [%s] %d: memory %s; swap %s; memory pressure full avg10 %s; root filesystem %s%s; resident: %s\n' \
      "${now}" "${sample}" "${memory}" "${swap}" "${pressure}" "${root}" "${target}" "${sums}"
    for ((slice = 0; slice < 30; slice++)); do
      parent=""
      read -r _ _ _ parent _ < "/proc/${BASHPID}/stat"
      if [[ "${parent:-$$}" != "$$" ]] || ! kill -0 "$$"; then exit 0; fi
      sleep 1 &
      nap=$!
      wait "${nap}" || exit 0
    done
  done
) 2>/dev/null &
sampler=$!
"$@"
status=$?
kill "${sampler}" 2>/dev/null
pkill -P "${sampler}" 2>/dev/null
wait "${sampler}" 2>/dev/null
exit "${status}"
