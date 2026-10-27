# Releasing Meta Forge

Meta Forge ships in its own GitHub release, beside Meta's releases:

- tag `forge-v<version>`, where the version comes from `installer/pyproject.toml`;
- packages built by `.github/workflows/forge-release.yml` and attested;
- published as a prerelease that never becomes the repository's latest release.

Meta's own release workflow takes no part. No step reads a secret.

## Pinned inputs

`installer/release/forge-release-inputs.json` (schema 1) pins every file the
packager reads, per target (`linux-x86_64`, `macos-arm64`):

- `files`: a relative name for each input, and its pin. A pin is one of:
  - `{"path", "size", "sha256"}`: a file committed under `installer/release/inputs/`;
  - `{"url", "size", "sha256"}`: a public file;
  - `{"url", "archive_size", "archive_sha256", "member", "size", "sha256"}`:
    one file taken out of a pinned `.tar.gz`.
  URLs must be Meta or Connect release downloads, or PyPI files.
- `selection`: the product owner's `forge-meta-toolkit-runtime-selection-v1`
  document. Its artifact sizes and SHA-256 values must equal the pins.
- `paths`: where each selected artifact sits among `files`, under
  `artifact_root`, in the form `launchkit.packaged_runtime.relocate_selection` reads.
- `gateway`: the pinned guide gateway, outside the product root.
- `ordinary_gateway` (optional): the pinned `rpc_gateway` member of the
  `meta-rpc-gateway-<version>-<triple>.tar.gz` Meta release asset whose version
  equals the selection's Meta version, for the target's own triple
  (`x86_64-unknown-linux-gnu` for `linux-x86_64`, `aarch64-apple-darwin` for
  `macos-arm64`), outside the product root. The package
  then also builds applications without a connection setup helper. Pin the
  asset of a published Meta release; its SHA-256 is never written in source.

`guide` holds the guide's Meta and Core versions and their source commits.
Inputs must be public, and they must not carry private paths, user names or
host details. A target without pins is not built.

`installer/release/inputs/forge-release-inputs.skeleton.json` holds the
`linux-x86_64` inputs of the Guide's selected Meta release (`SELECTED_VERSION`
in `launchkit/guide_sdk.py`), filled from that published release: the guide
versions, the Meta and Core release commits, and the gateway's release URL,
archive and member sizes and SHA-256 values. `null` marks what only the product
owner supplies: its selection and paths, whose files are not listed yet. The
release protocol never reads the skeleton, and the inputs schema refuses it
until every value is filled. Add the owner's selection, its paths and their
pinned files, then replace `forge-release-inputs.json` with it.

## Forge CI evidence

A candidate does not rerun Forge's own suite. Instead it binds a green
dispatched `forge.yml` run (backend and frontend) that tested exactly the
Forge inputs at the candidate's commit. A pull request run never counts: it
tests GitHub's merge of its head into the base, not its head commit. The
binding covers:

- the trees of `installer`, `console`, `packages/console-core`,
  `packages/console-components` and `sdk/python`;
- `forge.yml`, the lockfiles, the installer requirements and the vendored wheels;
- `crates/meerkat-meta/src/rpc_gateway.rs`, whose advertised shutdown horizon a
  Forge test pins;
- a recorded toolchain that fits `forge.yml`'s declared Python and Node versions.

The `backend` job runs only after every shard of the backend suite has passed,
and it records the one toolchain all shards recorded.

Evidence and packages are matched to one run attempt. A rerun of only the
failed jobs leaves the other jobs' records under the earlier attempt, so it
is not complete evidence, for a `forge.yml` run or for a candidate. Dispatch
a fresh run, or rerun all jobs, when the run must count.

If no run qualifies, the candidate refuses and names the members that changed.
Dispatch `forge.yml` on main for the commit (`gh workflow run forge.yml --ref main`)
and let it pass, then run the candidate again.

## Steps

1. Dry run, from any branch. It builds, verifies and attests packages, but its
   candidate can never be published:
   `gh workflow run forge-release.yml --ref <branch> -f release_mode=candidate -f source_sha=<branch head>`
2. Candidate, on main at the release commit, once `ci.yml` is green for that
   commit and a dispatched `forge.yml` run has passed for the same Forge inputs:
   `gh workflow run forge-release.yml --ref main -f release_mode=candidate -f source_sha=<main head>`.
   Keep the selection JSON from the run summary.
3. Review the manifest and the packages in the run's artifacts.
4. Tag the candidate's commit and push the tag, which publication checks on
   the remote:
   `git tag -a forge-v<version> <source_sha> -m "Meta Forge <version>"`, then
   `git push origin forge-v<version>`.
5. Publish:
   `gh workflow run forge-release.yml --ref main -f release_mode=publish -f artifact_selection='<selection JSON>'`.
   Publication re-verifies every attestation against main and binds the draft
   to the selection first. It then publishes the draft as a prerelease and
   checks that Meta's latest release has not changed.

To verify a download (the release notes give the same command with the
candidate's `--source-digest`):

```bash
gh attestation verify meta-forge-<version>-<target>.zip --repo lukacf/meerkat-meta \
  --signer-workflow lukacf/meerkat-meta/.github/workflows/forge-release.yml \
  --source-ref refs/heads/main
sha256sum -c meta-forge-<version>-<target>.zip.sha256
```

Each package's README states the destination it supports. Nothing else is
qualified.
