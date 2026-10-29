# Service component notices

This component contains the selected original Meerkat Toolkit Slack, Gmail, Google Calendar, Drive and
HTTPX client modules plus their configured MCP adapter and a new standard-library plain-text MIME
composer authored for this component. It also includes original Toolkit host-only
Google and Slack authorization-code protocol helpers. The original module
bodies and relative imports are copied unchanged into a distinct
`meerkat_toolkit_services` namespace. See the packaged `PACKAGE-SOURCES.json` for
the source mapping and SHA-256 hashes. Toolkit code is available under MIT or
Apache-2.0; both license texts are included exactly.

The readable Gmail formatter in `connectors/gmail_content.py` is new original
Toolkit code under the same MIT OR Apache-2.0 terms. It implements the public
Gmail Message/MessagePartBody JSON contracts using Python's standard-library
email/base64 APIs. No private `gmail_message.py` implementation or other donor
code was copied. Contract references and scope are in the packaged GUIDE.md.

The Calendar Events transport is an original Toolkit implementation informed
by Google's REST contracts and existing Toolkit HTTP/cancellation patterns.
Its source is copied unchanged. It contains no Google client-library or
host implementation body. Its original provenance is recorded in
`sdk/python/GOOGLE-CALENDAR.md` and
`docs/coordination/20260930-upstream-review/calendar-transport-adoption` in the
Toolkit repository. The standalone MCP profile exposes its list/get reads and
explicitly selected insert/patch/delete writes through that same transport.

The Drive transport is likewise original Toolkit code using public Drive REST
contracts and the existing borrowed-client request owner. Its provenance is in
`sdk/python/GOOGLE-DRIVE.md`. The configured profile selects list/get metadata
and bounded content reads, not its write or disk-publication methods. No Google
CLI implementation bytes or private host code are included.

This wheel does not contain any host's private Gmail MIME builder, other private
modules, Agno source or Activepieces source. It does not vendor its runtime
dependencies. MCP, HTTPX, AnyIO, jsonschema, referencing and their transitive
dependencies are separate distributions whose own installed metadata and license
notices remain applicable. The optional `google-oauth` extra installs `google-auth`
and its dependencies as separate distributions; their code is not vendored here.
The dependency pins in `pyproject.toml` are direct
dependencies, not a complete environment lock.
