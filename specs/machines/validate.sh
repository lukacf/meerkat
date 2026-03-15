#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
META="$ROOT/.tlc"

machines=(
  runtime_ingress
  runtime_control
  input_lifecycle
  peer_comms
  external_tool_surface
  turn_execution
  ops_lifecycle
  mob_orchestrator
  mob_lifecycle
  flow_run
)

mkdir -p "$META"

for machine in "${machines[@]}"; do
  mkdir -p "$META/$machine"
  echo "==> Checking $machine"
  tlc \
    -metadir "$META/$machine" \
    -config "$ROOT/$machine/ci.cfg" \
    "$ROOT/$machine/model.tla"
  echo
done

echo "All checked-in machine specs validated."
