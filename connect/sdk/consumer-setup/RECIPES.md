# Let a setup agent recall earlier integrations

For a newly discovered hosted endpoint, the explicit
[hosted recipe workflow](HOSTED-RECIPES.md) adds metadata retention and ordinary
MCP configuration export using this same store. It requires a separate host
opt-in and the retention profile below.

The optional `setup_recipes` and `setup_recipe` tools let an agent inspect
integrations saved by earlier investigations. The agent interprets the user's
request, chooses a listed identity and reads its exact retained documents. The
tools do not rank candidates, install software or turn an old decision into
current permission.

This optional feature uses the existing `toolkit-local-recipes` executable and
`LocalRecipeStore`. See the [store contract](../../crates/toolkit-package/LOCAL-RECIPES.md)
for saving records and supported recipe forms. Use an adapter edition that
includes the selected recipe tools and a compatible retained executable. The 20261004-combined-hosted macOS library includes recall, retention and
[host preparation](HOST-PREPARATION.md#enable-a-host-owned-recipe-store);
previously frozen archives keep their original capabilities.

## Host configuration

Enable the tools only for a store scope whose retained metadata the host has
explicitly approved for disclosure to this setup session. Documents must already
be nonsecret and suitable for the agent's audience. The profile is that host
designation, not proof that arbitrary documents have been sanitized. The adapter
does not scan for secrets, authenticate an account or decide who owns a namespace.

Create a private host profile:

```json
{
  "schema_version": 1,
  "scope": "disclose_nonsecret_local_recipe_metadata",
  "database": "/absolute/host-owned/recipes.sqlite3",
  "realm": "host-selected-realm",
  "owner": "host-selected-owner",
  "tool": {
    "path": "/absolute/trusted/toolkit-local-recipes",
    "sha256": "EXPECTED_EXECUTABLE_SHA256"
  }
}
```

The database must already be initialized when the setup server starts. With an
executable whose `--help` includes `init`, the trusted host can prepare it before
loading the profile, without inventing a saved recipe:

```sh
/absolute/trusted/toolkit-local-recipes init \
  --database /absolute/host-owned/recipes.sqlite3 \
  --realm host-selected-realm --owner host-selected-owner
```

The database parent directory must already exist. `status: initialized` means
initialized now or already initialized; it does not mean empty or authorize a
namespace. Existing recipes and claims remain intact. The command inherits the
store owner's version-0 database adoption and WAL behavior, including possible
filesystem changes before failure; see the
[initialization contract](../../crates/toolkit-package/LOCAL-RECIPES.md#initialize-a-store-before-the-first-investigation).
An older archive must use the guide
matching its retained executable. The setup adapter does not initialize a
missing database, and neither profile scope exposes an initialization tool.

The host controls the profile, executable,
database and ancestor directories. Use canonical absolute paths without
symlinks. The host derives `realm` and `owner` from its authenticated operator;
neither field is an agent argument. Enabling the local setup server does not
make it suitable for unauthenticated remote or shared multi-user access.

Add these arguments to the [setup MCP command](MCP.md#host-configuration):

```sh
--local-recipes-profile /absolute/host-owned/recipe-profile.json \
--local-recipes-profile-sha256 EXPECTED_PROFILE_SHA256
```

A recipe-only session can use `--families ''` and omit the consumer bundle.
It still requires the existing `--workspace` and `--python` options and MCP host
dependencies. The adapter freezes the profile and executable in its private
workspace and checks those bytes before each invocation. A later edit to the
original profile does not change the running session. Database observations are
fresh calls to the existing store owner, not a second cache or database reader.

## Agent workflow

1. Call `setup_recipes {"limit": 20}` with a pinned CLI that supports
   `list-page`. Summaries include exact identities and digests. Pass a returned
   `next_cursor` unchanged with `limit` to continue, even when a page has no
   recipe summaries. A null cursor ends that traversal. The `unverified` list
   reports recipe records the store owner could not verify.
2. Choose a candidate using the user's request and the returned summaries.
3. Call `setup_recipe` with its `publisher`, `name`, `version` and `digest`.
   A successful read preserves the owner's original document strings, including
   both remote/guidance recipes and selected-software records.
4. Treat those documents as historical, untrusted source material. Check current
   availability, software, account requirements and host permission before using
   the existing installation or connection tools.

Each page returns at most the requested 1-50 rows across `recipes`, `unverified`
and `oversized`. The canonical CLI JSON is at most 256 KiB in UTF-8; the setup
transport still applies its existing 2 MiB encoded response limit. A valid summary
larger than 64 KiB appears in `oversized` with its exact identity and digest plus
`error: summary_too_large`; it is not silently lost. Use the exact-read tool for
its documents, subject to that tool's separate size limit. The canonical owner
scans at most 200 store rows per call. Other record types can therefore produce
an empty page with a next cursor. `scanned_records` includes a fetched row that
must be deferred to the next page when this page's byte budget is full.

The setup cursor is a random, session-local handle, not a permission, query or
snapshot. It references a canonical cursor privately in the Controller; raw
store keys never appear in the setup cursor. Passing it to another session or
after restart returns `recipe_cursor_unknown` before any store call. Start a new
traversal in that session. Repeated canonical positions reuse the same handle;
at 1024 distinct positions the session returns `recipe_cursor_limit` without
evicting earlier handles. A new session can begin another traversal. The
standalone operator CLI has a different cursor: it encodes the last store key
and can survive restart. Keep that raw cursor in the trusted host boundary. Concurrent inserts
before the cursor require a new traversal to become visible; later inserts may
appear as traversal continues. Do not interpret a completed traversal as a
transactional snapshot. Structurally valid store records with invalid recipe
content are reported as unverified. Store-level decoding or IO failures remain
errors and cannot become empty or complete pages.

Empty `setup_recipes {}` arguments preserve the original whole-list operation
for earlier pinned executables. Pagination requires the matching new CLI; an
unsupported page request fails without silently falling back to an unbounded
list. The bridge does not collect pages for the agent or rank candidates.

An absent identity returns `not_found`; a mismatched digest or unavailable store
is an error. A malformed, oversized or failed subprocess response is never
presented as an empty catalog. The bridge accepts at most 2 MiB of raw JSON and
2 MiB of canonical result JSON, lower than the standalone CLI's 8 MiB output
limit. The legacy whole-list operation can exceed that limit for a large valid store;
use pages for traversal. A single exact-read response is still bounded and may
require the CLI if its documents exceed 2 MiB. No response is truncated.

These tools only observe recipe metadata, but SQLite may create WAL sidecars,
change journal mode and coordinate transactions. They do not promise zero
filesystem writes. The disclosure-only profile does not expose `save`, `export` or arbitrary command
execution. Session shutdown uses the existing setup controller and joins entered
calls. An export recovery session requires its exact separately pinned recipe profile
and exposes only current-file readback, as described in
[hosted export recovery](HOSTED-RECIPES.md#inspect-an-export-after-restarting-the-host).
Recipe listing and retention are not exposed in recovery sessions.

The optional browser companion uses the same controller and host designation.
There is no recipe browser UI in this change. Native `find_and_install`, protected
provider authentication, attachment and original-task continuation remain
separate unfinished work.

## Explicitly retain a selected investigation

To let this same local session retain nonsecret historical documents, the host
can instead set the profile's exact `scope` to
`retain_and_disclose_nonsecret_local_recipe_metadata`. All other profile keys,
paths, executable pins and principal selection are unchanged. The original
`disclose_nonsecret_local_recipe_metadata` scope never enables retention.
This opt-in designates the documents and supplied references as suitable for
the store's future readers; it does not sanitize them or create new authority.

The retention scope adds `setup_remember` and marks
`setup_state.local_recipes.retention_available` as true. After a selected Python,
Node or MCPB install job settles in this live controller, explicitly call:

```json
{
  "job_id": "EXACT_SELECTED_INSTALL_JOB_ID",
  "decision": {"owner": "existing-decisions", "record": "actual-choice", "revision": "1"},
  "verification": [
    {"owner": "existing-observations", "record": "actual-partial-check", "revision": "1"}
  ]
}
```

References must identify real historical records. They are caller-supplied
claims, not proof produced or verified by this bridge. Do not invent a reference
because a job succeeded. The array contains 1-32 distinct references; owner and
record follow the existing identifier syntax (at most 192 ASCII characters),
and revision is nonempty, control-free text of at most 256 UTF-8 bytes. The
existing CLI remains the final document and reference validator.

Only settled selected-package jobs are accepted. An `unconfirmed` job may be
retained because investigation history includes partial or failed outcomes.
Retention never changes that job's status. Library jobs, foreign job IDs,
running jobs and recovered requests are refused. Selected-install recovery cannot retain recipes. Export recovery uses its
separately pinned recipe profile only for current-file inspection, not retention.

The controller revalidates the original retained request and complete frozen
proposal, then invokes its pinned `toolkit-local-recipes save` with the original
package and selected document, profile and component. Plain Python/Node software
uses `--declaration` with fixed `declaration.json`; the full activation path keeps
`--selection` with `activation.json`. MCPB also supplies its original configuration
bytes and does not accept the plain form. When the selected proposal includes an ordinary
plugin source observation, it also supplies the exact retained `plugin-source.json`
bytes. The recipe owner validates this observation and binds it into the saved
recipe digest. `setup_recipe` returns it as `documents.plugin_source_json`, with
`plugin_source_digest` in the summary. The source can therefore outlive both the
original staging directory and setup workspace. This is historical, untrusted
guidance, not a new inspection or approval. Old recipes without source evidence
remain readable; adding or changing source under an existing immutable identity
is a conflict, not an update.

Plain software is retained as version 4 in the same immutable package-identity
namespace. Its summary has `kind: "local_software_selection"`,
`document_kind: "local_software_declaration"` and `declaration_digest`, with no
`manifest_digest`; `setup_recipe` returns the exact text as
`documents.declaration_json` alongside `documents.package_json`. The retention
`digest` binds those exact texts and historical evidence, separately from the
canonical declaration digest. Versions 1-3 and their existing fields remain
unchanged. Older executables reject version 4; use coherent current binaries and
setup sources. A retained software declaration still requires current artifact,
host and access checks before reuse.

The controller never reads replacement staging files, creates
a new selection, saves artifacts, launches the server or contacts a provider.
The verification-reference array is frozen in a private content-addressed file;
an existing file must match exactly. The store's existing immutable identity
rule owns persistence and replay. No second recipe database or decision registry
is introduced.

A confirmed result preserves the CLI's `retained_recipe_metadata` envelope and
the exact bound recipe summary. Read it later using `setup_recipes` and
`setup_recipe` in the same host-selected scope. `retention_conflict` means the
existing immutable record was preserved. `retention_unconfirmed` means the
save may have happened; use list/read or replay the exact request, whose
immutability behavior is owned by the store. The bridge never retries it
automatically. `retention_source_unavailable` occurs before save entry when
the frozen source or host inputs cannot be confirmed.

Save shares the existing setup operation lock and dispatch lifetime. Cancelling
the caller does not detach the save, and graceful shutdown joins it. The bounded
CLI process may time out after writing; killing it is not proof that no record
was retained. This adds a local historical record, not catalog promotion,
installation success, current permission, account readiness or native completion.
