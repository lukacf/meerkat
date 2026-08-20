#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
helper="${root}/scripts/release-npm-publish-preflight"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-npm-preflight-test.XXXXXX")"
trap 'rm -rf "${test_dir}"' EXIT

fake_bin="${test_dir}/bin"
mkdir -p "${fake_bin}"

cat >"${fake_bin}/npm" <<'FAKE_NPM'
#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${NPM_CONFIG_USERCONFIG:-}" || ! -f "${NPM_CONFIG_USERCONFIG}" ]]; then
  echo "fake npm did not receive a temporary user config" >&2
  exit 90
fi
if ! grep -Fq "${EXPECTED_NODE_AUTH_TOKEN}" "${NPM_CONFIG_USERCONFIG}"; then
  echo "temporary user config does not contain the expected token" >&2
  exit 91
fi

printf '%s\n' "$*" >>"${FAKE_NPM_LOG}"

case "${1:-}" in
  whoami)
    if [[ "${FAKE_NPM_WHOAMI_MODE:-success}" == "failure" ]]; then
      echo "authentication failed" >&2
      exit 2
    fi
    printf '%s\n' "${FAKE_NPM_ACTOR:-release-owner}"
    ;;
  access)
    if [[ "${FAKE_NPM_ACCESS_MODE:-success}" == "failure" ]]; then
      echo "access lookup failed" >&2
      exit 3
    fi
    if [[ -n "${FAKE_NPM_ACCESS_JSON+x}" ]]; then
      printf '%s\n' "${FAKE_NPM_ACCESS_JSON}"
    else
      printf '%s\n' '{"@rkat/sdk":"read-write","@rkat/web":"read-write"}'
    fi
    ;;
  *)
    echo "unexpected fake npm command: $*" >&2
    exit 92
    ;;
esac
FAKE_NPM
chmod +x "${fake_bin}/npm"

secret='npm_test_secret_must_not_leak'
run_output="${test_dir}/output"
fake_log="${test_dir}/npm.log"

assert_secret_absent() {
  if grep -Fq "${secret}" "${run_output}" "${fake_log}" 2>/dev/null; then
    echo "npm token leaked into command output or the npm argument log" >&2
    exit 1
  fi
}

run_case() {
  expected="$1"
  shift
  : >"${run_output}"
  : >"${fake_log}"

  set +e
  env \
    PATH="${fake_bin}:${PATH}" \
    NODE_AUTH_TOKEN="${secret}" \
    EXPECTED_NODE_AUTH_TOKEN="${secret}" \
    FAKE_NPM_LOG="${fake_log}" \
    "$@" \
    "${helper}" >"${run_output}" 2>&1
  status=$?
  set -e

  if [[ "${expected}" == "success" && ${status} -ne 0 ]]; then
    echo "expected npm preflight success, got exit ${status}" >&2
    cat "${run_output}" >&2
    exit 1
  fi
  if [[ "${expected}" == "failure" && ${status} -eq 0 ]]; then
    echo "expected npm preflight failure" >&2
    cat "${run_output}" >&2
    exit 1
  fi
  assert_secret_absent
}

: >"${run_output}"
: >"${fake_log}"
set +e
env -u NODE_AUTH_TOKEN \
  PATH="${fake_bin}:${PATH}" \
  EXPECTED_NODE_AUTH_TOKEN="${secret}" \
  FAKE_NPM_LOG="${fake_log}" \
  "${helper}" >"${run_output}" 2>&1
missing_token_status=$?
set -e
if [[ ${missing_token_status} -eq 0 ]]; then
  echo "missing NODE_AUTH_TOKEN unexpectedly passed" >&2
  exit 1
fi
if [[ -s "${fake_log}" ]]; then
  echo "missing-token case invoked npm before failing" >&2
  exit 1
fi
assert_secret_absent

run_case failure FAKE_NPM_WHOAMI_MODE=failure
run_case failure FAKE_NPM_ACCESS_MODE=failure
run_case failure FAKE_NPM_ACCESS_JSON='{"@rkat/sdk":"read-only","@rkat/web":"read-write"}'
run_case failure FAKE_NPM_ACCESS_JSON='{"@rkat/sdk":"read-write"}'
run_case failure FAKE_NPM_ACCESS_JSON='{not-json'
run_case success FAKE_NPM_ACCESS_JSON='{"@rkat/sdk":"read-write","@rkat/web":"read-write"}'

python3 - "${root}/.github/workflows/release.yml" <<'PY'
import pathlib
import re
import sys

workflow = pathlib.Path(sys.argv[1]).read_text()

def job(name: str) -> str:
    match = re.search(
        rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)",
        workflow,
    )
    if match is None:
        raise SystemExit(f"release workflow is missing job {name}")
    return match.group(0)

preflight = job("registry_credentials_preflight")
registries = job("publish_registries")
web = job("publish_web_sdk")

assert "scripts/release-npm-publish-preflight" in preflight
assert "alpha_crates_only" in preflight
assert re.search(r"(?m)^    needs: \[require_ci_green\]$", preflight)
assert "needs.require_ci_green.result == 'success'" in preflight
for name, body in (("publish_registries", registries), ("publish_web_sdk", web)):
    needs = re.search(r"(?m)^    needs: \[(.*?)\]$", body)
    assert needs is not None, f"{name} does not have an inline needs list"
    dependencies = {dependency.strip() for dependency in needs.group(1).split(",")}
    assert "registry_credentials_preflight" in dependencies, (
        f"{name} does not need the credential preflight"
    )
    assert "needs.registry_credentials_preflight.result == 'success'" in body, (
        f"{name} does not require a successful credential preflight"
    )
PY

printf 'npm publication preflight tests passed\n'
