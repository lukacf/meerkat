#!/usr/bin/env bash
set -euo pipefail

self_script="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/$(basename -- "${BASH_SOURCE[0]}")"

audit_fail() {
  echo "effect-authority audit failed: $*" >&2
  exit 2
}

required_tool() {
  local tool="$1"
  local path
  path="$(type -P "$tool" || true)"
  if [[ -z "$path" ]]; then
    echo "effect-authority audit failed: required command '$tool' not found" >&2
    exit 127
  fi
  printf '%s' "$path"
}

AWK_BIN="$(required_tool awk)"
FIND_BIN="$(required_tool find)"
GREP_BIN="$(required_tool grep)"
RG_BIN="$(type -P rg || true)"

if [[ "${1:-}" == "--self-test" ]]; then
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  reset_fixture() {
    rm -rf "$tmpdir"
    tmpdir="$(mktemp -d)"
  }

  write_fixture() {
    local path="$1"
    local dir="${path%/*}"
    [[ "$dir" != "$path" ]] && mkdir -p "$tmpdir/$dir"
    cat >"$tmpdir/$path"
  }

  expect_audit_failure() {
    local name="$1"
    local path="$2"
    reset_fixture
    write_fixture "$path"
    if "$self_script" "$tmpdir" >/dev/null 2>&1; then
      echo "audit-effect-authority self-test failed: $name fixture passed" >&2
      exit 1
    fi
  }

  expect_audit_failure "peer hard-cancel" "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs" <<'EOF'
fn bad(machine: &Machine) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad");
}
EOF

  expect_audit_failure "legacy interrupt_current_run definition" "meerkat-runtime/src/meerkat_machine/session_management.rs" <<'EOF'
impl Machine {
    pub async fn interrupt_current_run(&self, session_id: &SessionId) {
        let _ = self.hard_cancel_current_run(session_id, "bad").await;
    }
}
EOF

  expect_audit_failure "root user_interrupt module" "meerkat-runtime/src/meerkat_machine/mod.rs" <<'EOF'
#[path = "../user_interrupt.rs"]
pub(crate) mod user_interrupt;
EOF

  expect_audit_failure "split InterruptCurrentRun command" "meerkat-runtime/src/driver/sneaky.rs" <<'EOF'
fn bad(session_id: SessionId, reason: String) {
    let _ = MeerkatMachineCommand::InterruptCurrentRun
    {
        session_id,
        reason,
    };
}
EOF

  expect_audit_failure "InterruptCurrentRun command variant" "meerkat-runtime/src/meerkat_machine_types.rs" <<'EOF'
pub(crate) enum MeerkatMachineCommand {
    RegisterSession {
        session_id: SessionId,
    },
    InterruptCurrentRun {
        session_id: SessionId,
        reason: String,
    },
    CancelAfterBoundary {
        session_id: SessionId,
    },
}
EOF

  expect_audit_failure "direct RuntimeEffect constructor" "meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad() {
    let _ = RuntimeEffect::cancel_after_boundary("bad");
}
EOF

  expect_audit_failure "direct runtime-loop executor stop" "meerkat-runtime/src/runtime_loop.rs" <<'EOF'
async fn bad(executor: &mut dyn CoreExecutor) {
    let _ = executor
        .stop_runtime_executor("bad".to_string())
        .await;
}
EOF

  expect_audit_failure "warn-only cancel-after-boundary effect" "meerkat-runtime/src/control_plane.rs" <<'EOF'
async fn bad(executor: &mut dyn CoreExecutor) -> Result<bool, Error> {
    if let Err(err) = executor.cancel_after_boundary("bad".to_string()).await {
        tracing::warn!(error = %err, "failed to apply runtime executor effect");
    }
    Ok(false)
}
EOF

  expect_audit_failure "dropped interrupt-yielding effect send" "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs" <<'EOF'
fn bad(tx: Sender, projected_effect: ProjectedRuntimeEffect) {
    let _ = tx.try_send(projected_effect.into_effect());
}
EOF

  expect_audit_failure "trace-only boundary cancel failure" "meerkat-runtime/src/meerkat_machine/dispatch_control.rs" <<'EOF'
fn bad() {
    tracing::trace!("out-of-band Ingest boundary cancel was not applied");
}
EOF

  expect_audit_failure "RuntimeEffect::from_fact" "meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad(fact: RuntimeEffectFact) {
    let _ = RuntimeEffect::from_fact(fact);
}
EOF

  expect_audit_failure "visible RuntimeEffectFact/from_fact" "meerkat-runtime/src/effect.rs" <<'EOF'
pub(crate) enum RuntimeEffectFact {
    CancelAfterBoundary { reason: String },
}

impl RuntimeEffect {
    pub(crate) fn from_fact(fact: RuntimeEffectFact) -> Self {
        todo!()
    }
}
EOF

  expect_audit_failure "runtime-shell fact literal" "meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad(reason: String) {
    let _ = RuntimeEffectFact::CancelAfterBoundary { reason };
}
EOF

  expect_audit_failure "generated runtime-effect fact" "meerkat-runtime/src/runtime_loop.rs" <<'EOF'
fn bad(reason: String) {
    let _ = MeerkatMachineEffect::RuntimeEffectFact {
        kind: RuntimeEffectKind::CancelAfterBoundary,
        reason,
    };
}
EOF

  expect_audit_failure "public hard-cancel authority" "meerkat-runtime/src/user_interrupt.rs" <<'EOF'
impl Machine {
    pub async fn hard_cancel_current_run(&self) {
        let authority = UserInterruptAuthority::new();
        self.hard_cancel_current_run_authorized(authority).await;
    }
}
EOF

  expect_audit_failure "visible UserInterruptAuthority constructor" "meerkat-runtime/src/user_interrupt.rs" <<'EOF'
struct UserInterruptAuthority(());

impl UserInterruptAuthority {
    pub(super) fn new() -> Self {
        Self(())
    }
}
EOF

  expect_audit_failure "public hard-cancel live-handle" "meerkat-runtime/src/user_interrupt.rs" <<'EOF'
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

  expect_audit_failure "recursive RPC interrupt-handle" "meerkat-rpc/src/session_executor.rs" <<'EOF'
impl CoreExecutorInterruptHandle for SessionRuntimeInterruptHandle {
    async fn hard_cancel_current_run(&self) {
        let _ = self.runtime.interrupt(&self.session_id).await;
    }
}
EOF

  expect_audit_failure "bridge hard-cancel handler" "meerkat-runtime/src/comms_drain.rs" <<'EOF'
async fn bad(adapter: Adapter, session_id: SessionId, command: BridgeCommand) {
    match command {
        BridgeCommand::HardCancelMember(payload) => {
            let _ = adapter
                .hard_cancel_current_run(session_id, payload.reason)
                .await;
        }
        _ => {}
    }
}
EOF

  expect_audit_failure "comms-drain hard-cancel" "meerkat-runtime/src/comms_drain.rs" <<'EOF'
async fn bad(machine: Machine, session_id: SessionId) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad").await;
}
EOF

  expect_audit_failure "local bridge hard-cancel" "meerkat-mob/src/runtime/local_bridge.rs" <<'EOF'
async fn bad(machine: Machine, session_id: SessionId) {
    let _ = machine.hard_cancel_current_run(&session_id, "bad").await;
}
EOF

  expect_audit_failure "public surface interrupt" "meerkat-rest/src/lib.rs" <<'EOF'
async fn public_interrupt(service: Service, session_id: SessionId) {
    let _ = service.interrupt(&session_id).await;
}
EOF

  expect_audit_failure "multiline public surface interrupt" "meerkat-rest/src/lib.rs" <<'EOF'
async fn public_interrupt(service: Service, session_id: SessionId) {
    let _ = service
        .interrupt(&session_id)
        .await;
}
EOF

  expect_audit_failure "public interrupt_current_run" "meerkat-rpc/src/realtime_ws.rs" <<'EOF'
async fn public_interrupt(adapter: Adapter, session_id: SessionId) {
    let _ = adapter.interrupt_current_run(&session_id).await;
}
EOF

  expect_audit_failure "example interrupt bypass" "examples/999-runtime-backed/src/main.rs" <<'EOF'
async fn bad(service: Service, session_id: SessionId) {
    let _ = service.interrupt(&session_id).await;
}
EOF

  (cd /tmp && "$self_script" >/dev/null 2>&1) || {
    echo "audit-effect-authority self-test failed: absolute script invocation outside repo failed" >&2
    exit 1
  }

  if "$self_script" "$tmpdir/missing-root" >/dev/null 2>&1; then
    echo "audit-effect-authority self-test failed: missing scan root passed" >&2
    exit 1
  fi

  echo "audit-effect-authority self-test passed"
  exit 0
fi

root="${1:-}"
if [[ -z "$root" ]]; then
  script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
  if [[ "$(basename -- "$self_script")" == "audit_effect_authority_test" && -d "$script_dir/meerkat-runtime" ]]; then
    root="$script_dir"
  else
    root="$(cd "$script_dir/.." && pwd)"
  fi
fi
if [[ ! -d "$root" ]]; then
  audit_fail "scan root does not exist or is not a directory: $root"
fi
root="$(cd "$root" && pwd)"

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

fallback_rg_excluded() {
  local path="${1#./}"
  local glob
  case "$path" in
    .* | */.* | target/* | */target/*)
      return 0
      ;;
  esac
  for glob in "${fallback_rg_exclude_globs[@]:-}"; do
    [[ "$path" == $glob ]] && return 0
    if [[ "$glob" == **/* ]]; then
      [[ "$path" == "${glob#**/}" ]] && return 0
    fi
  done
  return 1
}

fallback_rg_list_files() {
  local paths=("$@")
  local path
  local file
  [[ "${#paths[@]}" -eq 0 ]] && paths=(".")
  for path in "${paths[@]}"; do
    if [[ -f "$path" ]]; then
      file="${path#./}"
      fallback_rg_excluded "$file" || printf '%s\n' "$file"
    elif [[ -d "$path" ]]; then
      while IFS= read -r file; do
        file="${file#./}"
        fallback_rg_excluded "$file" || printf '%s\n' "$file"
      done < <("$FIND_BIN" "$path" -type f -print)
    else
      echo "fallback rg: path not found: $path" >&2
      return 2
    fi
  done
}

fallback_rg_search() {
  local pattern="$1"
  local line_numbers="$2"
  local invert="$3"
  shift 3
  local paths=("$@")
  local grep_args=(-E -I)
  local file
  local matched=1
  local status
  [[ "$line_numbers" == true ]] && grep_args+=(-n)
  [[ "$invert" == true ]] && grep_args+=(-v)

  if [[ "${#paths[@]}" -eq 0 ]]; then
    "$GREP_BIN" "${grep_args[@]}" -- "$pattern"
    return $?
  fi

  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    set +e
    "$GREP_BIN" "${grep_args[@]}" -H -- "$pattern" "$file"
    status=$?
    set -e
    case "$status" in
      0) matched=0 ;;
      1) ;;
      *) return "$status" ;;
    esac
  done < <(fallback_rg_list_files "${paths[@]}")
  return "$matched"
}

rg_cmd() {
  if [[ -n "$RG_BIN" ]]; then
    "$RG_BIN" "$@"
    return $?
  fi

  local files_mode=false
  local line_numbers=false
  local invert=false
  local pattern=""
  local have_pattern=false
  local paths=()
  local glob
  fallback_rg_exclude_globs=()

  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      --files)
        files_mode=true
        shift
        ;;
      -n)
        line_numbers=true
        shift
        ;;
      -v)
        invert=true
        shift
        ;;
      --glob)
        shift
        [[ "$#" -gt 0 ]] || {
          echo "fallback rg: --glob requires a value" >&2
          return 2
        }
        glob="$1"
        [[ "$glob" == '!'* ]] || {
          echo "fallback rg: only exclusion --glob values are supported: $glob" >&2
          return 2
        }
        fallback_rg_exclude_globs+=("${glob#!}")
        shift
        ;;
      --glob=*)
        glob="${1#--glob=}"
        [[ "$glob" == '!'* ]] || {
          echo "fallback rg: only exclusion --glob values are supported: $glob" >&2
          return 2
        }
        fallback_rg_exclude_globs+=("${glob#!}")
        shift
        ;;
      -*)
        echo "fallback rg: unsupported option: $1" >&2
        return 2
        ;;
      *)
        if [[ "$files_mode" == true ]]; then
          paths+=("$1")
        elif [[ "$have_pattern" == false ]]; then
          pattern="$1"
          have_pattern=true
        else
          paths+=("$1")
        fi
        shift
        ;;
    esac
  done

  if [[ "$files_mode" == true ]]; then
    fallback_rg_list_files "${paths[@]}"
    return $?
  fi

  [[ "$have_pattern" == true ]] || {
    echo "fallback rg: missing pattern" >&2
    return 2
  }
  fallback_rg_search "$pattern" "$line_numbers" "$invert" "${paths[@]}"
}

capture_rg() {
  local output
  local status
  set +e
  output="$(rg_cmd "$@" 2>&1)"
  status=$?
  set -e
  case "$status" in
    0) printf '%s\n' "$output" ;;
    1) ;;
    *) audit_fail "search command failed: rg $*"$'\n'"$output" ;;
  esac
}

filter_rg() {
  local input="$1"
  shift
  [[ -z "$input" ]] && return 0
  printf '%s\n' "$(printf '%s\n' "$input" | capture_rg "$@")"
}

run_rg() {
  local pattern="$1"
  shift
  (cd "$root" && capture_rg -n "$pattern" . --glob '!target/**' --glob '!scripts/audit-effect-authority.sh' --glob '!audit_effect_authority_test' "$@")
}

scan_direct_interrupt_command_construction() {
  local files
  local file
  local found
  local matches=""

  files="$(cd "$root" && capture_rg --files . \
    --glob '!target/**' \
    --glob '!scripts/audit-effect-authority.sh' \
    --glob '!audit_effect_authority_test' \
    --glob '!meerkat-runtime/src/meerkat_machine/session_management.rs' \
    --glob '!meerkat-runtime/src/meerkat_machine/dispatch_session.rs' \
    --glob '!meerkat-runtime/src/meerkat_machine_tests.rs' \
    --glob '!meerkat-runtime/tests/**' \
    --glob '!**/tests/**' \
    --glob '!**/*tests.rs')"

  while IFS= read -r file; do
    [[ -z "$file" ]] && continue
    [[ "$file" == *.rs ]] || continue
    found="$("$AWK_BIN" -v file="$file" '
      function reset_pending() {
        pending = 0
        pending_line = 0
        pending_text = ""
      }
      function report(line_no, text) {
        print file ":" line_no ":" text
      }
      {
        line = $0
        if (pending) {
          if (line ~ /^[[:space:]]*$/ || line ~ /^[[:space:]]*\/\//) {
            next
          }
          if (line ~ /^[[:space:]]*\{/) {
            if (line !~ /\{[[:space:]]*\.\.[[:space:]]*\}/) {
              report(pending_line, pending_text)
            }
            reset_pending()
            next
          }
          reset_pending()
        }
        if (line ~ /^[[:space:]]*\/\//) {
          next
        }
        if (line ~ /MeerkatMachineCommand::InterruptCurrentRun/) {
          if (line ~ /MeerkatMachineCommand::InterruptCurrentRun[[:space:]]*\{/) {
            if (line !~ /\{[[:space:]]*\.\.[[:space:]]*\}/) {
              report(NR, line)
            }
          } else {
            pending = 1
            pending_line = NR
            pending_text = line
          }
        }
      }
    ' "$root/$file")"
    if [[ -n "$found" ]]; then
      matches+="$found"$'\n'
    fi
  done <<<"$files"

  printf '%s' "$matches"
}

scan_interrupt_command_variant_definition() {
  local file="$root/meerkat-runtime/src/meerkat_machine_types.rs"
  [[ -f "$file" ]] || return 0

  "$AWK_BIN" -v file="meerkat-runtime/src/meerkat_machine_types.rs" '
    function brace_delta(line, opened, closed, copy) {
      copy = line
      opened = gsub(/\{/, "{", copy)
      copy = line
      closed = gsub(/\}/, "}", copy)
      return opened - closed
    }
    /enum[[:space:]]+MeerkatMachineCommand[[:space:]]*\{/ {
      in_command_enum = 1
      depth = brace_delta($0)
      next
    }
    in_command_enum {
      if ($0 ~ /^[[:space:]]*InterruptCurrentRun([[:space:]]*[\{,]|$)/) {
        print file ":" NR ":" $0
      }
      depth += brace_delta($0)
      if (depth <= 0) {
        in_command_enum = 0
      }
    }
  ' "$file"
}

strip_core_executor_interrupt_impls() {
  "$AWK_BIN" '
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
  "$AWK_BIN" '
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
report_matches "legacy interrupt_current_run alias definitions are forbidden" \
  "$(run_rg 'fn[[:space:]]+interrupt_current_run(_with_reason)?[[:space:]]*\(' \
    --glob '!meerkat-runtime/src/meerkat_machine_tests.rs' \
    --glob '!meerkat-runtime/tests/**' \
    --glob '!**/tests/**' \
    --glob '!**/*tests.rs')"
report_matches "MeerkatMachineCommand must not expose an InterruptCurrentRun variant" \
  "$(scan_interrupt_command_variant_definition)"

if [[ -f "$root/meerkat-runtime/src/meerkat_machine/mod.rs" ]]; then
  root_user_interrupt_module="$(capture_rg -n 'mod[[:space:]]+user_interrupt|path[[:space:]]*=[[:space:]]*"\\.\\./user_interrupt\\.rs"' "$root/meerkat-runtime/src/meerkat_machine/mod.rs")"
  report_matches "user_interrupt authority module must not be mounted as a meerkat_machine sibling" "$root_user_interrupt_module"
fi

peer_matches=""
hard_interrupt_authority_pattern='\b(hard_cancel_current_run|interrupt_handle|interrupt_handle_for|interrupt_current_run_with_reason|interrupt_current_run_inner|dispatch_user_interrupt|apply_user_interrupt_live_cancel)\b|MeerkatMachineCommand::InterruptCurrentRun|runtime\.interrupt\(|session_service\.interrupt\('
for peer_file in \
  "$root/meerkat-runtime/src/meerkat_machine/dispatch_control.rs" \
  "$root/meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs"
do
  if [[ -f "$peer_file" ]]; then
    found="$(capture_rg -n "$hard_interrupt_authority_pattern" "$peer_file")"
    if [[ -n "$found" ]]; then
      peer_matches+="$found"$'\n'
    fi
  fi
done
peer_admission_files="$(cd "$root" && capture_rg --files)"
peer_admission_files="$(filter_rg "$peer_admission_files" '(^|/)peer_admission[^/]*\.rs$|(^|/)peer_admission/')"
while IFS= read -r peer_file; do
  [[ -z "$peer_file" ]] && continue
  found="$(capture_rg -n "$hard_interrupt_authority_pattern" "$root/$peer_file")"
  if [[ -n "$found" ]]; then
    peer_matches+="$found"$'\n'
  fi
done <<<"$peer_admission_files"
report_matches "peer-admission code can reach hard interrupt authority" "$peer_matches"

if [[ -f "$root/meerkat-runtime/src/comms_drain.rs" ]]; then
  comms_drain_body="$(cat "$root/meerkat-runtime/src/comms_drain.rs")"
  comms_drain_matches="$(filter_rg "$comms_drain_body" -n '\b(hard_cancel_current_run|interrupt_handle|interrupt_handle_for|interrupt_current_run_with_reason|interrupt_current_run_inner|dispatch_user_interrupt|apply_user_interrupt_live_cancel)\b|MeerkatMachineCommand::InterruptCurrentRun|\.interrupt_current_run(_with_reason)?\(|\b(runtime|adapter|session_service)\.interrupt\(')"
  report_matches "comms-drain code can reach hard interrupt authority" "$comms_drain_matches"
fi

bridge_hard_cancel_exposure=""
for bridge_hard_cancel_file in \
  "$root/meerkat-runtime/src/comms_drain.rs" \
  "$root/meerkat-mob/src/runtime/actor.rs" \
  "$root/meerkat-mob/src/runtime/provisioner.rs"
do
  if [[ -f "$bridge_hard_cancel_file" ]]; then
    stripped_bridge_file="$(strip_cfg_test_modules <"$bridge_hard_cancel_file")"
    found="$(filter_rg "$stripped_bridge_file" -n 'BridgeCommand::HardCancelMember|BridgeHardCancelPayload|hard_cancel_member:[[:space:]]*true')"
    if [[ -n "$found" ]]; then
      bridge_hard_cancel_exposure+="$bridge_hard_cancel_file"$'\n'"$found"$'\n'
    fi
  fi
done
report_matches "supervisor bridge must not expose hard-cancel member authority" "$bridge_hard_cancel_exposure"

if [[ -f "$root/meerkat-rpc/src/session_executor.rs" ]]; then
  rpc_executor_recursion="$(capture_rg -n '\.runtime\.interrupt\(' "$root/meerkat-rpc/src/session_executor.rs")"
  report_matches "RPC executor interrupt handle must not re-enter public SessionRuntime::interrupt" "$rpc_executor_recursion"
fi

if [[ -f "$root/meerkat-runtime/src/user_interrupt.rs" ]]; then
  strip_user_interrupt_internal_bodies() {
    "$AWK_BIN" '
      function brace_delta(line, opened, closed, copy) {
        copy = line
        opened = gsub(/\{/, "{", copy)
        copy = line
        closed = gsub(/\}/, "}", copy)
        return opened - closed
      }
      /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?async[[:space:]]+fn[[:space:]]+(hard_cancel_current_run_authorized|interrupt_current_run_inner|apply_user_interrupt_live_cancel)[[:space:]]*\(/ {
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
  public_interrupt_bypass="$(strip_user_interrupt_internal_bodies "$root/meerkat-runtime/src/user_interrupt.rs")"
  public_interrupt_bypass="$(filter_rg "$public_interrupt_bypass" -n 'self\.hard_cancel_current_run_authorized\(|UserInterruptAuthority::new\(\)|\.hard_cancel_current_run\(|\binterrupt_handle_for\(')"
  public_interrupt_bypass="$(filter_rg "$public_interrupt_bypass" -v 'fn[[:space:]]+interrupt_handle_for[[:space:]]*\(')"
  report_matches "public user-interrupt API must route through the command/DSL path" "$public_interrupt_bypass"

  authority_visibility="$(capture_rg -n 'pub(\([^)]*\))?[[:space:]]+struct[[:space:]]+UserInterruptAuthority|pub(\([^)]*\))?[[:space:]]+fn[[:space:]]+new[[:space:]]*\(' "$root/meerkat-runtime/src/user_interrupt.rs")"
  report_matches "UserInterruptAuthority type and constructor must remain private" "$authority_visibility"
fi

authority_mints="$(run_rg 'UserInterruptAuthority::new\(\)' --glob '!meerkat-runtime/src/user_interrupt.rs')"
report_matches "UserInterruptAuthority may only be minted by the command-owned interrupt path" "$authority_mints"

public_interrupt_bypass_pattern='\b(service|svc|session_service|cancel_svc|self\.service|self\.session_service)\.interrupt\(|^[[:space:]]*\.interrupt\(|session_service\(\)\.interrupt\(|\.interrupt_current_run(_with_reason)?\('
public_interrupt_bypasses=""
for surface_file in \
  "$root/meerkat-rest/src/lib.rs" \
  "$root/meerkat-mcp-server/src/lib.rs" \
  "$root/meerkat-rpc/src/handlers/session.rs" \
  "$root/meerkat-rpc/src/handlers/turn.rs" \
  "$root/meerkat-rpc/src/realtime_ws.rs" \
  "$root/meerkat-cli/src/main.rs" \
  "$root/meerkat-openai/src/realtime_attachment.rs" \
  "$root/meerkat-mob/src/runtime/local_bridge.rs" \
  "$root/meerkat-mob/src/runtime/provisioner.rs"
do
  if [[ -f "$surface_file" ]]; then
    stripped_surface="$(strip_core_executor_interrupt_impls "$surface_file")"
    stripped_surface="$(printf '%s\n' "$stripped_surface" | strip_cfg_test_modules)"
    found="$(filter_rg "$stripped_surface" -n "$public_interrupt_bypass_pattern")"
    if [[ -n "$found" ]]; then
      public_interrupt_bypasses+="$surface_file"$'\n'"$found"$'\n'
    fi
  fi
done
report_matches "public surface interrupt paths must route through MeerkatMachine::hard_cancel_current_run" "$public_interrupt_bypasses"

example_interrupt_bypasses=""
if [[ -d "$root/examples" ]]; then
  example_files="$(cd "$root" && capture_rg --files examples --glob '!target/**')"
  while IFS= read -r example_file; do
    [[ -z "$example_file" ]] && continue
    [[ "$example_file" == *.rs ]] || continue
    case "$example_file" in
      # Standalone demo server: ForceState intentionally uses EphemeralSessionService
      # and has no runtime-owned MeerkatMachine authority to route through.
      examples/034-codemob-mcp/src/tools/consult.rs)
        continue
        ;;
    esac
    stripped_example="$(strip_core_executor_interrupt_impls "$root/$example_file")"
    stripped_example="$(printf '%s\n' "$stripped_example" | strip_cfg_test_modules)"
    found="$(filter_rg "$stripped_example" -n "$public_interrupt_bypass_pattern")"
    if [[ -n "$found" ]]; then
      example_interrupt_bypasses+="$root/$example_file"$'\n'"$found"$'\n'
    fi
  done <<<"$example_files"
fi
report_matches "examples must not bypass runtime-owned interrupt authority unless explicitly classified as standalone" "$example_interrupt_bypasses"

direct_interrupt_current_run="$(run_rg '\.interrupt_current_run(_with_reason)?\(' \
  --glob '!meerkat-runtime/src/meerkat_machine/session_management.rs' \
  --glob '!meerkat-runtime/src/user_interrupt.rs' \
  --glob '!meerkat-runtime/src/meerkat_machine_tests.rs' \
  --glob '!meerkat-runtime/tests/**' \
  --glob '!**/tests/**' \
  --glob '!**/*tests.rs')"
report_matches "direct interrupt_current_run callsites are forbidden outside runtime tests" "$direct_interrupt_current_run"

direct_interrupt_command="$(scan_direct_interrupt_command_construction)"
report_matches "direct InterruptCurrentRun command construction is forbidden outside the session command API" "$direct_interrupt_command"

report_matches "direct RuntimeEffect constructor helpers are forbidden" \
  "$(run_rg 'RuntimeEffect::(cancel_after_boundary|stop_runtime_executor)\b')"

runtime_effect_assoc="$(run_rg 'RuntimeEffect::[A-Za-z_][A-Za-z0-9_]*' --glob '!**/effect.rs')"
report_matches "RuntimeEffect associated constructors must stay inside the sealed effect module" "$runtime_effect_assoc"

if [[ -f "$root/meerkat-runtime/src/runtime_loop.rs" ]]; then
  runtime_loop_direct_stop="$(capture_rg -n 'stop_runtime_executor[[:space:]]*\(' "$root/meerkat-runtime/src/runtime_loop.rs")"
  report_matches "runtime_loop must not call executor stop_runtime_executor directly" "$runtime_loop_direct_stop"
fi

if [[ -f "$root/meerkat-runtime/src/control_plane.rs" ]]; then
  warn_only_boundary_cancel="$(capture_rg -n 'failed to apply runtime executor effect|if[[:space:]]+let[[:space:]]+Err\(err\)[[:space:]]*=[[:space:]]*executor\.cancel_after_boundary' "$root/meerkat-runtime/src/control_plane.rs")"
  report_matches "cancel-after-boundary executor effects must fail closed, not warn-only" "$warn_only_boundary_cancel"
fi

interrupt_effect_drop_matches=""
for interrupt_effect_file in \
  "$root/meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs" \
  "$root/meerkat-runtime/src/meerkat_machine/dispatch_control.rs" \
  "$root/meerkat-runtime/src/meerkat_machine/runtime_control.rs"
do
  if [[ -f "$interrupt_effect_file" ]]; then
    found="$(capture_rg -n 'let[[:space:]]+_[[:space:]]*=[[:space:]]*tx\.try_send\(projected_effect\.into_effect\(\)\)|boundary cancel was not applied|runtime effect channel full after live boundary cancel|runtime effect channel closed after live boundary cancel' "$interrupt_effect_file")"
    if [[ -n "$found" ]]; then
      interrupt_effect_drop_matches+="$interrupt_effect_file"$'\n'"$found"$'\n'
    fi
  fi
done
report_matches "interrupt-yielding runtime effects must not be dropped or trace-only" "$interrupt_effect_drop_matches"

if [[ -f "$root/meerkat-runtime/src/effect.rs" ]]; then
  visible_runtime_effect_fact="$(capture_rg -n 'pub(\([^)]*\))?[[:space:]]+enum[[:space:]]+RuntimeEffectFact|pub(\([^)]*\))?[[:space:]]+fn[[:space:]]+from_fact[[:space:]]*\(' "$root/meerkat-runtime/src/effect.rs")"
  report_matches "RuntimeEffectFact and RuntimeEffect::from_fact must not be crate-visible" "$visible_runtime_effect_fact"
fi

if [[ -f "$root/meerkat-mob/src/runtime/local_bridge.rs" ]]; then
  local_bridge_hard_cancel="$(strip_cfg_test_modules <"$root/meerkat-mob/src/runtime/local_bridge.rs")"
  local_bridge_hard_cancel="$(filter_rg "$local_bridge_hard_cancel" -n '\bhard_cancel_current_run[[:space:]]*\(')"
  report_matches "local mob bridge interrupt must not hard-cancel member sessions" "$local_bridge_hard_cancel"
fi

fact_literals=""
if [[ -d "$root/meerkat-runtime/src" ]]; then
  runtime_files="$(cd "$root" && capture_rg --files meerkat-runtime/src)"
  runtime_files="$(filter_rg "$runtime_files" -v '(^|/)generated/|(^|/)effect\.rs$|(^|/)tests/|tests\.rs$')"
  while IFS= read -r runtime_file; do
    [[ -z "$runtime_file" ]] && continue
    runtime_path="$root/$runtime_file"
    stripped_runtime="$(strip_cfg_test_modules <"$runtime_path")"
    found="$(filter_rg "$stripped_runtime" -n 'RuntimeEffectFact::(CancelAfterBoundary|StopRuntimeExecutor)|MeerkatMachineEffect::RuntimeEffectFact|RuntimeEffectKind::(CancelAfterBoundary|StopRuntimeExecutor)')"
    if [[ -n "$found" ]]; then
      fact_literals+="$runtime_path"$'\n'"$found"$'\n'
    fi
  done <<<"$runtime_files"
fi
report_matches "runtime shell files must not construct RuntimeEffectFact literals" "$fact_literals"

if [[ "$failures" -ne 0 ]]; then
  exit 1
fi

echo "effect-authority audit passed"
