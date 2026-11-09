# Releasing Meta Forge

Meta Forge ships in its own GitHub release in the monorepo (`lukacf/meerkat`),
beside Core's and Meta's releases:

- tag `meta-installer-v<version>`, where the version comes from
  `meta/installer/pyproject.toml`;
- packages `meta-installer-<version>-<target>.zip`, built by the root workflow
  `.github/workflows/meta-installer-release.yml` (it runs
  `meta/scripts/installer-release.py`) and attested;
- published as a prerelease that never becomes the repository's latest release
  (Core's release stays the latest).

Meta's own release workflow (`meta-release.yml`) takes no part. No step reads a
secret. Releases made before the monorepo (tags `forge-v<version>`) stay in
`lukacf/meerkat-meta`.

## Pinned inputs

`installer/release/forge-release-inputs.json` (schema 1) pins every file the
packager reads, per target (`linux-x86_64`, `macos-arm64`):

- `files`: a relative name for each input, and its pin. A pin is one of:
  - `{"path", "size", "sha256"}`: a file committed under `installer/release/inputs/`;
  - `{"url", "size", "sha256"}`: a public file;
  - `{"url", "archive_size", "archive_sha256", "member", "size", "sha256"}`:
    one file taken out of a pinned `.tar.gz`.
  URLs must be Meta or Connect release downloads (the monorepo's, or the
  standalone repositories' for releases made before it), or PyPI files.
- `selection`: the product owner's `forge-meta-toolkit-runtime-selection-v1`
  document. Its artifact sizes and SHA-256 values must equal the pins. Its
  `artifact_root` is provenance only, since `relocate_selection` replaces it and
  every artifact path with the fetched input directory before any read: name the
  release job's own fetch root on the GitHub-hosted runner,
  `/home/runner/work/_temp/installer-inputs/<paths.artifact_root>`, a generic
  runner path rather than a private path or user name. The inputs carried over
  from the standalone repository name its job's root, `forge-inputs`.
- `paths`: where each selected artifact sits among `files`, under
  `artifact_root`, in the form `launchkit.packaged_runtime.relocate_selection` reads.
- `gateway`: the pinned guide gateway, outside the product root: the
  `rpc_gateway` member of the Guide's Meta release asset
  `meta-rpc-gateway-<version>-<triple>.tar.gz` from the monorepo's
  `meta-v<version>` release, for the target's own triple.
- `guide_sdk`: the Guide's client, outside the product root: that release's
  `meerkat_meta-<version>-py3-none-any.whl` on PyPI. The package carries it for
  Forge's own environment; no Guide wheel is committed.
- `ordinary_gateway` (optional): the pinned `rpc_gateway` member of the
  `meta-rpc-gateway-<version>-<triple>.tar.gz` Meta release asset whose version
  equals the selection's Meta version (released from the monorepo under
  `meta-v<version>`, or before it from `lukacf/meerkat-meta` under
  `v<version>`), for the target's own triple
  (`x86_64-unknown-linux-gnu` for `linux-x86_64`, `aarch64-apple-darwin` for
  `macos-arm64`), outside the product root. The package
  then also builds applications without a connection setup helper. Pin the
  asset of a published Meta release; its SHA-256 is never written in source.

`guide` holds the Guide's Meta release: its version, which Core shares, and its
release commit, which Meta's `meta-v<version>` and Core's `v<version>` tags
both name (`meta_source` and `core_source`). The Guide runs the workspace's own
Meta release (`SELECTED_VERSION` in `launchkit/guide_sdk.py`), so packaging
refuses inputs of another release. These pins exist only once that release is
published: after a release, fill them from it together with the agent-team
runtime (`launchkit/public_meta_runtime.py`: its wheel's SHA-256, its release
fixture under `installer/tests/fixtures/` and its model catalog, recorded with
`python -m launchkit.public_meta_runtime`).
The installer workflow tests the release commit's own `meta/sdk/python`, while
the package carries that release's published client, so a candidate and its
publication refuse unless the commit's `meta/sdk/python` is the tree at
`meta_source`: cut Forge from a commit whose Meta SDK is unchanged since the
Guide's Meta release.
Inputs must be public, and they must not carry private paths, user names or
host details. A target without pins is not built.

`installer/release/inputs/forge-release-inputs.skeleton.json` holds the
`linux-x86_64` inputs of the Guide's Meta release, filled from that published
release: the guide versions, the release commit, the gateway's release URL,
archive and member sizes and SHA-256 values, and the client wheel's URL, size
and SHA-256. `null` marks what only the product owner supplies: its selection
and paths, whose files are not listed yet. The release protocol never reads the
skeleton, and the inputs schema refuses it until every value is filled. Add the
owner's selection, its paths and their pinned files, then replace
`forge-release-inputs.json` with it.

## Forge CI evidence

A candidate does not rerun Forge's own suite. Instead it binds a green
dispatched run of the root workflow `.github/workflows/meta-installer.yml`
(jobs backend and frontend) that tested exactly the Forge inputs at the
candidate's commit. A pull request run never counts: it
tests GitHub's merge of its head into the base, not its head commit. The
binding covers:

- the trees of `meta/installer`, `meta/console`, `meta/packages/console-core`,
  `meta/packages/console-components` and `meta/sdk/python`;
- `meta-installer.yml`, the lockfiles, the installer requirements and the
  vendored wheels;
- `meta/crates/meerkat-meta/src/rpc_gateway.rs`, whose advertised shutdown
  horizon a Forge test pins;
- a recorded toolchain that fits `meta-installer.yml`'s declared Python and
  Node versions.

The `backend` job runs only after every shard of the backend suite has passed,
and it records the one toolchain all shards recorded.

Evidence and packages are matched to one run attempt. A rerun of only the
failed jobs leaves the other jobs' records under the earlier attempt, so it
is not complete evidence, for a `meta-installer.yml` run or for a candidate. Dispatch
a fresh run, or rerun all jobs, when the run must count.

If no run qualifies, the candidate refuses and names the members that changed.
Dispatch `meta-installer.yml` on main for the commit
(`gh workflow run meta-installer.yml --ref main`) and let it pass, then run the
candidate again.

## Steps

1. Dry run, from any branch. It builds, verifies and attests packages, but its
   candidate can never be published:
   `gh workflow run meta-installer-release.yml --ref <branch> -f release_mode=candidate -f source_sha=<branch head>`
2. Candidate, on main at the release commit, once `ci.yml` is green for that
   commit and a dispatched `meta-installer.yml` run has passed for the same
   Forge inputs:
   `gh workflow run meta-installer-release.yml --ref main -f release_mode=candidate -f source_sha=<main head>`.
   Keep the selection JSON from the run summary.
3. Review the manifest and the packages in the run's artifacts.
4. Tag the candidate's commit and push the tag, which publication checks on
   the remote:
   `git tag -a meta-installer-v<version> <source_sha> -m "Meta installer <version>"`, then
   `git push origin meta-installer-v<version>`.
5. Publish with `meta/scripts/installer-publish-dispatch.py`. It reads Forge's
   candidate review projection and Connect's A5, application and Guide join
   records, each checked against the SHA-256 its owner reported. It checks every
   join against the candidate and its release inputs, and builds the request
   from those checked bytes only. While main's head is the merged commit, it
   runs
   `gh workflow run meta-installer-release.yml --ref main -f release_mode=publish -f artifact_selection='<selection JSON>' -f candidate_review=<base64> -f candidate_review_sha256=<sha256>`.
   `candidate_review` carries the exact bytes of Forge's candidate review
   projection (`forge-candidate-review-v1`), whose SHA-256 is given
   separately. Before any tag or release write, publication hashes the
   decoded bytes, parses them strictly and derives the accepted candidate
   from them. It refuses another candidate run, attempt or manifest, other
   package digests, and any outcome or verdict other than `accepted`. Only the
   projection's SHA-256 is printed; the selection asset stays identity-only.
   Publication re-verifies every attestation against main and binds the draft
   to the selection first. It then publishes the draft as a prerelease and
   checks that the repository's latest release (Core's) has not changed.

To verify a download (the release notes give the same command with the
candidate's `--source-digest`):

```bash
gh attestation verify meta-installer-<version>-<target>.zip --repo lukacf/meerkat \
  --signer-workflow lukacf/meerkat/.github/workflows/meta-installer-release.yml \
  --source-ref refs/heads/main
sha256sum -c meta-installer-<version>-<target>.zip.sha256
```

Each package's README states the destination it supports. Nothing else is
qualified.
