# Forge GitHub Enterprise collection programs

This standalone Rust library owns bounded repository result parsing and call
sequencing for Forge. It has no network, process, credential or Core/Meta/Connect
dependency. It is not yet attached to an installed native host.

`PullRequestStatuses` reads one explicit enterprise PR and then its combined
commit statuses. It checks the exact repository URL, PR number, base repository,
returned commit SHA, complete status count, unique contexts and matching call
arguments. Errors end the cursor without retry. A failed CI status is valid
historical data. It is not a failed MCP call or an authorization decision.
`at_head` preserves a revision already selected in the native requirement and
refuses if the PR has moved before the first result is read.

The host chooses the repository and PR from its retained authorized requirement.
It must check the actual installed endpoint/account and construct the native
`CollectionProgram` adapter. That adapter must pass only a matching
`CommittedCollectionResult`, after native settlement and joined cleanup, and
handle tool errors before invoking this parser. Passing JSON to this library
does not establish any of those conditions. Source IDs, original login lifetime,
admission reservations and native evidence remain with their actual owners.

The pure program returns the commit represented by the result. It does not claim
that the PR still has that head or that credentials remain usable afterward.
The eventual adapter must retain the native observation alongside these data.

`PullRequestReviewThreads` reads the exact PR and then follows the provider's
opaque `after` cursor. It preserves outdated and resolved threads and nullable
line coordinates. It refuses repeated cursors, duplicate thread/comment
identities, changed totals, visible nested truncation and incomplete final pages.
Explicit call and cumulative-result-byte bounds include the initial PR read.
Results are provider-reported page sets, not an atomic revision snapshot or
proof of an unfiltered conversation.

## Pinned upstream behavior

Contract: GitHub MCP server v2.0.3, source
`b98d115bbb080e9555466a96656ec4df605aea21`:

- [PR read implementation](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/pullrequests.go)
- [Result types](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/minimal_types.go)

`get_status` fetches the PR head and requests combined statuses without pagination
options. When `total_count` differs from the returned row count, this program
returns `IncompleteStatusPage`. Inventing a page argument would not close the gap.

`get_check_runs` returns IDs and results but omits the commit SHA. That output does
not establish which head each page observed. Reading the PR before and after the
call does not rule out an intervening head change. A supported result with exact
revision correlation is still needed for complete check-run coverage.

`get_review_comments` fetches at most 100 comments per thread and supplies no
nested continuation. A larger reported thread count refuses. In lockdown mode,
the server filters comments and replaces each thread's count with the filtered
length. The native adapter must therefore verify the actual installed invocation
has unfiltered behavior before qualifying full discussion coverage. Count checks
alone cannot prove it, and Python/browser labels cannot establish that setting.
Review-summary pagination has a related limitation: `get_reviews` can filter
rows after REST pagination, so a short array alone is not proof of the final page.

Remaining work includes the native bridge/adapter and the other required GHE
collection programs: ordinary PR comments and review summaries, files and
commits, bounded repository history, workflow runs/jobs, and issues. Repository
metadata and historical non-PR commit checks still need a supported source.
These programs do not close those requirements or the overall shared ingestion
outcome.

## Verification

From the repository root, use the existing isolated wrapper and this manifest:

```sh
./scripts/repo-cargo test --locked --manifest-path installer/native/ghe-collections/Cargo.toml
./scripts/repo-cargo clippy --locked --manifest-path installer/native/ghe-collections/Cargo.toml --all-targets -- -D warnings
./scripts/repo-cargo fmt --manifest-path installer/native/ghe-collections/Cargo.toml -- --check
```

The separate workspace and lockfile keep these pure checks out of the upstream
native build graph. Controls use synthetic provider JSON, not actual provider
credentials, MCP execution, a native receipt or a live installed host.
