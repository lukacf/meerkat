# J: Archive, historical evidence, proposals and plans

[Audit index](README.md) | [Coverage](coverage.md)

Original documentation and initial evidence line ranges refer to baseline `af82b6b3ab34faed9bf3e962d148d55f10dcd1dc`, unless an external dependency or historical revision is explicitly identified. Final-review citations refer to the corrected files in this change. Source excerpts may be de-indented or omit intervening lines; cited ranges identify the complete context. Quoted defects are preserved as evidence, not current usage guidance.

No actionable defects were found. The independent adjudicator challenged archive coverage and historical framing without inventing current-code requirements for old proposals.

## Independent scope checks

> [
>   {
>     "name": "Independent scope census",
>     "method": "Enumerated every .md/.mdx under docs/archive with pathlib and find; compared the path set to audit-J.json coverage; counted source lines and checked symlink status. Recorded SHA-256 for every file.",
>     "result": "Exact path-set equality: 12 documents, 4617 lines, zero symlinks, no uncovered scope paths. git rev-parse HEAD matched the supplied baseline. git status --porcelain=v1 was empty before artifact creation."
>   },
>   {
>     "name": "Publication and direct-arrival framing",
>     "result": "The sole routed archive MDX page has an explicit historical title, description, warning, retirement notes, and current-guide links. Both internal archive subtrees remain excluded.",
>     "evidence": [
>       {
>         "path": "docs/.mintignore",
>         "lines": "6-10",
>         "quote": "# Archived internal records keep that status. The archived plan page under\n# archive/plans/ stays published; these records were never part of the\n# published surface and do not become part of it by being archived.\narchive/design/\narchive/proposals/",
>         "explanation": "The design/proposal records are not accidentally promoted to published operational instructions."
>       },
>       {
>         "path": "docs/docs.json",
>         "lines": "22-23",
>         "quote": "\"source\": \"/plans/storage-unification-plan\",\n      \"destination\": \"/archive/plans/storage-unification-plan-2026-07\"",
>         "explanation": "The legacy public URL targets an existing archive page, rather than an excluded internal record or missing file."
>       },
>       {
>         "path": "docs/archive/plans/storage-unification-plan-2026-07.mdx",
>         "lines": "7-10",
>         "quote": "<Warning>\nThis page is an archived implementation record from July 2026. The storage\nunification arc was implemented, and the text below is preserved as written\nrather than maintained as current operations guidance.",
>         "explanation": "Readers arriving directly through the redirect receive the historical boundary before the old implementation instructions."
>       }
>     ]
>   },
>   {
>     "name": "Archive navigation and current-guide landing sections",
>     "method": "Scanned all archive documents for Markdown destinations, reference definitions, HTML href/src, and raw link-like text. Excluded code fences and inline code, then independently inspected raw matches. Resolved root-relative MDX destinations and normalized heading anchors.",
>     "result": "Exactly three actual navigation links, all in the storage warning; all three resolve to existing sections. No other Markdown/HTML navigation targets require repair. reports.md:25 is an inline-code format example, not a broken file.md link.",
>     "evidence": [
>       {
>         "path": "docs/reference/configuration.mdx",
>         "lines": "506-563",
>         "quote": "## Storage layout and durability",
>         "explanation": "The /reference/configuration#storage-layout-and-durability landing section exists and covers canonical names, legacy probes, declared durability, and storage census surfaces."
>       },
>       {
>         "path": "docs/concepts/sessions.mdx",
>         "lines": "87-107",
>         "quote": "## On-disk layout and durability",
>         "explanation": "The /concepts/sessions#on-disk-layout-and-durability section exists and provides the session-storage guidance advertised by the archive warning."
>       },
>       {
>         "path": "docs/api/rpc.mdx",
>         "lines": "1142-1176",
>         "quote": "## Storage methods\n\n### `mobkit/storage/doctor`",
>         "explanation": "The /api/rpc#storage-methods section exists and documents the read-only doctor RPC. It is not a dangling or archive-self-referential escape link."
>       }
>     ]
>   },
>   {
>     "name": "Storage warning against implementation",
>     "result": "The warning's two retired assumptions are correct; it also preserves the distinction between retired checkpoint stamping and still-supported import/conversion paths.",
>     "evidence": [
>       {
>         "path": "meerkat-mobkit/src/storage_migrate.rs",
>         "lines": "50-52",
>         "quote": "//! 4. **(Retired with the 0.8.11 reset.)** Continuity checkpoint adoption\n//!    minted embedded checkpoint stamps meerkat no longer reads; released\n//!    0.8.10 documents now convert through the explicit one-time importer.",
>         "explanation": "Corroborated by the removed-case explanation at 809-813; the plan's old H3 should not be reinstated as current guidance."
>       },
>       {
>         "path": "meerkat-mobkit/src/identity_first/local_store.rs",
>         "lines": "1431-1450",
>         "quote": "/// Same-transaction one-time import of a released 0.8.10 blob row.",
>         "explanation": "The implementation calls meerkat_core::import_released_0810_session, verifies the receipt against the observed bytes, and handles read-only adoption separately. The warning does not falsely claim all legacy interpretation disappeared."
>       },
>       {
>         "path": "meerkat-mobkit/src/identity_first/adapters.rs",
>         "lines": "1140-1145",
>         "quote": "// No persisted canonical head. For a REGISTERED session, the\n            // SYNTHESIZING read decides: a legacy/imported blob synthesizes\n            // a head and converts on this very write (the 0.8.10\n            // lazy-migration dance - the store migrates the blob inside the\n            // first delta write's transaction), while a truly fresh session",
>         "explanation": "The actual capability-resolution branch retains the separate first-delta conversion described by the warning."
>       },
>       {
>         "path": "meerkat-mobkit/src/storage_doctor.rs",
>         "lines": "381-427",
>         "quote": "/// The M6 mutation verbs ([`Self::migrate`], [`Self::prune`]) are **inherent\n/// methods on this concrete type**, not additions to the meerkat-owned\n/// [`StorageMigrator`] trait and not a mobkit-side extension trait: the core\n/// trait stays diagnose-only (meerkat owns its contract), and an extension",
>         "explanation": "Verified the inherent migrate/prune bodies and the distinct impl StorageMigrator containing only diagnose, not merely this explanatory comment."
>       },
>       {
>         "path": "meerkat-mobkit/src/bin/mobkit_gateway.rs",
>         "lines": "1323-1328",
>         "quote": "if args.first().map(String::as_str) == Some(\"storage-migrate\") {\n        std::process::exit(run_storage_migrate(&args[1..]));\n    }\n    if args.first().map(String::as_str) == Some(\"storage-prune\") {\n        std::process::exit(run_storage_prune(&args[1..]));\n    }",
>         "explanation": "Both mutating CLI entrypoints named by the warning are wired."
>       }
>     ]
>   },
>   {
>     "name": "Console dispositions against current contract and handlers",
>     "result": "The archive's explanation of which proposals shipped remains substantively accurate despite drift in line-number hints.",
>     "evidence": [
>       {
>         "path": "meerkat-mobkit/src/rpc.rs",
>         "lines": "2037-2042",
>         "quote": "if identity_ctx.is_some() {\n                methods.extend_from_slice(&[\n                    \"mobkit/send\",\n                    \"mobkit/interact\",",
>         "explanation": "Confirms conditional advertisement; dispatch at 3608-3735 independently resolves the identity and returns the identity-stream route."
>       },
>       {
>         "path": "meerkat-mobkit/src/http_console.rs",
>         "lines": "462-483",
>         "quote": "\"/console/identity/{identity}/stream\",\n            get(console_identity_timeline_stream_handler),",
>         "explanation": "Read the entire route block: GET timeline/stream and POST console/rpc are present, supporting the historical memo's maintained status note."
>       },
>       {
>         "path": "docs/guides/console.mdx",
>         "lines": "533-536",
>         "quote": "- `mobkit/console/send` - identity-addressed console send\n- `mobkit/interact` - identity-addressed interaction that returns a `/console/identity/{identity}/stream` route",
>         "explanation": "The live guide still documents interact as the archive index says; its old :374 locator is only a drifted line hint."
>       },
>       {
>         "path": "packages/console-core/src/contract.ts",
>         "lines": "73",
>         "quote": "export const CONSOLE_TIMELINE_QUERY_MODES = [\"since\", \"recent\"] as const;",
>         "explanation": "Matched against both implemented branches in console_aggregator/mod.rs:1203-1215, not merely a declaration."
>       },
>       {
>         "path": "docs/rct/console-rest-sse-contract-v0.5.0.json",
>         "lines": "2",
>         "quote": "\"contract_version\": \"0.5.0\",",
>         "explanation": "The replacement contract exists and contains current REST/RPC/timeline routes; older v0.3/v0.4 mentions in May records remain historical."
>       }
>     ]
>   },
>   {
>     "name": "Concrete artifact and historical ledger checks",
>     "result": "Both console package manifests declare private=true; all three fixture directories and the six MDM artifacts named by the index exist. Source includes the extracted controller and component implementations, external MDM backend/supervisor bridge, and signed target comms/binding construction. Exactly 31 trace rows are VALIDATED, seven survey SYSTEM sections and six GAP sections exist.",
>     "evidence": [
>       {
>         "path": "packages/console-core/src/headless.ts",
>         "lines": "482",
>         "quote": "export function createMobKitConsoleController({",
>         "explanation": "The index's extraction claim is backed by actual implementation, not empty package directories."
>       },
>       {
>         "path": "examples/004-mdm-console-pack/target.rs",
>         "lines": "766-771",
>         "quote": "let comms_runtime = create_comms_runtime(&args).await?;",
>         "explanation": "The runtime startup creates comms, creates/resumes a session, and writes a binding. The runner independently declares the external supervisor bridge and external backend."
>       },
>       {
>         "path": "CHANGELOG.md",
>         "lines": "2450-2454",
>         "quote": "## [0.8.16] - 2026-08-11\n\nPaired release on meerkat v0.8.22. Delivers the owner-ratified 26-item\nprogram; every item's disposition is stated below, including the items\ndeliberately refused and the ones shipped as documented partials.",
>         "explanation": "Read through the actual refusals and not-in-release list. The archive index does not imply that all original cut proposals were applied."
>       },
>       {
>         "path": "CHANGELOG.md",
>         "lines": "2567-2569",
>         "quote": "### Fixed\n- Firing-intent schedule writes (`create`/`update`/`resume`) refuse typed while a gateway-owned store has no firing host bound (Bug C class); caller-injected library stores are never gated.\n- External-tool composition warns per shadowed pre-installed tool (the scoped form of the recorder clobber).",
>         "explanation": "The two narrow 0.8.15 stopgap claims match the release record; FiringHostGatedScheduleTools and the shadow warning also remain in source."
>       }
>     ]
>   },
>   {
>     "name": "Live-document boundaries and inbound references",
>     "result": "The deliberately-live paths all exist and their stated reasons are independently observable. Source cites upstream-asks.md at live_wiring.rs:1379 and workgraph_admission.rs:376; the memory gate/allowlist cite memory-hub-roadmap.md; memory.mdx references it twice; the memory console header remains proposal; the MDM deployment model still names the live example. The live architecture evidence link and upstream-asks followup link resolve to the moved survey.",
>     "evidence": [
>       {
>         "path": "docs/design/agent-memory-architecture.md",
>         "lines": "17-21",
>         "quote": "Evidence base: five-system survey (Claude Code, Codex, Meerkat, MobKit, an external memory hub)\nwith adversarially verified findings, committed at\n[`../archive/design/evidence/memory-survey-2026-07/`](../archive/design/evidence/memory-survey-2026-07/). File:line citations below\nrefer to the surveyed checkouts (2026-07-01); the archive README carries the\nstaleness caveat.",
>         "explanation": "The active consumer preserves the date/provenance boundary rather than advertising the archived survey as current implementation truth."
>       },
>       {
>         "path": "docs/archive/design/evidence/memory-survey-2026-07/README.md",
>         "lines": "21-22",
>         "quote": "File:line citations reflect the surveyed checkouts on 2026-07-01; verify against\ncurrent code before relying on them.",
>         "explanation": "Explicitly prevents treating the external repository paths and old memory APIs in the survey as current runnable MobKit guidance."
>       }
>     ]
>   },
>   {
>     "name": "Prior-work and no-edit check",
>     "method": "git show --format=oneline --name-only 01435e7ccda5e925fc2cf9f462327bdae9584d07 -- docs/archive; git status --porcelain=v1",
>     "result": "No archive changes in the prior audit commit; empty repository status. No finding was excluded on prior-commit identity. No repository edits, delegation, maintenance operations, dependency changes, or historical smoke commands were performed."
>   }
> ]

## Final scope checks

> [
>   "Initial audit read all 12 archived documents (4,617 lines), finding no actionable defects.",
>   "Separate wave-2 adjudicator independently challenged coverage, framing, links and historical authority; reported zero supplemental candidates.",
>   "Final coordinator ran git diff --exit-code -- docs/archive: successful, no archive modifications.",
>   "Historical console contract v0.1-v0.4 files also remain byte-identical to baseline, preserving rejected C-028.",
>   "All 28 tracked MDX files compile, including the published archived plan. Its docs/docs.json redirect resolves."
> ]
