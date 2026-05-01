#!/usr/bin/env bash
set -euo pipefail

self_script="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")"

if ! command -v rg >/dev/null 2>&1; then
  echo "effect-authority audit failed: required command 'rg' (ripgrep) not found" >&2
  exit 127
fi

if [[ "${1:-}" == "--self-test" ]]; then
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  mkdir -p "$tmpdir/meerkat-runtime/src/meerkat_machine"
  cat >"$tmpdir/meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs" <<'EOF'
fn bad(machine: &Machine) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad");
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: peer hard-cancel fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-runtime/src"
  cat >"$tmpdir/meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad() {
    let _ = RuntimeEffect::cancel_after_boundary("bad");
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: direct RuntimeEffect constructor fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-runtime/src"
  cat >"$tmpdir/meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad(reason: String) {
    let _ = RuntimeEffectFact::CancelAfterBoundary { reason };
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: runtime-shell fact literal fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-runtime/src"
  cat >"$tmpdir/meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad(reason: String) {
    let _ = MeerkatMachineEffect::RuntimeEffectFact {
        kind: RuntimeEffectKind::CancelAfterBoundary,
        reason,
    };
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: generated runtime-effect fact fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-runtime/src"
  cat >"$tmpdir/meerkat-runtime/src/user_interrupt.rs" <<'EOF'
impl Machine {
    pub async fn hard_cancel_current_run(&self) {
        let authority = UserInterruptAuthority::new();
        self.hard_cancel_current_run_authorized(authority).await;
    }
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: public hard-cancel authority fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-runtime/src"
  cat >"$tmpdir/meerkat-runtime/src/user_interrupt.rs" <<'EOF'
impl Machine {
    pub async fn hard_cancel_current_run(&self) {
        let handle = self.interrupt_handle_for(&session_id).await.unwrap();
        handle.hard_cancel_current_run("bad".to_string()).await.unwrap();
    }

    pub(crate) async fn hard_cancel_current_run_authorized(&self) {
        let handle = self.interrupt_handle_for(&session_id).await.unwrap();
        handle.hard_cancel_current_run("allowed".to_string()).await.unwrap();
    }

    async fn interrupt_handle_for(&self) {}
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: public hard-cancel live-handle fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-rpc/src"
  cat >"$tmpdir/meerkat-rpc/src/session_runtime.rs" <<'EOF'
impl SessionRuntime {
    pub async fn interrupt(&self, session_id: &SessionId) {
        let _ = self.service.interrupt(session_id).await;
    }
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: public session-runtime interrupt fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-rpc/src"
  cat >"$tmpdir/meerkat-rpc/src/session_executor.rs" <<'EOF'
impl CoreExecutorInterruptHandle for SessionRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self) {
        let _ = self.runtime.interrupt(&self.session_id).await;
    }
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: recursive RPC interrupt-handle fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-runtime/src"
  cat >"$tmpdir/meerkat-runtime/src/comms_drain.rs" <<'EOF'
async fn bad(machine: Machine, session_id: SessionId) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad").await;
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: comms-drain hard-cancel fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-rest/src"
  cat >"$tmpdir/meerkat-rest/src/lib.rs" <<'EOF'
async fn public_interrupt(service: Service, session_id: SessionId) {
    let _ = service.interrupt(&session_id).await;
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: public surface interrupt fixture passed" >&2
    exit 1
  fi

  rm -rf "$tmpdir"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT
  mkdir -p "$tmpdir/meerkat-rpc/src"
  cat >"$tmpdir/meerkat-rpc/src/realtime_ws.rs" <<'EOF'
async fn public_interrupt(adapter: Adapter, session_id: SessionId) {
    let _ = adapter.interrupt_current_run(&session_id).await;
}
EOF
  if "$self_script" "$tmpdir" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: public interrupt_current_run fixture passed" >&2
    exit 1
  fi

  (cd /tmp && "$self_script" >/dev/null 2>&1) || {
    echo "audit-effect-authority self-test failed: absolute script invocation outside repo failed" >&2
    exit 1
  }

  echo "audit-effect-authority self-test passed"
  exit 0
fi

root="${1:-}"
if [[ -z "$root" ]]; then
  script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
  root="$(cd "$script_dir/.." && pwd)"
fi

failures=0

report_matches() {
  local label="$1"
  local matches="$2"
  if [[ -n "$matches" ]]; then
    echo "effect-authority audit failed: $label" >&2
    echo "$matches" >&2
    failures=$((failures + 1))
  fi
}

run_rg() {
  local pattern="$1"
  shift
  (cd "$root" && rg -n "$pattern" . --glob '!target/**' --glob '!scripts/audit-effect-authority.sh' "$@") 2>/dev/null || true
}

strip_core_executor_interrupt_impls() {
  awk '
    function brace_delta(line, opened, closed, copy) {
      copy = line
      opened = gsub(/\{/, "{", copy)
      copy = line
      closed = gsub(/\}/, "}", copy)
      return opened - closed
    }
    /impl[[:space:]][^{]*CoreExecutorInterruptHandle[[:space:]]+for[[:space:]]/ {
      in_impl = 1
      depth = 0
      saw_open = 0
    }
    in_impl {
      if (index($0, "{") > 0) {
        saw_open = 1
      }
      depth += brace_delta($0)
      if (saw_open && depth <= 0) {
        in_impl = 0
        depth = 0
        saw_open = 0
      }
      next
    }
    { print }
  ' "$1"
}

strip_cfg_test_modules() {
  awk '
    function brace_delta(line, opened, closed, copy) {
      copy = line
      opened = gsub(/\{/, "{", copy)
      copy = line
      closed = gsub(/\}/, "}", copy)
      return opened - closed
    }
    /^[[:space:]]*#\[cfg\(test\)\]/ {
      pending_test_attr = 1
      print
      next
    }
    pending_test_attr && /^[[:space:]]*mod[[:space:]]+tests[[:space:]]*\{/ {
      in_test = 1
      pending_test_attr = 0
      depth = brace_delta($0)
      next
    }
    pending_test_attr && /^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?(fn|struct|enum|impl|trait|type|const|static|use)[[:space:]]/ {
      pending_test_attr = 0
    }
    in_test {
      depth += brace_delta($0)
      if (depth <= 0) {
        in_test = 0
        depth = 0
      }
      next
    }
    { print }
  '
}

run_control_name="RunControl""Command"
core_control_name="CoreExecutor""Control"
report_matches "$run_control_name references remain" "$(run_rg "\\b${run_control_name}\\b")"
report_matches "$core_control_name references remain" "$(run_rg "\\b${core_control_name}\\b")"

peer_matches=""
for peer_file in \
  "$root/meerkat-runtime/src/meerkat_machine/dispatch_control.rs" \
  "$root/meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs"
do
  if [[ -f "$peer_file" ]]; then
    found="$(rg -n '\b(hard_cancel_current_run|interrupt_handle|interrupt_handle_for)\b|runtime\.interrupt\(|session_service\.interrupt\(' "$peer_file" 2>/dev/null || true)"
    if [[ -n "$found" ]]; then
      peer_matches+="$found"$'\n'
    fi
  fi
done
while IFS= read -r peer_file; do
  [[ -z "$peer_file" ]] && continue
  found="$(rg -n '\b(hard_cancel_current_run|interrupt_handle|interrupt_handle_for)\b|runtime\.interrupt\(|session_service\.interrupt\(' "$root/$peer_file" 2>/dev/null || true)"
  if [[ -n "$found" ]]; then
    peer_matches+="$found"$'\n'
  fi
done < <(cd "$root" && rg --files 2>/dev/null | rg '(^|/)peer_admission[^/]*\.rs$|(^|/)peer_admission/' || true)
report_matches "peer-admission code can reach hard interrupt authority" "$peer_matches"

if [[ -f "$root/meerkat-runtime/src/comms_drain.rs" ]]; then
  comms_drain_matches="$(rg -n '\b(hard_cancel_current_run|interrupt_handle|interrupt_handle_for)\b|\.interrupt_current_run\(|\b(runtime|adapter|session_service)\.interrupt\(' "$root/meerkat-runtime/src/comms_drain.rs" 2>/dev/null || true)"
  report_matches "comms-drain code can reach hard interrupt authority" "$comms_drain_matches"
fi

if [[ -f "$root/meerkat-rpc/src/session_executor.rs" ]]; then
  rpc_executor_recursion="$(rg -n '\.runtime\.interrupt\(' "$root/meerkat-rpc/src/session_executor.rs" 2>/dev/null || true)"
  report_matches "RPC executor interrupt handle must not re-enter public SessionRuntime::interrupt" "$rpc_executor_recursion"
fi

if [[ -f "$root/meerkat-runtime/src/user_interrupt.rs" ]]; then
  strip_authorized_interrupt_body() {
    awk '
      function brace_delta(line, opened, closed, copy) {
        copy = line
        opened = gsub(/\{/, "{", copy)
        copy = line
        closed = gsub(/\}/, "}", copy)
        return opened - closed
      }
      /pub\(crate\)[[:space:]]+async[[:space:]]+fn[[:space:]]+hard_cancel_current_run_authorized[[:space:]]*\(/ {
        in_authorized = 1
        depth = 0
        saw_open = 0
      }
      in_authorized {
        if (index($0, "{") > 0) {
          saw_open = 1
        }
        depth += brace_delta($0)
        if (saw_open && depth <= 0) {
          in_authorized = 0
          depth = 0
          saw_open = 0
        }
        next
      }
      { print }
    ' "$1"
  }
  public_interrupt_bypass="$(strip_authorized_interrupt_body "$root/meerkat-runtime/src/user_interrupt.rs" \
    | rg -n 'self\.hard_cancel_current_run_authorized\(|UserInterruptAuthority::new\(\)|\.hard_cancel_current_run\(|\binterrupt_handle_for\(' \
    | rg -v 'fn[[:space:]]+interrupt_handle_for[[:space:]]*\(' 2>/dev/null || true)"
  report_matches "public user-interrupt API must route through the command/DSL path" "$public_interrupt_bypass"
fi

authority_mints="$(run_rg 'UserInterruptAuthority::new\(\)' --glob '!meerkat-runtime/src/meerkat_machine/runtime_control.rs')"
report_matches "UserInterruptAuthority may only be minted by the command-owned interrupt path" "$authority_mints"

public_interrupt_bypasses=""
for surface_file in \
  "$root/meerkat-rest/src/lib.rs" \
  "$root/meerkat-mcp-server/src/lib.rs" \
  "$root/meerkat-rpc/src/handlers/session.rs" \
  "$root/meerkat-rpc/src/handlers/turn.rs" \
  "$root/meerkat-rpc/src/realtime_ws.rs" \
  "$root/meerkat-rpc/src/session_runtime.rs" \
  "$root/meerkat-cli/src/main.rs" \
  "$root/meerkat-openai/src/realtime_attachment.rs" \
  "$root/meerkat-mob/src/runtime/local_bridge.rs" \
  "$root/meerkat-mob/src/runtime/provisioner.rs"
do
  if [[ -f "$surface_file" ]]; then
    found="$(strip_core_executor_interrupt_impls "$surface_file" \
      | strip_cfg_test_modules \
      | rg -n '\b(runtime|service|svc|session_service|cancel_svc|self\.service|self\.session_service)\.interrupt\(|session_service\(\)\.interrupt\(|\.interrupt_current_run\(' 2>/dev/null || true)"
    if [[ -n "$found" ]]; then
      public_interrupt_bypasses+="$surface_file"$'\n'"$found"$'\n'
    fi
  fi
done
report_matches "public surface interrupt paths must route through MeerkatMachine::hard_cancel_current_run" "$public_interrupt_bypasses"

direct_interrupt_current_run="$(run_rg '\.interrupt_current_run\(' \
  --glob '!meerkat-runtime/src/meerkat_machine/session_management.rs' \
  --glob '!meerkat-runtime/src/meerkat_machine_tests.rs' \
  --glob '!meerkat-runtime/tests/**' \
  --glob '!**/tests/**' \
  --glob '!**/*tests.rs')"
report_matches "direct interrupt_current_run callsites are forbidden outside runtime tests" "$direct_interrupt_current_run"

report_matches "direct RuntimeEffect constructor helpers are forbidden" \
  "$(run_rg 'RuntimeEffect::(cancel_after_boundary|stop_runtime_executor)\b')"

runtime_effect_assoc="$(run_rg 'RuntimeEffect::[A-Za-z_][A-Za-z0-9_]*' --glob '!**/effect.rs')"
runtime_effect_assoc="$(printf '%s\n' "$runtime_effect_assoc" | rg -v 'RuntimeEffect::from_fact' || true)"
report_matches "RuntimeEffect associated constructors must go through from_fact" "$runtime_effect_assoc"

fact_literals=""
if [[ -d "$root/meerkat-runtime/src" ]]; then
  while IFS= read -r runtime_file; do
    [[ -z "$runtime_file" ]] && continue
    runtime_path="$root/$runtime_file"
    found="$(awk '
      function brace_delta(line, opened, closed, copy) {
        copy = line
        opened = gsub(/\{/, "{", copy)
        copy = line
        closed = gsub(/\}/, "}", copy)
        return opened - closed
      }
      /^[[:space:]]*#\[cfg\(test\)\]/ {
        pending_test_attr = 1
        next
      }
      pending_test_attr && /^[[:space:]]*mod[[:space:]]+tests[[:space:]]*\{/ {
        in_test = 1
        pending_test_attr = 0
        depth = brace_delta($0)
        next
      }
      pending_test_attr {
        pending_test_attr = 0
      }
      in_test {
        depth += brace_delta($0)
        if (depth <= 0) {
          in_test = 0
          depth = 0
        }
        next
      }
      { print }
    ' "$runtime_path" \
      | rg -n 'RuntimeEffectFact::(CancelAfterBoundary|StopRuntimeExecutor)|MeerkatMachineEffect::RuntimeEffectFact|RuntimeEffectKind::(CancelAfterBoundary|StopRuntimeExecutor)' 2>/dev/null || true)"
    if [[ -n "$found" ]]; then
      fact_literals+="$runtime_path"$'\n'"$found"$'\n'
    fi
  done < <(cd "$root" && rg --files meerkat-runtime/src 2>/dev/null \
    | rg -v '(^|/)generated/|(^|/)effect\.rs$|(^|/)tests/|tests\.rs$' || true)
fi
report_matches "runtime shell files must not construct RuntimeEffectFact literals" "$fact_literals"

if [[ "$failures" -ne 0 ]]; then
  exit 1
fi

echo "effect-authority audit passed"
