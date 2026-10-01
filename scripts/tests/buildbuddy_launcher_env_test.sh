#!/usr/bin/env bash
# The BuildBuddy launchers must not leak their environment to the Bazel/bb
# client: Bazel records every client environment variable as
# `--client_env=NAME=value` (and its command line) in the build event stream
# it sends to BuildBuddy. This test needs no BuildBuddy and no network: it
# points the launcher at a fake `bb` that records its own environment, argv
# and the bazelrc files it was given, then checks them.
#
# Every input below is a fake, generated per run, and the launcher runs under
# `env -i` with only these values, so no real secret is ever in play. Values
# are only compared, never printed.
#
# Run: ./scripts/tests/buildbuddy_launcher_env_test.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

pass=0
fail=0
ok()  { printf '  OK:   %s\n' "$*"; pass=$((pass + 1)); }
bad() { printf '  FAIL: %s\n' "$*"; fail=$((fail + 1)); }

work="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-bb-env-test.XXXXXX")"
trap 'rm -rf "${work}"' EXIT
mkdir -p "${work}/home" "${work}/tmp" "${work}/out" "${work}/cache"
: >"${work}/empty-secrets.env"

nonce="$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')"
canary="meerkat-canary-${nonce}"
fake_openai="fake-openai-${nonce}"
fake_bb_key="fake-bb-key-${nonce}"
fake_explicit="fake-explicit-${nonce}"

# The fake client: records env, argv and every --bazelrc file (content and
# mode) under an output directory baked in at creation, because the launcher
# hands the client a sanitized environment. It fails the first main run with
# a corrupt-fetch signature when asked to, so the clean --expunge repair path
# also runs through the launcher's client helper.
shim="${work}/bb"
cat >"${shim}" <<EOF
#!/usr/bin/env bash
out="${work}/out"
n=\$(ls "\${out}" | grep -c '^env\.' || true)
n=\$((n + 1))
env >"\${out}/env.\${n}"
printf '%s\n' "\$@" >"\${out}/argv.\${n}"
for arg in "\$@"; do
  case "\${arg}" in
    --bazelrc=*)
      rc="\${arg#--bazelrc=}"
      if [[ "\${rc}" == "${ROOT}/.bazelrc" ]]; then continue; fi
      cp "\${rc}" "\${out}/rc.\${n}"
      stat -c '%a' "\${rc}" >"\${out}/rcmode.\${n}"
      printf '%s\n' "\${rc}" >"\${out}/rcpath.\${n}"
      ;;
  esac
done
if [[ -f "${work}/fail-once" ]] && ! printf '%s\n' "\$@" | grep -qx clean; then
  rm -f "${work}/fail-once"
  echo "ERROR: No MODULE.bazel file found in /fake/contents" >&2
  exit 1
fi
exit 0
EOF
chmod 700 "${shim}"

# Run a launcher under env -i with only fake inputs plus the given extras.
run_launcher() {
  env -i \
    PATH="/usr/local/bin:/usr/bin:/bin" \
    HOME="${work}/home" \
    TMPDIR="${work}/tmp" \
    XDG_CACHE_HOME="${work}/cache" \
    LANG="C.UTF-8" \
    LC_CTYPE="C.UTF-8" \
    TERM="dumb" \
    TEST="probe-test-name" \
    SMOKE_MODEL="probe-smoke-model" \
    CI="true" \
    GITHUB_SHA="probe-sha" \
    MEERKAT_SECRETS_ENV="${work}/empty-secrets.env" \
    MEERKAT_IMPORT_LOGIN_ZSH_ENV=0 \
    BUILDBUDDY_BB="${shim}" \
    BUILDBUDDY_API_KEY="${fake_bb_key}" \
    MEERKAT_CANARY_SECRET="${canary}" \
    OPENAI_API_KEY="${fake_openai}" \
    GITHUB_TOKEN="${canary}" \
    "$@"
}

contains() { grep -Fq -- "$2" "$1"; }

check_client_env() {
  local label="$1" n="$2"
  local env_file="${work}/out/env.${n}" argv_file="${work}/out/argv.${n}"
  if [[ ! -f "${env_file}" ]]; then
    bad "${label}: the fake client was not invoked"
    return
  fi
  for secret in "${canary}" "${fake_openai}" "${fake_bb_key}" "${fake_explicit}"; do
    if contains "${env_file}" "${secret}" || contains "${argv_file}" "${secret}"; then
      bad "${label}: a secret value reached the client env or argv"
      return
    fi
  done
  ok "${label}: no canary, provider key, API key or explicit override in client env/argv"
  for name in MEERKAT_CANARY_SECRET OPENAI_API_KEY BUILDBUDDY_API_KEY GITHUB_TOKEN MEERKAT_SECRETS_ENV; do
    if grep -q "^${name}=" "${env_file}"; then
      bad "${label}: ${name} is present in the client env"
      return
    fi
  done
  ok "${label}: secret-bearing names absent from the client env"
  local expected
  for expected in "PATH=/usr/local/bin:/usr/bin:/bin" "HOME=${work}/home" "TMPDIR=${work}/tmp" \
    "TEST=probe-test-name" "SMOKE_MODEL=probe-smoke-model" "LC_CTYPE=C.UTF-8" "CI=true" "GITHUB_SHA=probe-sha"; do
    if ! grep -qxF -- "${expected}" "${env_file}"; then
      bad "${label}: allowlisted ${expected%%=*} did not reach the client"
      return
    fi
  done
  ok "${label}: allowlisted PATH, HOME, TMPDIR, TEST, SMOKE_MODEL, LC_*, CI metadata reach the client"
}

check_secret_rc() {
  local label="$1" n="$2" want_provider="$3"
  local rc="${work}/out/rc.${n}"
  if [[ ! -f "${rc}" ]]; then
    bad "${label}: no secret bazelrc handed to the client"
    return
  fi
  if [[ "$(cat "${work}/out/rcmode.${n}")" != "600" ]]; then
    bad "${label}: the secret bazelrc is not mode 0600"
  else
    ok "${label}: the secret bazelrc is mode 0600"
  fi
  if contains "${rc}" "--remote_header=x-buildbuddy-api-key=${fake_bb_key}"; then
    ok "${label}: the API key travels only in the secret bazelrc"
  else
    bad "${label}: the API key header is missing from the secret bazelrc"
  fi
  if [[ "${want_provider}" == "1" ]]; then
    if contains "${rc}" "x-buildbuddy-platform.secret-env-overrides=" \
      && contains "${rc}" "OPENAI_API_KEY=${fake_openai}"; then
      ok "${label}: the provider key travels only as secret-env-overrides in the secret bazelrc"
    else
      bad "${label}: the provider key is not in the secret-env-overrides header"
    fi
  elif contains "${rc}" "${fake_openai}"; then
    bad "${label}: a non-live lane forwarded the provider key"
  else
    ok "${label}: a non-live lane forwards no provider key"
  fi
  if [[ -e "$(cat "${work}/out/rcpath.${n}")" ]]; then
    bad "${label}: the secret bazelrc was not removed after the run"
  else
    ok "${label}: the secret bazelrc is removed after the run"
  fi
}

reset_out() { rm -f "${work}/out/"*; }

echo "buildbuddy-bazel-poc, live lane (e2e-live-rbe):"
reset_out
if run_launcher BUILDBUDDY_BAZEL_COMMAND=e2e-live-rbe BUILDBUDDY_NO_CACHE_REPAIR=1 \
  scripts/buildbuddy-bazel-poc >"${work}/run.log" 2>&1; then
  check_client_env "poc live" 1
  check_secret_rc "poc live" 1 1
else
  bad "poc live: launcher exited non-zero (log withheld: it may echo inputs)"
fi

echo "buildbuddy-bazel-poc, explicit overrides stay out of argv:"
reset_out
if run_launcher BUILDBUDDY_BAZEL_COMMAND=e2e-live-rbe BUILDBUDDY_NO_CACHE_REPAIR=1 \
  MEERKAT_BUILDBUDDY_SECRET_ENV_OVERRIDES="EXTRA_KEY=${fake_explicit}" \
  scripts/buildbuddy-bazel-poc >"${work}/run.log" 2>&1; then
  check_client_env "poc explicit overrides" 1
  if contains "${work}/out/rc.1" "EXTRA_KEY=${fake_explicit}" \
    && contains "${work}/out/rc.1" "OPENAI_API_KEY=${fake_openai}"; then
    ok "poc explicit overrides: explicit and auto entries are both in the secret bazelrc"
  else
    bad "poc explicit overrides: the merged overrides are missing from the secret bazelrc"
  fi
else
  bad "poc explicit overrides: launcher exited non-zero"
fi

echo "buildbuddy-bazel-poc, non-live lane (build):"
reset_out
if run_launcher BUILDBUDDY_BAZEL_COMMAND=build BUILDBUDDY_NO_CACHE_REPAIR=1 \
  scripts/buildbuddy-bazel-poc //crates/meerkat-core:meerkat_core >"${work}/run.log" 2>&1; then
  check_client_env "poc build" 1
  check_secret_rc "poc build" 1 0
else
  bad "poc build: launcher exited non-zero"
fi

echo "buildbuddy-bazel-poc, clean --expunge repair path:"
reset_out
: >"${work}/fail-once"
if run_launcher BUILDBUDDY_BAZEL_COMMAND=e2e-live-rbe \
  scripts/buildbuddy-bazel-poc >"${work}/run.log" 2>&1; then
  if [[ -f "${work}/out/env.3" ]] && grep -qx clean "${work}/out/argv.2"; then
    ok "repair: the launcher ran failed run, clean --expunge, retry"
    check_client_env "repair clean --expunge" 2
    check_client_env "repair retry" 3
  else
    bad "repair: expected three client invocations with clean in the second"
  fi
else
  bad "repair: launcher exited non-zero"
fi
rm -f "${work}/fail-once"

echo "buildbuddy-bazel-poc, gcp-local refuses a live lane with provider keys:"
reset_out
if run_launcher MEERKAT_BAZEL_BACKEND=gcp-local BUILDBUDDY_BAZEL_COMMAND=e2e-live-rbe \
  BUILDBUDDY_NO_CACHE_REPAIR=1 scripts/buildbuddy-bazel-poc >"${work}/run.log" 2>&1; then
  bad "gcp-local: a live lane with provider keys ran"
elif ls "${work}/out" | grep -q '^env\.'; then
  bad "gcp-local: the client was invoked before the refusal"
elif grep -q "refusing to forward them through the Bazel client environment" "${work}/run.log" \
  && ! contains "${work}/run.log" "${fake_openai}"; then
  ok "gcp-local: typed refusal naming the keys, without their values, and no client run"
else
  bad "gcp-local: refused without the expected error"
fi

echo "buildbuddy-bazel-poc, a provider value the header cannot carry is refused:"
reset_out
if run_launcher BUILDBUDDY_BAZEL_COMMAND=e2e-live-rbe BUILDBUDDY_NO_CACHE_REPAIR=1 \
  GOOGLE_APPLICATION_CREDENTIALS_JSON="{\"a\":1,\"b\":2}" \
  scripts/buildbuddy-bazel-poc >"${work}/run.log" 2>&1; then
  bad "unencodable: a value with a comma was forwarded"
elif ls "${work}/out" | grep -q '^env\.'; then
  bad "unencodable: the client was invoked"
elif grep -q "GOOGLE_APPLICATION_CREDENTIALS_JSON contains characters the secret-env-overrides header cannot carry" "${work}/run.log"; then
  ok "unencodable: typed refusal before any client run"
else
  bad "unencodable: refused without the expected error"
fi

echo "buildbuddy-dev, live lane through the developer facade:"
reset_out
if run_launcher scripts/buildbuddy-dev e2e-live >"${work}/run.log" 2>&1; then
  check_client_env "buildbuddy-dev e2e-live" 1
  check_secret_rc "buildbuddy-dev e2e-live" 1 1
else
  bad "buildbuddy-dev e2e-live: launcher exited non-zero"
fi

echo
echo "buildbuddy launcher env: ${pass} passed, ${fail} failed"
[[ "${fail}" -eq 0 ]]
