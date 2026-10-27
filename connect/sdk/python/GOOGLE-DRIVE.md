# Google Drive operations

Install `meerkat-toolkit[google-drive]` and import
`GoogleDriveOperations` from `meerkat_toolkit.connectors.google_drive`.
This is an explicit service transport. The host selects the account, credentials,
permissions, approval, file paths and result audience. It is not an OAuth client,
agent router, cursor store or retry engine.

```python
from meerkat_toolkit.httpx_client import prepare_httpx_client

# At host composition, before sharing the client with operations.
prepare_httpx_client(authenticated_client)
ops = GoogleDriveOperations(authenticated_client, base_url="https://www.googleapis.com/")
page = await ops.list_files_page(query="trashed = false", fields="nextPageToken,files(id,name)")
detail = await ops.get_file("selected-file-id", fields="name,mimeType")
# Only an explicitly selected next page is requested.
next_page = await ops.list_files_page(
    query="trashed = false", fields="nextPageToken,files(id,name)",
    page_token=page["nextPageToken"],
)
```

The constructor borrows an `httpx.AsyncClient` and never closes it. The host must
[prepare that client](HTTPX-CLIENT.md) before use and keep response-hook ordering
and stream ownership stable while it is borrowed. Preparation installs one first
non-suspending response hook, preserving the order of existing hooks, auth and
client lifetime. It covers HTTPX response cleanup that happens before `send`
returns, including application hooks and authentication flows. This is an
explicit initial interface correction; older direct-client examples require the
preparation step. Operations validate the client without mutating its hooks.
Drive maps missing or reordered preparation to `DriveError` with kind `CLIENT`
and `NOT_ATTEMPTED` for a write, before token lookup, file work or provider entry.
Reads use `NOT_APPLICABLE` instead. Its optional
`auth` applies only to these requests; omission keeps the borrowed client's auth.
`base_url` is one HTTP(S) API root without credentials, query or fragment. Both
`drive/v3/` and `upload/drive/v3/` resolve below it. Production normally uses the
root above; loopback roots support host tests. Host-supplied auth, hooks and
transports remain responsible for avoiding redirects, retries or retargeting of
their own. The operations disable HTTPX redirects and clear inherited query
defaults, conditional headers, ranges and method overrides. They do not mutate
the client's defaults or own its credential storage.

## Operation surface

| Method | Inputs beyond the selected client | Result |
| --- | --- | --- |
| `list_files_page` | Optional `query`, `page_size`, `fields`, `order_by`, `page_token` | One provider object |
| `get_file` | `file_id`; optional `fields` | Provider object |
| `download_content` | `file_id` | Complete `DriveContent(data: bytes, mime_type: str)` |
| `export_content` | `file_id`, `mime_type` | Complete bounded `DriveContent` |
| `download_file` | `file_id`, `output_path` | `output_path` and byte count |
| `export_file` | `file_id`, `mime_type`, `output_path` | `output_path` and byte count |
| `create_file` | Metadata object; optional `upload_path`, `upload_content_type`, `fields` | Provider object |
| `update_file` | `file_id`, metadata object; optional upload inputs, `add_parents`, `remove_parents`, `fields` | Provider object |
| `create_permission` | `file_id`, permission object | Provider permission object |
| `delete_file` | `file_id` | `None` after an accepted empty deletion response |

These are the concrete operations currently used by the paired host. They do not expose
every Drive REST parameter or implement automatic pagination. Field masks are
sent exactly; omitting `fields` differs from passing an empty string. Selected
masks need not return `id`. If get/update does return `id`, it must match the
requested file. A permission's `id` identifies the permission, not the file.
Unknown JSON fields, nulls and false values survive. Query text is UTF-8 data,
including whitespace; the transport does not interpret or rewrite it. File IDs
are encoded as individual path segments. Opaque page tokens are retained exactly
within the nonempty, control-free text profile.

## Bounded in-memory content

`download_content` reads one blob with `alt=media`; `export_content` reads one
explicit Workspace export format. Both use the same request, deadline and
response cleanup owner as existing operations. `max_content_bytes` is a separate
positive constructor bound (default 2 MiB) on complete raw bytes in memory.
Excess size, malformed length, content encoding or cleanup failure returns no
content. The successful immutable `DriveContent` contains exact bytes and the
validated response media type (or `application/octet-stream` if absent). There
is no local file allocation, conversion, truncation or fallback request.

The configured services MCP profile projects this value to an embedded text or
base64 resource and has its own maximum configuration bound. The direct SDK
continues to expose the existing disk methods below; their media size semantics
are unchanged. Neither content route guarantees a particular revision or model
support for its returned media.

## Media and local file custody

Downloads and exports copy raw response bytes, including an `application/json`
body. They stage in the destination directory, validate the complete body and
declared length, join response closure, flush/fsync and close the file, then
atomically replace the destination. Pre-publication failure preserves the prior
destination and removes staging. No partial file is published. Cancellation that
races an already-entered atomic replace can leave the complete new file; a
cancelled caller must inspect its selected destination before deciding what to
do next. This is local publication, not a crash-durable application receipt.

Relative upload and output paths are made absolute before the first suspension.
The returned `output_path` preserves the caller's original spelling. This fixes
the selected directory across a later process-CWD change; it does not prevent
other owners from replacing files or symlinks. The host authorizes paths and
controls concurrent content changes. Existing the paired host upload approval binds a
path, not immutable file bytes.

Uploads use a retained regular-file handle and streamed multipart/related body.
Local I/O uses joined worker calls and reads at most 64 KiB per chunk. Metadata is
captured before suspension. MIME precedence is explicit `upload_content_type`,
filename inference, string metadata `mimeType`, then `application/octet-stream`.
The captured file length bounds the stream; detected length changes fail. The
transport does not promise a content snapshot if another process overwrites the
same-length file during reading.

The default 2 MiB request/response limits apply to JSON metadata, not streamed
media. No new 5 MB media cap is introduced. Multipart preserves the existing
The paired host CLI strategy. Google's [upload guide](https://developers.google.com/workspace/drive/api/guides/manage-uploads)
recommends resumable uploads for larger or interruption-prone transfers. This
adapter does not implement resumable sessions and synthetic large-stream tests
do not prove acceptance of arbitrary live upload sizes. Google also imposes
method-specific constraints such as the [export limit](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/export).

## Failures, cancellation and ownership

Each call enters at most one HTTP request. A redirect, auth error, provider error,
lost reply, malformed body or cleanup failure does not trigger another request.
`DriveError` carries safe `kind`, `write_outcome`, optional `status_code` and
bounded numeric `retry_after_seconds`; it omits provider prose, URLs and paths.
Duplicate JSON keys, nonfinite numbers, wrong object shapes, unsupported content
encoding and malformed/truncated `Content-Length` refuse a result.

`WriteOutcome` is `NOT_APPLICABLE` for reads, `NOT_ATTEMPTED` for a typed local
failure before write entry, or `UNKNOWN` once a failed write has entered the HTTP
owner. A successful return is the accepted response, not proof of later business
effects. Input validation can raise `ValueError` before entry. Entered-write
cancellation raises `DriveWriteCancelled`, an `asyncio.CancelledError` subclass
whose `write_outcome` is `UNKNOWN`. Reads preserve cancellation as cancellation.
Caller cancellation and the default 30-second cooperative timeout join retained
request, response-close and worker cleanup before returning. Slow cleanup can
outlast that timeout. HTTPX's automatic EOF close and pre-return response-hook cleanup are retained too. No rollback or
automatic reattempt follows uncertainty.

The paired host's owned client factories prepare their clients immediately; injected
clients must be explicitly prepared by their composing host. Its adapters refuse
an unprepared borrowed client before broker lookup. The paired host's Drive adapter
validates its supported command shapes before credential
access, supplies its existing broker's selected-account token and quota project,
and routes Drive before the old generic subprocess retry. A typed 401 invalidates
only that account's cached token for a later explicit call. The paired host still owns
query modes, manual continuation maps, approvals, display projection, conversion,
collection, source ingestion and policy. Local account labels are not new native
verified-account or binding-generation attestations.

## Provenance and proof limits

This original implementation uses the existing Toolkit borrowed-client pattern
and public Drive REST documentation: [list](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list),
[get](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get),
[create](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/create),
[update](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/update),
[permission create](https://developers.google.com/workspace/drive/api/reference/rest/v3/permissions/create)
and [delete](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/delete).
No Google CLI implementation bytes were ported. The inspected gws executor was a
behavioral reference for existing multipart/MIME behavior and exposed the JSON
media-output defect; its license is not a grant for the paired host's private code.
The SDK's existing mixed-source package licensing remains unchanged.

Source tests use synthetic HTTP and local files. They do not establish live
Google account authorization, provider acceptance, native admission, installed
artifact correctness, deployment or completion of the full Toolkit specification.
