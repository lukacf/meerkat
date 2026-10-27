# Google Drive 0.1.0 support record

This is the first retained `meerkat-toolkit/google-drive-api-setup` library entry.
It describes one `google-drive-api-setup.service` service-connector export and
one explicit `toolkit-rest` strategy. All existing package versions stay intact.
The agent chooses the exact package version, package digest, library digest and
strategy. No heuristic selector, automatic upgrade or fallback is supplied.

| Explicit strategy | Available material | Boundary |
| --- | --- | --- |
| `toolkit-rest` | Python `GoogleDriveOperations` and a paired host adapter with bounded local source evidence | Guidance-only catalog activation; existing host owners supply authenticated account, policy, schemas and service binding |

This package declares no remote MCP endpoint or agent frontend. That says nothing
about other products an agent might separately investigate. `guidance_only`
means the current library selection API cannot activate this recipe; it does not
mean the operation library or the paired host adapter is missing.

## Select the transport and its owners

Use the [Python Drive guide](../../../../../../sdk/python/GOOGLE-DRIVE.md) and
[shared HTTPX client contract](../../../../../../sdk/python/HTTPX-CLIENT.md).
The import is `meerkat_toolkit.connectors.google_drive.GoogleDriveOperations`.
Select the `google-drive` optional profile from an authorized exact source or
verified local artifact: Python 3.11+, HTTPX `>=0.28.1,<0.29` and AnyIO
`>=4.13,<5`. A package name and version do not establish a public PyPI release.
This record represents no TypeScript Drive implementation.

At host composition, call `prepare_httpx_client` on the selected authenticated
HTTPX client before sharing it. Preparation adds one first, non-suspending
response hook, preserving existing hook order, auth and client lifetime. The
host must keep hook ordering and stream ownership stable while the client is
borrowed. Operations validate preparation without per-call client mutation.
This explicit contract covers response cleanup inside HTTPX, including cleanup
before `send` returns. The operation library never closes the borrowed client.
Host auth, hooks and transport must not independently replay or retarget calls.

The host selects the API root, normally `https://www.googleapis.com/`; metadata
and upload paths resolve below that root. The library disables HTTPX redirects
and clears inherited query defaults, conditional/range and method-override
headers. Those mechanics do not authenticate an account or authorize a resource.
The existing credential owner retains OAuth registration, secrets, consent,
refresh/revoke and operation-scope selection. No credential data belongs in this
package or in model context. A local account label, quota project or file ID is
not a verified account or resource binding.

Follow the typed prerequisites in `recipes.json`: artifact selection, project
and client registration, actual account selection, client authentication,
organization/resource permissions and application binding. The official method
scope lists are alternatives for the owner to assess, not permission to request
all scopes. Reading, sharing, uploading and deleting have different effects;
actual grants, resource ACLs, approvals, paths and result audiences remain host
responsibilities.

## Exact operation coverage

| Method | Bounded coverage |
| --- | --- |
| `list_files_page` | One page with exact query, field mask, order and opaque page token |
| `get_file` | Selected file metadata with caller-selected fields |
| `download_file` | Raw media bytes to the selected local output path |
| `export_file` | Selected export MIME type and byte-exact local output |
| `create_file` | Metadata create or streamed multipart upload |
| `update_file` | Metadata update or streamed multipart upload, with selected parent changes |
| `create_permission` | One selected file permission mutation |
| `delete_file` | One selected deletion, returning after accepted empty success |

See the captured official [list](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list),
[get](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get),
[export](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/export),
[create](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/create),
[update](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/update),
[permission-create](https://developers.google.com/workspace/drive/api/reference/rest/v3/permissions/create)
and [delete](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/delete)
contracts. This is not every Drive endpoint or parameter. Unsupported host
command shapes refuse before credential/provider entry, with no CLI fallback.

The caller owns pagination. Query text is data, including whitespace; the
transport does not parse intent or rewrite it. Field masks remain exact and may
omit `id`. A returned get/update `id`, when present, must match the requested
file. A permission ID is a different object. Unknown JSON values, nulls and
false values survive. The page-token profile is nonempty, control-free text.

Downloads and exports preserve all response bytes, including `application/json`
media. Output is staged beside the destination, with complete-body validation,
joined response closure and file flush/fsync/close before atomic replacement.
Pre-publication failure preserves prior output and removes staging. No partial
file is published. Cancellation racing an already-entered atomic replace can
leave a complete new file; the caller must inspect that selected destination.
This local publication is not a crash-durable application receipt.

Uploads stream a retained regular-file handle through joined worker I/O.
Metadata limits do not cap streamed media, and no new 5 MB upload limit is
introduced. The implementation preserves the existing multipart strategy;
Google's [upload guide](https://developers.google.com/workspace/drive/api/guides/manage-uploads)
recommends resumable uploads for large or interruption-prone transfers. This
transport does not implement resumable sessions, and local large-stream tests
do not prove arbitrary live upload-size acceptance. Provider-specific export
and operation limits still apply. Paths are fixed before suspension, but existing
The paired host upload approval binds a path, not an immutable snapshot of file bytes.

## Authorization handoff and safe resumption

The supported auth path is host-owned OAuth bearer authentication through the
selected HTTPX client or the constructor's per-request `httpx.Auth`. The existing
credential owner issues and resolves an opaque, non-secret connection/credential
reference for the selected account, client/project and consent/scopes. Setup
context retains only that reference and safe status. Access and refresh tokens,
client secrets, authorization codes and sensitive callback material stay with
the credential owner and outside model context.

If authorization is not available, request the existing owner's supported secure
human-facing handoff. Resume only when that same owner reports authorization for
the selected connection and confirms its current account/scope binding. A local
account label, a quota project or file metadata does not establish that binding.
Do not invent a reference format, authorization URL, ceremony or resume API.
Missing owner support, cancellation or denial leaves setup incomplete; it does
not select another account, expand scopes or activate an auth fallback. This
recipe describes the handoff requirement, not a runnable generic wizard flow.

## A bounded read observation before activation

After existing host policy authorizes it, call
`get_file(host_selected_file_id, fields="id,mimeType")` using the same selected
credential context and a host-selected known readable resource. Require an
object with the exact selected `id` and a nonempty string `mimeType`. A missing
or mismatched identity is not a passed check. This particular probe requests
identity explicitly; it does not change the general SDK contract that other
caller-selected field masks may omit `id`. Confirm account binding separately
through the existing credential owner, never through the file response.

If listing itself is the requested and explicitly authorized capability, an
alternative is one narrow
`list_files_page(query=host_authorized_query, page_size=1, fields="files(id),nextPageToken")`.
Validate the returned object and `files` list, including each returned ID. An
empty list observes only acceptance of that query. It does not establish access
to arbitrary files. Do not automatically follow a returned page token.

Record safe selected connection/resource/operation context and observation time
with the existing host owner. Missing client preparation or credentials leaves
setup incomplete. Authentication refusal returns to the credential owner;
403/404 needs owner diagnosis without inferring resource absence or permission
to widen scope. Timeout, cancellation, transport or malformed-result failures
leave the check inconclusive. None of these outcomes triggers an automatic retry,
account substitution or stronger credential request.

Success observes only that metadata read under the selected credential/resource
context at that time. It proves no account identity, all-file access, write
permission, upload/export readiness, generic activation or complete adoption.
Catalog load, token resolution and client preparation are not readiness. Do not
use create, update, share, delete, download or export as this read-only check;
download/export also publish local files. The guidance introduces no health
runner or new receipt store. Essential handoff/probe instructions also appear in
the serialized strategy and prerequisites because discovery does not return
this support document.

## Outcomes and the paired host composition

There is no automatic retry after auth failure, redirect, malformed response,
lost reply or cancellation. Typed local write refusal before entry is
`NOT_ATTEMPTED`; a failed entered write is `UNKNOWN`. Reads use
`NOT_APPLICABLE`. Entered-write cancellation preserves uncertainty through
`DriveWriteCancelled`. Cleanup joins retained request, response and file work;
a cooperative deadline can be exceeded by cleanup. Success is the accepted
operation response, not proof of downstream business completion. The host owns
reconciliation, and uncertainty never authorizes a replacement write or fallback.

The actual paired host adapter routes supported Drive commands through Toolkit
before the generic subprocess/auth-retry path. Its owned HTTPX clients prepare
at construction; borrowed clients must be prepared by their composing host
before broker lookup. The existing GoogleTokenBroker supplies the selected
credential and quota project. A typed 401 invalidates only that selected cached
credential for a later explicit call, without replaying the failed operation.

The paired host retains its account resolution, grants, approvals, query modes, manual
continuation maps, projection limits, source collection and ingestion. Catalog
metadata does not change those owners or claim native account attestation,
binding-generation continuity or complete host migration.

## Evidence and current acceptance boundary

The retained [Drive source checkpoint 2](../../../../../../docs/coordination/20260930-upstream-review/drive-rest-transport-successor/source-checkpoint-2/manifest.json)
has SHA256
`624703b05b1604813eb1e1701375c8f42660b018eaabff0982ed9b3f7156b1c4`.
Its Drive module has SHA256
`d9d567bf2fad5f0807623518eaa714173cec323ec30cb3c4359171a20a657102`.
The [author source report](../../../../../../docs/coordination/20260930-upstream-review/drive-rest-transport-successor/REPORT-checkpoint-2.md)
and [independent source closure](../../../../../../docs/coordination/20260930-upstream-review/drive-rest-transport-independent-review/REPORT-checkpoint-2.md)
record 223 author cases and 59 independent cases respectively. Their scopes
include preserved predecessor failures, local protocol/file cleanup and actual
The paired host composition with synthetic or loopback providers. Overlapping cases
are not additive evidence.

The [combined private-artifact acceptance](../../../../../../docs/coordination/20260930-upstream-review/drive-rest-transport-independent-review/installed-proof/root-acceptance.json)
binds the final combined HTTPX/Drive and the paired host composition, with receipt SHA256
`6b43fcf88de0b71f202219954ef56fea3a2f89015984ad32f45c89bc7bf64f03`.

| Accepted private artifact | SHA256 |
| --- | --- |
| Toolkit wheel | `155531563a87bec0f8ed6ac89af08824d1276efb52cc7bf480698370fd8c8a32` |
| The paired host wheel | `e68e60495eef87ead970ceccd0d5b9818dcbd6e9fe348abe9a91e5c812befef7` |

That receipt records 630 distinct installed cases plus 170 subtests, both wheel
RECORD/payload checks and actual installed origins. The selected graph contains
142 distributions; the other 140 distributions and 12,215 unrelated files were
unchanged. Native SDK 0.8.45 and MobKit 0.8.43 stayed pinned. This is bounded local
artifact evidence for these exact private bytes, not evidence for a different
artifact or a public PyPI release. Source and installed case counts overlap and
must not be added together.

This record remains experimental. No live Google account authorization, native
runtime adoption, deployment or complete Toolkit requirement is certified by
that acceptance or this recipe.

## Source and license scope

The eight official documentation bodies were captured on 2026-10-01 at
19:54:28.966382 UTC. [The observation manifest](../../../../../../docs/coordination/20260930-upstream-review/drive-rest-transport-successor/public-docs/manifest.json)
records URLs, HTTP status, exact body paths and SHA256 values. `recipes.json`
retains these observations and package assessments reference every source.
Documentation hashes identify inspected bytes, not Google's implementation,
current credentials or service conformance.

Catalog metadata and the Drive transport are original Toolkit work under MIT OR
Apache-2.0. Google's pages state CC BY 4.0 for prose and Apache-2.0 for samples,
subject to page exceptions; service terms are separate. No Google CLI source or
sample code was ported. The inspected gws executor informed the existing
multipart/MIME contract and revealed the JSON media-output defect. That
inspection is not a license grant for the paired host code. The complete private SDK
artifact retains its existing mixed-source declaration and license files; this
catalog entry does not relicense other modules.
