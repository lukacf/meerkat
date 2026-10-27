# Retry an unsuccessful preparation

If a library preparation fails or is explicitly stopped, the same live setup
session can make a new attempt. The browser's **Try preparation again** action and
the `setup_retry_prepare` MCP tool use the existing Controller and Runner.
Retry is explicit. Polling, reopening a job, losing an MCP reply and cancelling
a request do not request another attempt.

This is a source capability. Previously frozen standalone kits keep their
original tools until a later edition includes this implementation.

## Choose the exact attempt

Read `setup_state` and the intended job through `setup_job`. Retry is available
when `preparation_retry.available` is true, the job has `kind: "prepare"` and
`status: "unconfirmed"`, and its ID equals that family's `preparation_job_id`.
Wait until `operation_in_progress` is false before requesting a new attempt.
An unconfirmed job preserves uncertainty about existing files; it does not mean
the previous attempt had no effects.

Call the MCP tool with the exact job ID:

```json
{"job_id":"0123456789abcdef0123456789abcdef"}
```

The authenticated local HTTP equivalent is `POST /api/retry_prepare` with the
same JSON body and the existing browser session protection. The caller cannot
supply a family, destination, command, runtime, bundle or replacement policy.
The host's original family allowlist and pinned inputs continue to apply.

The result has the existing `{"job": {...}}` shape. Its new job has
`kind: "prepare"`, the same `family`, a new `id`, and `retry_of` equal to the
requested predecessor ID. The family's `preparation_job_id` now identifies this
new attempt. Observe it using `setup_job`; a successful attempt publishes the
prepared profiles for ordinary `setup_help`, `setup_check` and `setup_install`.

Repeating the exact accepted retry request returns the same new job, including
after a lost reply, failed retry, success or a later attempt. It never starts
another worker. To try again after the retry itself fails or stops, explicitly
request retry using that retry job's own ID. These identities and mappings are
process-local; restarting the host is not preparation recovery.

## Preserve earlier work and uncertainty

For `agno`, `services` and `activepieces`, the first preparation keeps its
existing `inspection/<family>` destination. Each retry uses a separate
`inspection/<family>-<new-job-id>` directory. Earlier job records and inspection
files remain unchanged. The installer, receipt validation and profile reader
run again against the new destination. No automatic cleanup, deletion or
rollback is performed.

The candidate inspection command remains private to the attempt while its
receipt and profiles are checked. The Controller publishes that command and
the profiles together under the same state lock as successful completion.
Failure or an accepted stop leaves both unpublished. Subsequent help and
configuration checks use the successful attempt's command.

For `official-github` and `official-playwright`, preparation and retry only
recheck the pinned metadata. They do not create an inspection installation,
start the provider server, invoke Node or launch a browser.

The operation lock prevents overlapping setup workers. The new attempt and
predecessor-to-retry mapping are reserved before dispatch. If the worker cannot
start, the reserved retry remains observable as `unconfirmed`; replay still
returns it. An already accepted request can be read back while another
operation is running, without changing that operation or the latest job.

## Refusals and scope

Unknown or foreign IDs return `unknown_job`. Running or successful preparations
and installation jobs return `preparation_retry_unavailable`. A previously
unaccepted request for a superseded preparation returns
`preparation_superseded`. A busy operation returns `operation_in_progress`
before reserving a new retry. Recovery sessions and closed admission retain
their existing refusals and expose no retry tool.

This action retries local preparation only. It does not retry an installation,
repeat a service operation, restore credentials, authenticate an account,
attach tools or resume an original agent task. The existing
[stop contract](MCP.md#stop-one-local-setup-job) applies to the new attempt;
stopping the predecessor cannot stop its successor. Direct-child settlement
does not establish that every descendant has exited.
