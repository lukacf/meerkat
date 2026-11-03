# Current Gmail verification

This page adds current evidence for the retained Gmail `0.2.0` choices. The
[versioned SUPPORT.md](integrations/connectors/gmail-api-setup/0.2.0/SUPPORT.md)
remains unchanged as part of its hashed review record. Its statement that Node
22 had not been executed describes the earlier freeze; the later receipt below
supersedes that execution limit. No package version, recipe, endpoint, strategy
or activation contract changes with this documentation update.

The [current library](integrations/README.md) has 14 retained package versions
across 11 identities, with 17 strategies: 6 remote service recipes and 11
guidance-only recipes. Gmail `0.1.0` remains available. For `0.2.0`, an agent
explicitly chooses `hosted-mcp-preview` or `toolkit-rest`; neither is an automatic
fallback. The REST selection remains guidance-only until the host supplies the
required authentication, adapter and service-export binding.

The operation libraries cover one-page message listing, selected-message
retrieval and one-attempt MIME send. See [Python Gmail](../../sdk/python/GMAIL.md)
and [TypeScript Gmail](../../sdk/typescript/GMAIL.md) for APIs and limits. The host
owns the authenticated transport, selected account, permissions and recipients.
A send receipt is not delivery proof, and an unknown outcome does not authorize
a retry.

| Local evidence | Recorded result |
| --- | --- |
| [Python wheel packaging successor](../../docs/coordination/20260930-upstream-review/gmail-operations-verification/license-packaging-correction/summary.json) | 20 Gmail loopback cases pass against the installed wheel; exact MIT and Apache-2.0 texts and package metadata checked |
| [TypeScript Node 22.0.0 successor](../../docs/coordination/20260930-upstream-review/gmail-typescript-node22-verification/20261001T043213.702428Z/summary.json) | 36 loopback cases pass against both unchanged repository build and exact installed npm artifact: 21 Gmail and 15 Home Assistant; macOS arm64 |
| [Catalog successor](../../docs/coordination/20260930-upstream-review/gmail-library-successor-verification/20261001T044817.044748Z/summary.json) | 7 library, 6 discovery-adapter and 2 bundled-metadata cases pass, plus scoped strict Clippy and formatting checks |

The Node 22.0.0 receipt establishes the declared minimum runtime for those
operations on macOS arm64. It does not cover every Node version, other platforms
or browser Fetch behavior. The earlier Node 25.9.0 results and failed initial
runtime lookup remain retained; no SDK source or artifact was changed for the
successful Node 22 run.

The catalog run uses the historical local Meerkat 0.8.44 development graph,
optional MobKit 0.8.43, default features and the existing Cargo patch map with
`--locked`. It does not enable `native-host` or execute an actual MobKit member.
The [independent closure](../../docs/coordination/20260930-upstream-review/gmail-library-successor-independent-review/execution-closure/REVIEW.md)
binds those narrower results; [verification commands](README.md#validation)
retain the required configuration.

The wheel and npm archive are unpublished local builds, selected by their exact
hashes in the receipts and retained support record. Import names and version
`0.1.0` are not evidence of a public PyPI or npm release. None of these results
certifies Google authentication, live provider schemas or operations, connector
activation, human approval, native OAuth/C1 integration, host migration or full
library completion. Those requirements remain open.
