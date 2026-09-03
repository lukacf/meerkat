#!/usr/bin/env bash
# shellcheck disable=SC2030,SC2031
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-build-backend-env.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

FAKE_HOME="${TEST_ROOT}/home"
FAKE_BIN="${TEST_ROOT}/bin"
mkdir -p "${FAKE_HOME}" "${FAKE_BIN}"

cat > "${FAKE_HOME}/.zshrc" <<'EOF'
export OPENAI_API_KEY=login-openai
export ANTHROPIC_API_KEY=login-anthropic
export GEMINI_API_KEY=login-gemini
export GOOGLE_API_KEY=login-google
export BUILDBUDDY_API_KEY=login-buildbuddy
export NOT_A_PROVIDER_SECRET=$'ignored\nRKAT_INJECTED=injected'
EOF

cat > "${FAKE_BIN}/zsh" <<'EOF'
#!/bin/bash
set -euo pipefail
[[ "$1" == "-lic" && "$2" == "/usr/bin/env -0 >&3" ]]
if [[ "${FAKE_ZSH_FAIL:-0}" == "1" ]]; then
  set -a
  source "${HOME}/.zshrc"
  /usr/bin/env -0 >&3
  exit 42
fi
set -a
source "${HOME}/.zshrc"
printf 'startup output containing login-openai\n'
printf 'OPENAI_BAD-NAME=invalid\0' >&3
/usr/bin/env -0 >&3
EOF
chmod +x "${FAKE_BIN}/zsh"

cat > "${TEST_ROOT}/secrets.env" <<'EOF'
OPENAI_API_KEY=secrets-openai
ANTHROPIC_API_KEY=secrets-anthropic
printf 'secret source output containing secrets-anthropic\n' >&2
EOF

(
  source "${REPO_ROOT}/scripts/build-backend-env"
  unset OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY GOOGLE_API_KEY BUILDBUDDY_API_KEY
  unset RKAT_INJECTED
  unset NOT_A_PROVIDER_SECRET MEERKAT_SECRETS_ENV MEERKAT_IMPORT_LOGIN_ZSH_ENV

  output="${TEST_ROOT}/import.out"
  HOME="${FAKE_HOME}" \
    PATH="${FAKE_BIN}:${PATH}" \
    meerkat_load_local_secrets_env "/" >"${output}" 2>&1

  [[ ! -s "${output}" ]]
  [[ "${OPENAI_API_KEY}" == "login-openai" ]]
  [[ "${ANTHROPIC_API_KEY}" == "login-anthropic" ]]
  [[ "${GEMINI_API_KEY}" == "login-gemini" ]]
  [[ "${BUILDBUDDY_API_KEY}" == "login-buildbuddy" ]]
  [[ -z "${NOT_A_PROVIDER_SECRET+x}" ]]
  [[ -z "${RKAT_INJECTED+x}" ]]
  ! printenv 'OPENAI_BAD-NAME' >/dev/null 2>&1
)

(
  source "${REPO_ROOT}/scripts/build-backend-env"
  unset OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY GOOGLE_API_KEY
  unset NOT_A_PROVIDER_SECRET MEERKAT_SECRETS_ENV MEERKAT_IMPORT_LOGIN_ZSH_ENV
  export OPENAI_API_KEY=caller-openai
  export GOOGLE_API_KEY=

  output="${TEST_ROOT}/precedence.out"
  HOME="${FAKE_HOME}" \
    PATH="${FAKE_BIN}:${PATH}" \
    MEERKAT_SECRETS_ENV="${TEST_ROOT}/secrets.env" \
    meerkat_load_local_secrets_env "${TEST_ROOT}" >"${output}" 2>&1

  [[ ! -s "${output}" ]]
  [[ "${OPENAI_API_KEY}" == "caller-openai" ]]
  [[ "${ANTHROPIC_API_KEY}" == "secrets-anthropic" ]]
  [[ "${GEMINI_API_KEY}" == "login-gemini" ]]
  [[ -z "${GOOGLE_API_KEY}" ]]
  [[ -z "${NOT_A_PROVIDER_SECRET+x}" ]]
)

(
  source "${REPO_ROOT}/scripts/build-backend-env"
  unset OPENAI_API_KEY MEERKAT_SECRETS_ENV

  output="${TEST_ROOT}/opt-out.out"
  HOME="${FAKE_HOME}" \
    PATH="${FAKE_BIN}:${PATH}" \
    MEERKAT_IMPORT_LOGIN_ZSH_ENV=0 \
    meerkat_load_local_secrets_env "/" >"${output}" 2>&1

  [[ ! -s "${output}" ]]
  [[ -z "${OPENAI_API_KEY+x}" ]]
)

(
  source "${REPO_ROOT}/scripts/build-backend-env"
  unset OPENAI_API_KEY MEERKAT_SECRETS_ENV MEERKAT_IMPORT_LOGIN_ZSH_ENV

  output="${TEST_ROOT}/failed-zsh.out"
  HOME="${FAKE_HOME}" \
    PATH="${FAKE_BIN}:${PATH}" \
    FAKE_ZSH_FAIL=1 \
    meerkat_load_local_secrets_env "/" >"${output}" 2>&1

  [[ ! -s "${output}" ]]
  [[ -z "${OPENAI_API_KEY+x}" ]]
)

(
  source "${REPO_ROOT}/scripts/build-backend-env"
  unset OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY
  unset MEERKAT_IMPORT_LOGIN_ZSH_ENV
  export OPENAI_API_KEY=caller-openai

  output="${TEST_ROOT}/xtrace.out"
  {
    set -x
    HOME="${FAKE_HOME}" \
      PATH="${FAKE_BIN}:${PATH}" \
      MEERKAT_SECRETS_ENV="${TEST_ROOT}/secrets.env" \
      meerkat_load_local_secrets_env "${TEST_ROOT}"
    [[ "$-" == *x* ]]
    set +x
  } >"${output}" 2>&1

  if grep -Fq 'caller-openai' "${output}" ||
    grep -Fq 'secrets-anthropic' "${output}" ||
    grep -Fq 'login-gemini' "${output}"; then
    echo "xtrace exposed a credential value" >&2
    exit 1
  fi
)

(
  source "${REPO_ROOT}/scripts/build-backend-env"
  unset OPENAI_API_KEY ANTHROPIC_API_KEY GEMINI_API_KEY
  unset MEERKAT_SECRETS_ENV MEERKAT_IMPORT_LOGIN_ZSH_ENV MEERKAT_BUILDBUDDY

  output="${TEST_ROOT}/missing-zsh.out"
  HOME="${FAKE_HOME}" \
    PATH="${TEST_ROOT}/missing-bin" \
    meerkat_load_local_secrets_env "/" >"${output}" 2>&1

  [[ ! -s "${output}" ]]
  [[ "$(meerkat_selected_build_backend)" == "cargo" ]]
  [[ -z "${OPENAI_API_KEY+x}" ]]
  [[ -z "${ANTHROPIC_API_KEY+x}" ]]
  [[ -z "${GEMINI_API_KEY+x}" ]]
)

echo "build backend secret environment contract holds"
