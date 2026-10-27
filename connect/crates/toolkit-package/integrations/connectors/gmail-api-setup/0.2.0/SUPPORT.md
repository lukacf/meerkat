# Gmail 0.2.0 support record

This is an additive version of `meerkat-toolkit/gmail-api-setup`, preserving the
`gmail-api-setup.service` service-connector export. The retained `0.1.0` bytes and
its guidance-only behavior remain unchanged. Agents select the exact version,
library digest and strategy. No resolver ranks these choices and no failure
causes automatic fallback.

| Explicit strategy | Available material | Boundary |
| --- | --- | --- |
| `hosted-mcp-preview` | Portable Streamable HTTP endpoint plus typed setup prerequisites | Experimental remote option; no live account, schema or role readiness receipt |
| `toolkit-rest` | Independently usable Python and TypeScript operation libraries in this repository | Guidance-only activation plan until host auth, adapter and service-export binding exist |

## Hosted MCP preview

The retained endpoint is `https://gmailmcp.googleapis.com/mcp/v1`, with Google as
publisher. The observed [Google setup guide](https://developers.google.com/workspace/gmail/api/guides/configure-mcp-server)
describes a Developer Preview covering search/threads, drafts and labels. It does
not document a send tool. Creating a draft is not sending mail. This route is not
read-only because draft and label operations can change data. Actual schemas and
effect classification must come from the selected connection owner.

Follow the concrete account, project, API enablement, client/redirect and scope
prerequisites in `recipes.json`. A quickstart/example client is not the Toolkit
host's registered client. [Preview program terms](https://developers.google.com/workspace/preview)
and [Workspace MCP security requirements](https://developers.google.com/workspace/guides/configure-mcp-security)
remain provider prerequisites, including preview distribution restrictions and
the host's selected screening solution. No secrets belong in `mcp.json`, package
metadata or model context. The endpoint has no inline auth or query credentials.

Successful offline manifest validation is still only a declaration. Activation
must retain owner-authenticated evidence for the exact account, credential
binding, current schemas, operation scope, policy, role and lifetime. Account or
administrator refusal is reported as refusal; REST is not a workaround.

## Toolkit REST operations and local availability

The existing APIs are documented in [Python GMAIL.md](../../../../../../sdk/python/GMAIL.md)
and [TypeScript GMAIL.md](../../../../../../sdk/typescript/GMAIL.md):

- Python: `meerkat_toolkit.connectors.gmail.GmailOperations`, with an already
  authenticated host-owned HTTPX client and explicit `user_id`.
- TypeScript: `GmailOperations` from `@rkat/toolkit/connectors/gmail`, with the
  host-selected `baseUrl`, `userId` and authenticated Fetch function.

Both implement one-page listing, selected message retrieval and one-attempt MIME
send. They preserve opaque continuation and nested message data. Their returned
send receipt is not delivery proof; an unknown outcome is not permission to
retry. The host authorizes the selected mailbox, complete message and recipients.
See the official [list](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list),
[get](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get)
and [send](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send)
contracts for the underlying REST coverage. Scope selection and any required
production verification remain host setup work under the
[Google OAuth quickstart guidance](https://developers.google.com/workspace/gmail/api/quickstart/python).

Availability here means repository source and locally built artifacts. This
record does not establish a PyPI or npm release. Do not turn the import names or
package version `0.1.0` into an instruction to download an unverified public
package. Select and hash the local artifact or build the selected repository
source with the matching SDK instructions and dependencies.

The recorded local artifacts, relative to the repository root, are:

| Artifact | SHA-256 |
| --- | --- |
| `docs/coordination/20260930-upstream-review/gmail-operations-verification/license-packaging-correction/wheels/meerkat_toolkit-0.1.0-py3-none-any.whl` | `40674a717336a47a7ab9bd283e0d72a752b13f116b9319018e6ec4d1490878b7` |
| `docs/coordination/20260930-upstream-review/gmail-typescript-operations-verification/packages/rkat-toolkit-0.1.0.tgz` | `ffb79a13d2ff7e77f4442dfacf48c3448639b9c5437ab49b54019020cbdc933b` |

The Python artifact's 20 Gmail loopback cases and the TypeScript artifact's 21
Gmail cases (36 including HA) passed against the installed artifacts. They prove
that bounded local protocol profile, not Google authentication, provider
conformance, package activation, native OAuth/C1, human approval or host migration.
Read the SDK documents for exact runtime and cleanup limits. The TypeScript
record used Node 25.9.0; Node 22 was not separately executed.

The libraries do not register tools, choose accounts, acquire credentials or own
agent authority. A host adapter still needs to bind their actual operations,
schemas, limits and lifetime to the selected service export. The credential,
authorization, approval and reconciliation owners remain separate. The REST
recipe therefore deliberately remains `guidance_only` in the current library
selection API. No additional activation registry is introduced.

## Source and license scope

`recipes.json` records seven official documentation observations made on
2026-10-01. Exact response bytes, timestamps and hashes are retained under
`docs/coordination/20260930-upstream-review/gmail-library-successor/` in this
repository. These hashes identify the inspected documentation, not Google's
mutable hosted implementation. The local artifact hashes above identify actual
builds, not production deployment or credential state.

Toolkit metadata and SDK code are original MIT OR Apache-2.0 work. Google's
pages state CC BY 4.0 for prose and Apache-2.0 for samples, subject to their page
exceptions; service and preview terms are separate. No provider implementation
or quickstart code was ported. This version remains experimental. The complete
M4/M8 breadth, lifecycle/auth integration and required host migrations remain open.
