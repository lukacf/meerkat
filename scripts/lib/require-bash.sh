# Sourced, not executed. Stops a script on Bash < 4.4 with a clear reason.
#
#   . "<scripts>/lib/require-bash.sh"
#
# Repository scripts use `#!/usr/bin/env bash` and Bash 4.4 behaviour:
# mapfile, BASHPID, associative arrays, and expanding an empty "${array[@]}"
# under `set -u`. macOS ships Bash 3.2 as /bin/bash, where these abort partway
# through ("unbound variable", "command not found"). This file uses only
# Bash 3.2 syntax so the check itself always runs.
if [ "${BASH_VERSINFO[0]:-0}" -lt 4 ] ||
  { [ "${BASH_VERSINFO[0]:-0}" -eq 4 ] && [ "${BASH_VERSINFO[1]:-0}" -lt 4 ]; }; then
  echo "error: $(basename "$0") needs Bash >= 4.4; found ${BASH_VERSION:-unknown} (${BASH:-bash})." \
    "On macOS install a newer bash (brew install bash) and put it ahead of /bin in PATH, so \`env bash\` finds it." >&2
  exit 1
fi
