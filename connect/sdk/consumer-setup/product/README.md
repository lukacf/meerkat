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

Preparation enables the accepted Agno, Services and Activepieces library routes
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

`scripts/package-connect-resources.py` uses the exact selected portable-JSON
archive and paired starter, not a rebuild from the current source tree. It
accepts already acquired, hash-pinned upstream runtime archives and builds a
reproducible resource archive. It keeps all selected runtime files and licenses,
including Python's full-build `PYTHON.json` and dependency license texts. The
previously acquired pip 26.2.1 zipapp is hash-pinned separately, with its MIT
notice alongside the resource notices and vendored notices retained inside the
zipapp. Its source URL identifies the upstream project, not a new download.

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
