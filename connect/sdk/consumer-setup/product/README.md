# Installed Connect resources

This is the internal resource contract for the standalone `meerkat-connect`
product. It supplies the accepted setup starter and library archive together
with complete Python and Node/npm runtimes and pinned pip for inert Python
package acquisition. Ordinary users do not select runtime
paths, copy MCP configuration, or operate a setup server.

The first profile is macOS arm64, macOS 13.5 or newer. CPython 3.11 is required by
the selected archive's wheel ABI. Linux remains required product work and is not
qualified by this resource package.

## Layout and trust

```text
<prefix>/bin/meerkat-connect
<prefix>/libexec/meerkat-connect/resources.json
<prefix>/libexec/meerkat-connect/prepare.py
<prefix>/libexec/meerkat-connect/runtime/python/
<prefix>/libexec/meerkat-connect/runtime/node/
<prefix>/libexec/meerkat-connect/runtime/pip.pyz
<prefix>/libexec/meerkat-connect/starter/
<prefix>/libexec/meerkat-connect/archives/
<prefix>/libexec/meerkat-connect/notices/python/
```

The CLI discovers the prefix from its own installed executable. Its build embeds
the exact `resources.json` digest in `MEERKAT_CONNECT_RESOURCES_SHA256`; this is a
maintainer build input, never a user runtime override. Before executing Python,
the native caller verifies that digest, target and the complete resource tree,
including Python's interpreter, libraries and standard library. Verification
inside Python would be too late to establish those pre-execution facts.

`resources.json` version 1 contains `kind: "meerkat_connect_resources"`, `target`,
the root-relative paths `prepare`, `python`, `node`, `npm_cli`, `starter` and
`archive`, and these exact sorted `entries`:

- File: `path`, `kind: "file"`, `size`, `sha256`, `mode`.
- Directory: `path`, `kind: "directory"`, `mode`.
- Symlink: `path`, `kind: "symlink"`, `target`, `mode`.

Modes are integer Unix permission bits. Paths are unique normalized relative
paths without traversal. Symlinks are relative, resolve inside the resource root
and have no cycles. `resources.json` itself is the only excluded tree entry; its
digest is the external build anchor. `sources` records each selected upstream
archive's name, URL, size and SHA-256. These provenance records are not new grant
authority. Reject unknown or changed entries before starting any resource code.

Resource files are 0444 or 0555 and resource directories are 0555. The supplied
archive extraction must preserve these modes (`tar -xpf`). This keeps ordinary
Python bytecode writes from changing the pinned runtime. It does not protect
against an administrator or malicious same-user owner changing permissions.
The installation must remain stable for the duration of preparation and use.

An optional `library_release` selects the complete extracted library inside
these resources. Its only accepted spelling is `library/<archive filename
without .tar.gz>`, derived from `archive`. Both directories are 0555; all
descendants are manifest-covered directories (0555) or regular files (0444 or
0555). Links and writable members are refused. Omission preserves legacy
extraction beneath the preparation directory. Explicit null, an invalid path
or a missing selected directory never falls back to that legacy behavior.

## Preparation and the native handoff

The native owner supplies a fresh absolute preparation directory below its
selected realm's product-artifact directory, separate from native realm data,
configuration and approval state. It invokes the verified interpreter with
`-I -B`, `prepare.py`, and `--destination <fresh-directory>`. It supplies a clean
environment without Python/Node lookup, user site-packages or model credentials.

`prepare.py` rechecks resources, calls the selected `start.py` owner
directly, and binds `installed.json`, `prepared.json` and every generated
configuration file to their returned owner hashes. It returns only the existing
standard `mcpServers.toolkit-setup` command/args projection on stdout. The native
host attaches that exact stdio transport and owns its lifetime. The preparation
process is not an MCP supervisor or an agent runtime.

With `library_release` selected, the starter validates the entire resource
library against the pinned archive before any archived owner runs. Recipe
initialization, bootstrap, inspection and validation tools, local-session
projection, retained discovery, installation readback and later round offers
all use that same resource library. Client files, configuration, staging,
sessions and the recipe database remain in mutable state. Preparation does not
extract an executable library into that state.

Installation readback requires the original preparation receipt's archive hash
to match this selected library archive. It checks that the receipt remains
unchanged through the canonical owner's read, including for non-local sessions.

This does not make the complete preparation tree compatible with a `noexec`
mount. Bootstrap still installs the setup client into `prepared/client/runtime`,
and service installation creates environments below
`prepared/session/installations`. The Linux Services closure includes native
Python extensions, including `pydantic_core` and `rpds`, that need executable
file-backed mappings when imported. The MCP setup server uses the generated
client environment. Keeping its interpreter and the archived library in the
read-only resource prefix does not relocate those installed extensions.

Before starting preparation on Linux, the product reads the canonical
preparation parent's mount flags using `statvfs`. An observed `ST_NOEXEC` refuses
with the private diagnostic phase `destination_filesystem` and code
`destination_noexec`, before any starter, recipe initialization or client
installation. An unavailable flag API or invalid observation refuses as
`destination_mount_unconfirmed`; operating-system errors retain their existing
closed diagnostic category. No executable probe, fallback location, remount or
retry is performed. Absence of the flag is not an execution-readiness guarantee,
and the mount must remain stable throughout preparation and later use.
Later local setup rounds recheck the selected preparation directory's mount
before offering a continuation, after receipt validation and the cancellation check.

Host storage policy remains the host owner's responsibility. Supporting a
fully non-executable mutable state tree requires a separate runtime installation
layout and installed Linux qualification; `library_release` alone does not
provide that support.

`product-prepared.json` retains the existing owner result and resource digest.
Any error, signal or missing response remains unconfirmed; retain the directory
and its logs. A fresh path is not an automatic retry or recovery authority. There
is no silent cache, reused partially prepared environment, or install replay.

SIGINT, SIGTERM and SIGHUP set the same stop event passed into the accepted
starter. Its existing runner settles its directly owned child: SIGTERM with a
four-second wait, then its process-group SIGKILL and a two-second wait if needed.
The CLI signals and reaps this preparer, without adding a competing descendant
supervisor. Synchronous validation/extraction completes between stop checkpoints;
a signal receipt alone does not establish that all descendants have settled.

By default, preparation enables the accepted Agno, Services and Activepieces library routes
and package inspection. Service preparation also enables the existing trusted
PyPI wheel acquisition and acquired-archive inspector. The agent supplies an
exact distribution, version and request ID; the owner returns bounded original
wheels, source URLs and hashes. It never runs selected package code. The same
agent authors exact metadata and uses the existing checked-install approval
where that composition enables installation. CLI preparation alone does not
enable installation. Frontend-only and legacy preparation-only sessions do not
gain these acquisition tools. Caller ceilings still apply.

Pip must be present in the verified resource closure. Missing or changed bytes
refuse preparation; no system pip, download-at-start fallback, existing workspace
reuse or automatic recovery replay is attempted. Reconciliation keeps its
existing read-only authority. Node/npm, MCPB and other acquisition owners remain
required scope but are not enabled by this Python increment. Authentication,
grants, attachment and task continuation still require their native owners.

## Maintainer assembly

Pass `--immutable-library-release` with the complete explicit library and
current starter selection (`--library`, `--library-sha256`, `--library-bytes`,
`--starter`, `--release-manifest-sha256`) to assemble this layout. Packaging
extracts inert bytes from the same captured archive used for compatibility
inspection, then applies the checkout's existing whole-release validator. It
does not execute archived code. Existing archive, target, membership, hash and
mode checks remain in force. The final resource manifest covers both archive
and extracted library, and `build.json` reports the selection, manifest size,
archive member count, member content size, largest member and resource entry
count for installer-bound checks.

The paired starter must implement the internal `release_dir` keyword. An older
starter that lacks it refuses the explicit selection. Ship regenerated full
resources and a native binary built against their exact manifest digest; a
loose script overlay is insufficient. Existing prepared states retain their
original resource digest and are not migrated or relabeled.

For an explicit family selection, pass `--families` to
`scripts/package-connect-resources.py` together with its complete explicit
library and starter inputs. For example,
`--families agno,services,activepieces,official-github` records that exact string
in the optional `resources.json` field `families`. The native manifest digest
binds the selection; product preparation forwards it through the selected
starter to the existing host preparer. Omission preserves the defaults above.
The archived preparer rejects empty, duplicate or unsupported families and
requires the selected official component for the exact target. Merely shipping
official payloads does not enable them. A new selection requires packaging
resources and building the native binary against the resulting manifest digest.
This selection does not authorize credential submission or provider operations.

For GitHub Enterprise, select a separate Linux library input bundle first.
The existing `scripts/package-github-enterprise.py` helper accepts only the
pinned v2.0.3 upstream archive at revision
`b98d115bbb080e9555466a96656ec4df605aea21`. Reuse retained verified inputs.
Its output is an offline, non-executable bundle with an exact `CONTENTS.json`
digest, without a `SHA256SUMS` file. The library packager accepts this additional
field in its hash-pinned `--input-selection` document:

```json
"official": {
  "bundle": "dist/selected-enterprise",
  "contents": "<exact CONTENTS.json SHA256>",
  "profile": "enterprise-selected-reads"
}
```

The document must also explicitly pin the supporting installer under
`sources["setup/install-library-consumer.py"]` and backend under
`sources["setup/backend.py"]`; select other coherent source changes through the
same existing source fields. Paths are relative
to the packager's input root. This selection preserves the supplied bundle bytes
and records the exact input document in the library archive. It requires Linux
x86_64 and matching delivery metadata. The frozen default inputs stay unchanged.
It does not associate the GitHub.com catalog identity with Enterprise: use the
explicit `official-github` family and its advertised `enterprise-selected-reads`
profile. Consumer inputs with precomposed official catalog routes are refused.

Package that new library archive with its paired starter, current product owner,
and an explicit `--families` selection including `official-github`. All native
binaries in the future composition must bind the resulting resource manifest.
Adding the family to an old GitHub.com archive does not select Enterprise, and
source support alone does not change installed resources.

Product retained-installation and credential readback reuse the selected
installer's exact profile, host, launch and configuration checks. Credential
operations must be a subset of the installed selection, even when the profile
supports more tools. The only credential slot remains
`GITHUB_PERSONAL_ACCESS_TOKEN`; no credential value travels in these documents.
Repository/method scope, protected credentials, live provider qualification and
host admission remain with their existing owners.

`scripts/package-connect-resources.py` uses the exact selected portable-JSON
archive and paired starter, not a rebuild from the current source tree. It
accepts already acquired, hash-pinned upstream runtime archives and builds a
reproducible resource archive. It keeps all selected runtime files and licenses,
including Python's full-build `PYTHON.json` and dependency license texts. The
previously acquired pip 26.2.1 zipapp is hash-pinned separately, with its MIT
notice alongside the resource notices and vendored notices retained inside the
zipapp. Its source URL identifies the upstream project, not a new download.

Python and Node archives are copied into private temporary files and checked
against their selected size and SHA-256 before decompression or extraction.
Library compatibility inspection and the packaged library also consume the same
verified copy in both explicit and default selection modes. Replacing an input
pathname after capture cannot substitute new bytes; a changed input at capture
refuses assembly. Captures are streamed with bounded memory and removed when
their consumer finishes or fails. Copied starter files are checked again before
the resource manifest or success receipt is emitted.

Each runtime archive is preflighted before its mapped entries are created. The
limits are 50,000 mapped entries and 1 GiB of declared mapped content, including
the final member. Duplicate filesystem paths, file or link parents, existing
file conflicts, escaping links and link cycles refuse extraction before that
archive writes output. Link resolution includes links declared later in the
archive and is bounded to 40 traversals per link. Existing real directories may
be shared by the separately selected runtime archives; existing files are never
replaced. The final manifest still verifies the complete assembled tree.

Run the inert packaging regressions from the repository root with
`python3.11 -m unittest discover -s scripts -p 'test_package_connect_resource*.py'`.
They use temporary local fixtures and do not execute supplied runtimes or
qualify a native product build.

The CPython distribution is the maintained
[Python Build Standalone release](https://github.com/astral-sh/python-build-standalone/releases/tag/20260901).
Its [distribution documentation](https://github.com/astral-sh/python-build-standalone/blob/20260901/docs/running.rst)
describes relocatable installation payloads and notices. Node comes from the
[official 24.19.0 distribution](https://nodejs.org/download/release/v24.19.0/),
checked against its [official checksums](https://nodejs.org/download/release/v24.19.0/SHASUMS256.txt).
Node's [platform policy](https://github.com/nodejs/node/blob/v24.19.0/BUILDING.md)
sets the macOS 13.5 minimum for this profile.

The resource archive is a component until the separately qualified native
`meerkat-connect` binary, built with its exact manifest digest, is placed in
`bin/` and the complete product is assembled. Resource qualification alone is
not evidence of native agent dispatch, approval, persistence or provider use.
