# 041 - Verify a background job (Python)

One agent submits `expr 17 + 25` with `background: true`. The returned job ID is a submission receipt, not an answer. Python reads the structured receipt for that specific shell call, finds the same ID in the originating session's job list, then waits for its terminal result. It requires `phase` and `result.kind` to both be `succeeded`. Finally, it fetches the referenced result blob, checks the blob ID, decodes the shell output, and requires exit code `0` and stdout `42` followed by a newline. An assistant saying "Submitted" cannot pass these checks.

## Run on Linux

Use Python 3.10+ with matching releases of `meerkat-sdk` and checksum-verified `rkat-rpc`, and `expr` on the system path. Start a trusted **credential-free, loopback** OpenAI-compatible model server with tool calling; no cloud provider key is needed. Copy `config.toml` and set `base_url` and `remote_model` to your server and its loaded model. Leave the local alias `job-demo`, no-auth binding, and single-command shell allowlist intact. The sample port and remote model name are placeholders, not a bundled model.

From this directory, set `PY` to your virtual environment's absolute Python executable, `RPC` to the absolute `rkat-rpc`, and `CONFIG` to the absolute path to your copy of `config.toml`. Run under an unprivileged account in a disposable workspace without credentials. A shell allowlist is not an OS sandbox.

```bash
(
  set -eu
  scratch=$(mktemp -d)
  trap 'rm -rf -- "$scratch"' EXIT
  mkdir "$scratch/home" "$scratch/tmp"
  env -i PATH=/usr/bin:/bin HOME="$scratch/home" TMPDIR="$scratch/tmp" \
    RKAT_RPC="$RPC" "$PY" main.py "$CONFIG"
)
```

The script creates a fresh realm under the temporary directory and closes the RPC client before removing its state. After the initial agent task, Python only reads history, job projections and the output blob. A timeout or mismatch stops the check; there is no cancellation, retry, or resubmission.

**Verification scope:** tested with the published Linux 0.9.16 SDK/runtime pair and a scripted loopback model response. The shell worker, job store, terminal result and output blob were real; selection of the tool by an independently reasoning model was not tested. This example does not test process recovery, delivery subscriptions, or wakeups. A durable job record does not make a lost shell process resumable.
