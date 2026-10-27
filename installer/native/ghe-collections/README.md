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

`PullRequestChanges` reads the exact PR, its file and commit pages, and a final
PR result. It requires explicit totals, exact page sizes against the remaining
counts, unique file/commit identities and the observed head in the commit set.
It refuses a changed final PR head or count. `at_head` also preserves a revision
already selected in the native requirement. The call budget reserves the final
read, and the byte budget includes every result. Filenames, patches, author
fields and messages remain data; the program never opens paths or follows them.

`PullRequestDiscussion` reads ordinary PR comments or review summaries after an
exact PR read. It pages until a short response, requiring another empty page for
an exact multiple of 100 rows. Explicit comment totals detect visible gaps;
missing totals remain unknown. Comment/review IDs must match their resource URLs
and remain unique across pages. Review states and old commit IDs remain
historical data, not the current effective approval decision. A returned
`ReportedDiscussionPages` records a short-page observation, not proof of complete
coverage. The native adapter must verify actual unfiltered installed behavior
before it can use that observation to qualify the source.

`RepositoryHistory` resolves either an explicitly selected commit through
`get_commit` or the first commit in a one-row default-branch `list_commits`
response. Subsequent list calls retain that exact SHA and an explicit UTC window
of at most seven days. It does not reread a moving branch or use local time.
It pages until a short response, rejecting duplicate or foreign commit results
and any changed call, filter, revision or time bound. Dates and messages remain
provider data; no timestamp ordering or early date cutoff is assumed. The anchor
is retained separately, including when no commits fall within the queried window.

`WorkflowRuns` enumerates repository workflow runs with stable explicit counts,
unique run IDs and exact repository/run URLs. `WorkflowRunJobs` retains a selected
run ID, head SHA and attempt, reads that run before and after job paging, and
requests jobs from all attempts. Each job must belong to that run and head, with
an explicit attempt no later than the selected attempt. Failed jobs and steps,
earlier attempts, and changing run status remain historical data. Both programs
bound calls and cumulative result bytes, reject changed counts or incomplete
pages, and return no partial collection after refusal. Job collection reserves
both run reads within its call budget.

`RepositoryIssues` supports all issues, open issues without an age restriction,
or issues updated since an explicit UTC timestamp. It fixes the query across
opaque cursor pages and checks stable totals, unique issue numbers and terminal
page counts. It never uses returned dates to stop paging. Recent updates and
older open issues are separate native-selected queries; this library does not
silently widen one into the other or merge their observations into a snapshot.

`IssueComments` reads an exact issue, collects comment pages, and reads that issue
again before returning. It checks issue/comment URLs and IDs, explicit comment
counts and observed final state/count/update changes. Missing counts remain
unknown. Full pages require a short followup, including an empty page for exact
multiples of 100 comments. The budget reserves both issue-detail reads.

`RepositoryPullRequests` enumerates all, open or closed PRs with explicit
creation-order paging. It checks unique numbers, exact PR URLs, the selected
base repository and head/base commit SHAs. A deleted head repository remains
absent, and fork metadata never changes the selected repository. Full pages
require a followup, including an empty page at an exact multiple of 100.
It uses no date cutoff, head/base filter or field projection.

`projected_github_payload` selects the original JSON payload from native content
blocks. The pinned Core MCP projection removes text equal to structured JSON,
so PR reads and commit reads can arrive as a single `Structured` block. Issue
and Actions reads have method-tagged structured wrappers that differ from their
text. List results can also differ through zero-value serialization. The decoder
keeps that companion separately and uses the exact text bytes without merging
fields or concatenating JSON documents. It refuses tool errors, malformed or
extra blocks, missing wrapped text, mismatched wrapper methods and results over
32 MiB across both blocks. It does not assert that a companion's descriptive
fields equal the text or turn omitted text fields into known values.

The native adapter must map actual `ContentBlock` variants directly and pass the
pending call from the matching committed result. `ToolResult::text_content()`
can concatenate two JSON documents and must not feed these programs. A decoder
refusal must end the native cursor; it must not retry the pending observation.
The native collection budget still counts the entire original result, including
its companion and envelope. This helper is not the native adapter or proof of
native settlement, installed credentials or source coverage.

## Pinned upstream behavior

Contract: GitHub MCP server v2.0.3, source
`b98d115bbb080e9555466a96656ec4df605aea21`:

- [PR read implementation](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/pullrequests.go)
- [Result types](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/minimal_types.go)
- [Ordinary comment implementation](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/issues.go)
- [Repository history implementation](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/repositories.go)
- [Workflow run and job implementation](https://github.com/github/github-mcp-server/blob/b98d115bbb080e9555466a96656ec4df605aea21/pkg/github/actions.go)

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
Ordinary comment and review-summary pagination have the same limitation:
`get_comments` and `get_reviews` can filter rows after REST pagination and discard
pagination headers. A short array alone is not proof of the final source page.
The PR's optional `comments` total belongs to ordinary comments, not reviews.

The file and commit methods discard response pagination headers. This program
uses the explicit initial PR totals to detect incomplete or capped responses.
The source also omits zero totals and converts absent upstream totals to zero;
missing totals therefore refuse rather than establish an empty collection.
File pages carry no revision, and matching initial/final PR reads cannot rule
out intervening changes. The result remains a reported page set, not an atomic
snapshot of that head. Patches are optional and may be absent or incomplete;
listing every file does not establish a complete diff.

The history program requests neither author/path filters nor field projection.
The pinned `list_commits` method performs date filtering at the provider and
returns commit metadata without parent links, files or status/check results.
The result records the exact queried interval and retained anchor; it does not
independently prove ancestry or date-filter semantics, current default-branch
identity, complete diffs or historical CI coverage. Empty/malformed anchor
results refuse; provider errors must be handled by the native adapter first.

The Actions programs consume the pinned successful textual JSON payload. Job
pages have the shape `{"jobs":{"total_count":N,"jobs":[...]}}`; the MCP
`structuredContent` projection has a different wrapper and must not be silently
substituted. The native adapter owns explicit envelope selection and tool-error
handling. Missing run/job revision or attempt fields refuse.

The pinned workflow-run list exposes no creation-date or head-SHA filter. This
program therefore enumerates the unfiltered repository list within its finite
budget; an oversized repository refuses instead of claiming seven-day or older
outstanding-run coverage from partial pages. Matching initial/final run reads
detect observed head/attempt changes but do not prove an atomic job snapshot.
Reported states never establish current CI readiness or execution success. Job
URLs remain data; exact typed run/head/attempt fields establish correlation.

The pinned issue list omits repository URLs and database IDs from each row.
Its repository association therefore depends on the exact native committed
call, not independently on returned identity fields. Issue detail reads require
the exact repository/issue URL and reject pull-request URLs. The list's `since`
is a provider-side lower bound, with no upper bound or independently proven date
semantics. No field projection, label or custom-field filter is introduced.

Issue labels and custom fields can be capped in the upstream GraphQL fragment.
Hierarchy enrichment is best-effort, and closing-PR references are capped at
five. Retained metadata does not establish complete relationships, labels or
custom fields; missing keys remain missing. None of these references schedules
another read. Issue comments use the same
filtered-after-pagination implementation as ordinary PR comments, so a short
page does not independently prove complete unfiltered coverage. Matching detail
reads cannot rule out intervening edits. These remain reported page sets.

The pinned PR list supplies no total, discards pagination headers, skips nil
rows and exposes no date filter. A short-page observation therefore does not
independently prove exhaustive recent history or outstanding-work coverage.
Oversized histories refuse at the finite budget. Provider conversion can turn
absent merged/draft fields into false, so those flags remain reported metadata,
not proof that a PR was never merged or is ready. Listing also does not establish
an atomic or still-current set of PR revisions.

Remaining work includes the native bridge/adapter, native selection and
composition of these programs, and installed qualification. Repository metadata and
historical non-PR commit checks still need a supported source.
These programs do not close those requirements or the overall shared ingestion
outcome.

Native compatibility is not established by the pure controls. Connect source
`762c4f4a5fab6f3f2cbb7e4441ffebbaeb3217f6` rejects a repeated requirement,
operation and canonical-argument digest before dispatch. `PullRequestChanges`,
`WorkflowRunJobs` and `IssueComments` intentionally repeat their initial detail
read after paging, so that native engine cannot consume them unchanged. A
supported fresh-observation contract is required. Final verification must not
be removed or bypassed with dummy arguments, requirement relabeling or reused
receipts; failed or uncertain calls must remain non-retryable.

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
