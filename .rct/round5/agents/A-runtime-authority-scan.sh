#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

cd "$ROOT_DIR"

declare -a state_forbidden_patterns=(
  '&& self.turn_state.can_accept(&input);'
  'if self.turn_state.active_run() != Some(run_id) || self.turn_state.cancel_after_boundary() {'
  'match self.turn_state.phase() {'
  'let in_extraction = self.turn_state.in_extraction_flow();'
  'if self.turn_state.has_barrier_ops() {'
  '} else if self.turn_state.in_extraction_flow() {'
  'if !self.turn_state.phase().is_terminal() {'
  'attempts: self.turn_state.extraction_attempts(),'
  '&& !self.turn_state.in_extraction_flow()'
  'if self.turn_state.pending_op_refs().is_none() {'
  'let barrier_ids = self.turn_state.barrier_op_ids();'
  'let outcome = self.turn_state.terminal_outcome();'
  '} => match self.turn_state.primitive_kind() {'
)

declare -a runner_forbidden_patterns=(
  'if let Some(run_id) = self.turn_state.active_run().cloned() {'
  '.or_else(|| self.turn_state.active_run().cloned())'
)

for pattern in "${state_forbidden_patterns[@]}"; do
  if rg -n -F "$pattern" meerkat-core/src/agent/state.rs >/dev/null; then
    echo "Lane A runtime-authority scan failed: found forbidden runtime-backed LocalTurnExecutionState usage in state.rs"
    rg -n -F "$pattern" meerkat-core/src/agent/state.rs || true
    exit 1
  fi
done

for pattern in "${runner_forbidden_patterns[@]}"; do
  if rg -n -F "$pattern" meerkat-core/src/agent/runner.rs >/dev/null; then
    echo "Lane A runtime-authority scan failed: found forbidden runtime-backed LocalTurnExecutionState usage in runner.rs"
    rg -n -F "$pattern" meerkat-core/src/agent/runner.rs || true
    exit 1
  fi
done

echo "Lane A runtime-authority scan passed."
