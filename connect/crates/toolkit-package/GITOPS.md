# Prepare a Kubernetes GitOps change

`toolkit-gitops-prepare` turns one explicit package selection and host profile
into an actual Deployment replacement and a new immutable ConfigMap. It reads
one JSON request on stdin and emits one `prepared` proposal on stdout. It does
not access Git, a registry, credentials or a cluster.

Build through the repository wrapper:

```sh
./scripts/repo-cargo build -p toolkit-package --bin toolkit-gitops-prepare
./scripts/repo-cargo --print-env
```

Invoke `toolkit-gitops-prepare` from the printed `CARGO_TARGET_DIR/debug`
directory, redirecting the host's request file to stdin. The library entry
points are `prepare_kubernetes_gitops_json` and `prepare_kubernetes_gitops`.
The latter accepts an existing `ValidatedManifest`; both reuse canonical
package and host compatibility rules.

## Supported selection

The current profile prepares one independent, stateless, self-contained
container with Deployment lifetime and one unauthenticated service export.
Dependencies, state, frontend roles, events, host services, additional required
component contracts and remote/host-provided sources are unsupported. A host
must offer the selected service enforcement contract. Compatibility declares
what the host can implement; it does not authorize the selected operation.

The artifact location uses the HTTPS OCI Distribution form
`https://REGISTRY/v2/REPOSITORY/manifests/sha256:DIGEST`. Its digest must equal
the selected artifact digest. The generated image reference is
`REGISTRY/REPOSITORY@sha256:DIGEST`. This stage checks references without fetching
or verifying image bytes. Nondefault ports, credentials, query strings,
fragments and encoded path segments are unsupported.
Repository segments use lowercase letters/digits separated by one period,
one underscore or a run of hyphens. `REGISTRY/REPOSITORY` is at most 255 bytes.
This is a conservative subset of registry reference syntax.

The host's existing container has `workingDir: "/"`. The selected relative
executable must contain a slash and is projected as the sole Kubernetes
`command` item, with literal arguments in `args`. Dollar signs are unsupported
because Kubernetes expands variable references in these fields. There is no
shell or image-default command inference. The host must validate that the
selected image implements the declared entrypoint and consumes the supplied
configuration files.

## Request and host profile

The request has `schema_version: 1`, raw JSON strings `package_json`,
`manifest_json`, `profile_json`, the exact `profile_sha256`, `base_documents`
mapping two relative repository paths to their original raw JSON strings, and
nonsecret UTF-8 `configuration` and `policy` strings. Their hashes must match
the manifest. Duplicate JSON keys and excessive inputs refuse.

The profile is a host-selected input, not an agent-selected authorization:

- `owner`, exact HTTPS `source_repository`, full hexadecimal `source_commit`,
  `host` capabilities and `placement`.
- `deployment`: existing `path`, raw `sha256`, `namespace`, `name`, and exact
  `container` name.
- `config_map`: existing template `path`, raw `sha256`, `namespace`, `name`,
  `configuration_key`, `policy_key`, `volume`, `mount_path`,
  `configuration_file`, `policy_file`, and `snapshot_directory`.

The existing Deployment must mount exactly those two keys read-only into the
selected container. Shared consumption of that volume by other regular,
init or ephemeral containers refuses. Ambiguous targets, conflicting mounts,
cross-namespace references and runtime-owned ConfigMap metadata also refuse.
The template ConfigMap is never modified. It can already be immutable.

The generated ConfigMap has `immutable: true` and a name derived from the
selection, configuration, policy and exact profile. The new Deployment refers
to that name. The pod template receives the reserved annotations
`toolkit.rkat.ai/manifest-sha256`, `toolkit.rkat.ai/configuration-sha256` and
`toolkit.rkat.ai/policy-sha256`. Existing valid values in that reserved namespace
are updated; other JSON fields are preserved. Serialization can change
whitespace and key ordering.

## Consuming a prepared proposal

The result includes the full canonical selection, source identity, exact base
hashes, profile/capability hashes, output bytes and their hashes, and an overall
`proposal_sha256`. A file with `action: "replace"` requires its expected old
bytes; `action: "create"` requires an absent destination. These are proposals,
not reusable authorization receipts.

The trusted host must check the repository and base commit, revalidate current
profile and input bytes, enforce the path allowlist and all file preconditions,
and stage the complete change together in its own isolated checkout. It must
obtain any required approval against that exact payload, run its schema,
admission and image checks, and submit through its deployment workflow. Changed
inputs require a newly prepared and approved proposal. Repeated preparation
with identical inputs is deterministic; this does not grant permission to
repeat a deployment effect.

### Stage a local review checkout

The trusted-host command `scripts/stage-gitops-proposal.py` performs the local
staging step with Python's standard library, an explicitly selected Git
executable and the existing canonical planner. It accepts the original raw
request and expected canonical proposal digest. It invokes that pinned planner
again, then checks the result against the exact committed base and the paths
already selected in the host profile. It does not accept an independently
authored patch or interpret Kubernetes fields itself.

```sh
python3 scripts/stage-gitops-proposal.py \
  --source-repository /absolute/path/to/source-repository \
  --request /absolute/path/to/request.json \
  --request-sha256 "$REQUEST_SHA256" \
  --proposal-sha256 "$PROPOSAL_SHA256" \
  --planner /absolute/path/to/toolkit-gitops-prepare \
  --planner-sha256 "$PLANNER_SHA256" \
  --git /absolute/path/to/git \
  --git-sha256 "$GIT_SHA256" \
  --output /absolute/path/to/new-review-output \
  --timeout-seconds 120
```

The host supplies the four expected digests from its selected inputs and tools;
they are not discovered or trusted from a candidate package. The source's
literal `remote.origin.url` must equal the profile's repository URL. This is a
local configuration association, not authentication of an upstream repository.
The selected full SHA-1 commit must already exist locally. It need not be the
source checkout's current `HEAD`. Dirty tracked files, staged changes, untracked
files and source refs are left alone and excluded from the new checkout.

The source must be an ordinary complete repository with a `.git` directory.
Linked worktrees, shallow or partial repositories, object alternates, grafts,
submodules and committed symlinks are unsupported. The command disables replace
refs, lazy fetch, ambient Git configuration and repository hooks. It uses an
empty Git template and raw object/index operations rather than checkout or
`git add`, so source filters and `.gitattributes` conversions do not run. It
copies reachable history to an independent object store, removes its temporary
read-only source alternate, and checks the result before publishing a receipt.
It never fetches, commits, pushes or contacts a registry or cluster.

Input paths and the output parent are resolved once to canonical host paths.
The host must keep those locations under exclusive custody for the invocation;
this is not a sandbox against concurrent filesystem writers. Control characters
in host paths, case/Unicode-normalization collisions in tree paths, and output
overlap with selected input locations refuse. The destination must be absent,
even when a previous output is empty or failed.

The new output contains `checkout/`, `proposal.json`, `staged.patch`,
`commands.json` and, on successful staging, `receipt.json`. The checkout retains
the exact selected base commit and has only the proposal's two changes staged.
The receipt binds request, proposal, tools, staged tree, file contents and patch
hashes, with status `staged_proposal_only`. Approval, image checks, deployment,
readiness and original-agent continuation remain separate owner actions.

The command retains partial output on failure. An absent success receipt means
that the output is unconfirmed, including a complete-looking checkout. After
receipt publication, interruption or an output error can still prevent the
caller from receiving success: preserve and inspect the receipt and its bound
files rather than automatically replaying the command. `failure.json` may
coexist with a completed receipt in that case. A new attempt always requires a
new output path; the command never repairs or overwrites an old one.

The request and planner output are each bounded to 2 MiB. Individual tree files
are bounded to 32 MiB and the selected tree to 128 MiB/10,000 files. The source
object store is bounded to 256 MiB/100,000 entries; child output files are bounded
to 256 MiB and the copied store is checked against the same aggregate budget.
The required deadline is positive and at most 300 seconds. Child pipes and
process waits are bounded by it, with up to four additional seconds for process
group termination and reaping. Synchronous local filesystem work uses deadline
checkpoints; it is not a guarantee against an indefinitely stalled filesystem.

Actual local-planner, source-preservation and refusal evidence is retained in
`docs/verification/20261004-gitops-staging`. This command is a trusted-host
staging surface, not an agent tool granting repository or deployment authority.
The focused tests require an explicitly selected planner and expected digest;
missing inputs fail rather than silently skipping or using a local fallback:

```sh
TOOLKIT_GITOPS_PLANNER=/absolute/path/to/toolkit-gitops-prepare \
TOOLKIT_GITOPS_PLANNER_SHA256="$PLANNER_SHA256" \
  python3 scripts/test_gitops_staging.py
```

Retain earlier ConfigMap snapshots until the deployment owner can safely
retire them. Readiness, shutdown, fencing, rollback and role requirements remain
in the manifest for their existing owners to fulfill. A successful preparation
or Git patch application does not satisfy any of those requirements.
Changing a pod template can restart every container in that pod, including
preserved sidecars. The host's exact-payload approval must assess that whole-pod
disruption; preserved configuration fields do not establish runtime isolation.

The bounded example and actual offline Git/schema acceptance are retained in
`docs/verification/20261004-gitops-preparation`. This is a native Kubernetes
JSON profile. An organizational host's private Helm chart, composite deployment and native
activation still require their own implementations and acceptance.
