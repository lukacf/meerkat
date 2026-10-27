# Acquire an agent-selected Node package

`setup_acquire_node` prepares a chosen public npm package and its exact archive
closure for the existing selected Node installer. Conditional optional closures
require matching acquisition and installer versions; older packaged editions
retain their own supported subset. The agent researches the
request and chooses the package/version. npm resolves dependencies; Toolkit
checks the resulting artifacts. No intent matching or entrypoint inference is
performed.

## Research public npm candidates

`setup_search_node` closes the earlier gap where a setup-only agent needed an
external research tool before it could name an exact npm package. Enable it
separately with `--allow-node-search` and the same pinned Node/npm profile below.
The ordinary preparer and bootstrap expose `--enable-node-search`; see
[host preparation](HOST-PREPARATION.md#add-public-npm-search).

The input is exactly `{"query":"MCP local files"}`. This literal text is sent
publicly to `registry.npmjs.org`; do not include secrets. Queries must be trimmed,
nonempty, at most 512 UTF-8 bytes, contain no Unicode control characters, and
have no whitespace-separated term beginning with `/` or `-`. Toolkit passes the
query as one argument after `--`; npm regular expressions and CLI options are
not exposed. The fixed command uses npm JSON search with a limit of 20 results.

Results have scope `untrusted_public_npm_search_evidence` and preserve registry
order. Each candidate contains validated exact `name` and `version`, a description
limited to 2048 UTF-8 bytes with control characters removed, and derived HTTPS
npm package/version and registry metadata links. Other provider fields and raw
diagnostics are omitted. The whole captured response is limited to 256 KiB;
malformed or oversized results fail with `node_search_failed`. An empty result
is valid. Results are fresh observations, never instructions, authentication,
catalog adoption, selection or installation authority.

Search uses the existing owned process runner with a 30-second process deadline,
fresh temporary cwd, HOME, empty user/global npm configuration and cache, and a
minimal environment without inherited npm credentials, proxies or Node options.
Temporary search files are removed on success and failure. MCP cancellation and
host close join the bounded call; they do not detach a registry worker. No
acquisition request, staging artifact or latest installation job is created.
This preserves the existing trusted npm transport boundary, not a new network
sandbox.

The optional profile Boolean `allow_acquisition` defaults to `true` for existing
explicit acquisition profiles. Set it to `false` for search without acquisition;
only exact JSON Booleans are accepted. This profile needs no selected installer,
validator or staging authority. The profile and full npm tree remain frozen and
verified, so changing `false` to `true` invalidates that session. A fresh host
must explicitly supply a new profile and any installation permissions. Search
is disabled by default and unavailable in recovery-only sessions. Search itself
never enables `setup_acquire_node`; that tool is both hidden and refused when
`allow_acquisition` is false.

The agent still decides whether a candidate addresses the user's need and chooses
an exact package/version. If acquisition is separately enabled, use
`setup_acquire_node`, inspect the retained package, author explicit metadata,
then check and install through the existing selected-package tools. The curated
catalog remains a useful starting point rather than the limit of discovery.

## Host configuration

Enable the [selected Node profile](SELECTED.md) and supply:

```text
--node-acquisition-profile /ABS/host/node-acquisition.json
--node-acquisition-profile-sha256 EXACT_PROFILE_SHA256
```

The host profile has these required fields:

```json
{
  "schema_version": 1,
  "scope": "trusted_npm_registry_acquisition",
  "python_sha256": "SHA256_OF_HOST_PYTHON",
  "node_sha256": "SHA256_OF_HOST_NODE",
  "npm": {
    "path": "/ABS/trusted/npm-12.2.0.tgz",
    "sha256": "SHA256_OF_COMPLETE_NPM_DISTRIBUTION"
  }
}
```

An optional `target` field selects `macos-arm64` (the default) or `linux-x86_64`.
For Linux, add `"target": "linux-x86_64"` to this host profile. The agent's
acquisition request cannot choose or override the target.

Paths must be canonical and absolute. Python runs the acquisition worker and
Node runs npm. Both targets require Node 24.15 or later in major 24 and npm
12.2.0. The selected Python process must run natively on the chosen system and
architecture; Linux requires a 64-bit x86_64 process. A supervised Node probe
checks the actual platform, architecture and version before any registry request.
The npm worker checks them again before resolution. This is native acquisition,
not cross-compilation or a target emulation mode.

Pin the complete official npm distribution; the entry JavaScript file
alone does not identify the sibling modules it loads. The host obtains and
verifies that tool distribution through its trusted installation process.

To share setup with library integrations using a different Node version, pass
both `--selected-node` and `--selected-npm-cli` as described in
[selected host configuration](SELECTED.md#configure-the-host). This profile's
`node_sha256` must match the effective selected Node executable. Acquisition
uses the complete npm distribution pinned above; later offline installation
uses the selected npm CLI. Library `--node`/`--npm-cli` remain unchanged. If both
selected options are omitted, acquisition and installation use the original
shared Node/npm pair. A partial override is refused before workspace creation.

The setup host freezes the full npm payload, worker, profile and existing
installer. It rejects changed files, added files/directories and symlinks in
that frozen tree. Host executables and inputs are rechecked before publication.
The host must maintain exclusive custody throughout; these checks are not
protection from a malicious concurrent filesystem writer.

## Agent flow

1. Choose an exact package name and SemVer version from research.
2. Call `setup_acquire_node` with `request_id` (32 lowercase hexadecimal
   characters), `package` and `version`.
3. Poll `setup_job` or `setup_state`. Success returns
   `scope: retained_node_package_inputs`, a staging child ID, original URLs,
   identities, integrity values and SHA-256 hashes.
4. Inspect the retained primary archive without executing it, using ordinary
   research tools or host-enabled [acquired archive inspection](ACQUIRED-INSPECTION.md). Author Toolkit
   `package.json` and `activation.json` using the actual operations, JavaScript
   entrypoint, literal arguments and host-supplied policy/lifecycle references.
5. Continue through `setup_selected_check` and `setup_selected_install`.
   The canonical inspector and existing offline installer validate the result.

The prepared layout contains `artifact.tgz`, `closure/package-json`,
`closure/package-lock` and every lock-referenced dependency archive, including
archives that will be excluded for the selected target. Conditional closures
also contain the host-generated `closure/npm-target`; bind its exact bytes as
the `npm-target` entry in the selected `PinnedClosure`, alongside the project,
lock and all archives. The npm project
selects only `file:./artifact.tgz`; the lock bytes emitted by npm are retained
unchanged. Toolkit's package metadata stays separate from this npm project.
The acquisition receipt is written last and is not an installation or readiness
receipt.

Linux receipts include `target: linux-x86_64`; Mac receipts preserve the existing
shape without a target field. The job retains the host-owned `probe.json` and
`resolve.json` settings, the `preflight.json` Node observation and the matching
`runtime.json` resolution observation. All four must agree before publication.

For a conditional graph, `closure/npm-target` binds the observed Node executable
SHA-256, version, module ABI, platform, architecture and libc (the observed glibc
version on Linux, null on macOS). These facts come from the existing host-owned
probe, not an agent-selected target. Before offline npm installation, the
installer requires the actual selected Node runtime to match that complete
binding. Moving the bundle to a different target requires a new acquisition;
editing its lock or target record is not a compatibility repair.

The parser checks the full lock and all archives before selecting a realizable
graph. A platform-incompatible optional branch may be excluded with an explicit
reason; required dependencies and shared required rows cannot be omitted.
Dependencies within a selected subtree remain required unless their own edge is
optional. Compatible selected optional packages must appear with their exact
files in installed readback even if npm reports success. The receipt records
`npm_target` and `excluded_optional_packages`. Unconditional closures retain
their existing behavior. Acquisition success does not establish later runtime
startup, authentication, native grants or usability.

The existing setup controller owns jobs, stop, process join and request replay.
A request ID can identify only one acquisition kind and exact argument set in
that live session. Repeating it returns the same job, including after failure.
Changed arguments or reuse between Python and Node refuse. A deliberate retry
requires a new ID/destination. Restart does not reconstruct this live job map.

## Execution boundary

The profile authorizes trusted npm to contact the public registry. npm runs
lock-only with lifecycle scripts and npm extensions disabled and fixed source controls:
`--allow-git=none --allow-remote=none --allow-directory=none --allow-file=root`.
Only the root's retained local archive uses a file dependency. Ambient npm,
Node, proxy and user configuration are excluded. Private empty user/global
configuration files, home, cache and temporary directories are used.

Artifact and metadata downloads use exact HTTPS `registry.npmjs.org` URLs,
without redirects or proxy inheritance. Network parsing runs in the existing
Runner-owned child process, so an overall deadline or explicit stop can
terminate a peer that trickles headers or chunk framing. npm also runs in that
same supervised process path. No selected package is imported or started.

The public registry and pinned npm remain trusted network participants. Fixed
registry options are not an egress firewall. Per-file size, CPU and Node heap
limits do not establish a hard aggregate disk/RSS limit or a filesystem sandbox.
Hosts that require those boundaries must supply actual process/network isolation.
The acquisition deadline is 120 seconds; each archive is at most 64 MiB, retained
inputs at most 512 MiB, and there are at most 255 unique package archives. Existing
installer expansion, path, member-count and complete graph checks still apply.

The bounded optional/platform subset requires the exact target binding above.
Conditional graphs with peer metadata or `devOptional`, unsupported libc,
workspaces and shrinkwrap remain unsupported. Bundled dependencies, aliases and
links remain refused. Required dependency failures and optional acquisition,
extraction, engine or script failures do not become successful omissions. npm
success alone is not a complete installed-graph proof. Packages requiring native
compilation still need a separately supported backend; explicitly selected
prebuilt addons retain their separate contract. Unsupported selections fail
explicitly; their lock is never edited to manufacture compatibility.

This is software acquisition, not native `find_and_install`, protected account
authentication, authority grants, attachment or original-task continuation.

## Recover retained inputs

Keep the returned `recovery_request` and original workspace. A new readback-only
host can check the final receipt and acquired files without downloading or
pulling again. Follow [acquisition recovery](ACQUISITION-RECOVERY.md). A matching
file check leaves the historical job outcome unchanged and supplies no execution
or account authority.

Readback verifies the target against the frozen acquisition profile, receipt and
retained worker observations without executing the old Python, Node or npm.
Missing or inconsistent Linux target proofs remain unconfirmed. Older Mac
acquisitions without the new probe files retain their existing readback path.

For conditional closures, readback also recomputes the selected graph and checks
the retained target/omission record. It executes no runtime probe and does not
establish the reader host's current OS or libc compatibility.
