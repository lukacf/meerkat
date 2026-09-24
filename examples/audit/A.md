# Evidence group A

[Audit index](../AUDIT.md)

<a id="a01"></a>

### A01: Fourteen registered example entry points bypass the shared host stack budget

**Severity:** medium. **Verdict:** confirmed. **Examples:** 001, 005, 006, 009, 011, 012, 013, 014, 015, 017, 018, 019, 024, 025.

**Original proof:** Static host-entry audit: each of A's six mains contains #[tokio::main] and none calls meerkat_runtime::host_stack::run_host; audit B independently supplied the same exact observation for its eight cited mains. The supplied runtime helper is therefore not consulted, and setting RKAT_WORKER_STACK_BYTES cannot change these fourteen examples' main/worker stacks. No stack overflow was experimentally reproduced; this is a concrete host-stack contract bypass, not a claim that every invocation currently crashes.

- [`examples/001-hello-meerkat-rs/main.rs:29-30`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/001-hello-meerkat-rs/main.rs#L29-L30) The entry point uses #[tokio::main], not run_host.
- [`examples/005-streaming-events-rs/main.rs:27-28`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/005-streaming-events-rs/main.rs#L27-L28) The entry point uses #[tokio::main].
- [`examples/006-custom-tools-rs/main.rs:152-153`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/006-custom-tools-rs/main.rs#L152-L153) The entry point uses #[tokio::main].
- [`examples/009-budget-and-retry-rs/main.rs:26-27`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L26-L27) The entry point uses #[tokio::main].
- [`examples/011-hooks-guardrails-rs/main.rs:78-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/main.rs#L78-L79) The entry point uses #[tokio::main].
- [`examples/012-skills-loading-rs/main.rs:46-47`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs#L46-L47) The entry point uses #[tokio::main].
- [`examples/013-context-compaction-rs/main.rs:26-27`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/main.rs#L26-L27) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/014-semantic-memory-rs/main.rs:36-37`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/014-semantic-memory-rs/main.rs#L36-L37) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/015-session-persistence-rs/main.rs:26-27`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/015-session-persistence-rs/main.rs#L26-L27) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/017-mob-coding-swarm-rs/main.rs:87-88`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/main.rs#L87-L88) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/018-mob-research-team-rs/main.rs:85-86`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/018-mob-research-team-rs/main.rs#L85-L86) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/019-mob-pipeline-rs/main.rs:130-131`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs#L130-L131) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/024-host-mode-event-mesh-rs/main.rs:39-40`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/024-host-mode-event-mesh-rs/main.rs#L39-L40) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`examples/025-full-stack-agent-rs/main.rs:121-122`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L121-L122) Audit B verified #[tokio::main], a direct async main, and no run_host call anywhere in this file.
- [`crates/meerkat-runtime/src/host_stack.rs:1-27,31-34,120-178,186-196`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-runtime/src/host_stack.rs) The shared 8 MiB budget covers workers AND the thread polling the main future; run_host honors RKAT_WORKER_STACK_BYTES. Its documentation records measured debug stack requirements and the platform-main-stack distinction.
- [`crates/meerkat-session/src/ephemeral.rs:5402-5426`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/ephemeral.rs#L5402-L5426) The ephemeral service used by 001 runs session_task on a Tokio task, so its agent is not confined to the larger macOS platform main stack.

**Independent challenge:** All 14 manifest-registered numbered mains independently contain #[tokio::main] and no run_host call. The repository explicitly requires the common host entry seam. run_host resolves RKAT_WORKER_STACK_BYTES and budgets both runtime workers and the dedicated main-future thread; the Tokio macro does neither through this seam. This is a verified contract bypass, not evidence that any particular example currently overflows. The larger macOS main stack is not a counterexample to the worker issue: 001's session service spawns its session task onto Tokio.

**Accepted correction:** Replace only the 14 example host entry wrappers with the shared run_host seam, preserving their async bodies and truthful error propagation. Use Send-compatible returned errors as required. Do not raise stack budgets, invent per-example runtime builders, or claim reproduced crashes.

- [`crates/meerkat/Cargo.toml:265-319`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/Cargo.toml#L265-L319) Independently enumerated 11 registered numbered examples and their source paths.
- [`crates/meerkat-mob/Cargo.toml:174-184`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-mob/Cargo.toml#L174-L184) Independently enumerated the other three numbered examples.
- [`examples/001-hello-meerkat-rs/main.rs:29-30`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/001-hello-meerkat-rs/main.rs#L29-L30) Tokio macro async main; no run_host anywhere in this file.
- [`examples/005-streaming-events-rs/main.rs:27-28`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/005-streaming-events-rs/main.rs#L27-L28) Tokio macro async main; no run_host anywhere in this file.
- [`examples/006-custom-tools-rs/main.rs:152-153`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/006-custom-tools-rs/main.rs#L152-L153) Tokio macro async main; no run_host anywhere in this file.
- [`examples/009-budget-and-retry-rs/main.rs:26-27`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L26-L27) Tokio macro async main; no run_host anywhere in this file.
- [`examples/011-hooks-guardrails-rs/main.rs:78-79`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/main.rs#L78-L79) Tokio macro async main; no run_host anywhere in this file.
- [`examples/012-skills-loading-rs/main.rs:46-47`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs#L46-L47) Tokio macro async main; no run_host anywhere in this file.
- [`examples/013-context-compaction-rs/main.rs:26-27`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/013-context-compaction-rs/main.rs#L26-L27) Independently checked Tokio macro async main and no run_host.
- [`examples/014-semantic-memory-rs/main.rs:36-37`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/014-semantic-memory-rs/main.rs#L36-L37) Independently checked Tokio macro async main and no run_host.
- [`examples/015-session-persistence-rs/main.rs:26-27`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/015-session-persistence-rs/main.rs#L26-L27) Independently checked Tokio macro async main and no run_host.
- [`examples/017-mob-coding-swarm-rs/main.rs:87-88`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/017-mob-coding-swarm-rs/main.rs#L87-L88) Independently checked Tokio macro async main and no run_host.
- [`examples/018-mob-research-team-rs/main.rs:85-86`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/018-mob-research-team-rs/main.rs#L85-L86) Independently checked Tokio macro async main and no run_host.
- [`examples/019-mob-pipeline-rs/main.rs:130-131`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/019-mob-pipeline-rs/main.rs#L130-L131) Independently checked Tokio macro async main and no run_host.
- [`examples/024-host-mode-event-mesh-rs/main.rs:39-40`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/024-host-mode-event-mesh-rs/main.rs#L39-L40) Independently checked Tokio macro async main and no run_host.
- [`examples/025-full-stack-agent-rs/main.rs:121-122`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/025-full-stack-agent-rs/main.rs#L121-L122) Independently checked Tokio macro async main and no run_host.
- [`CLAUDE.md:520`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/CLAUDE.md#L520) Explicit prohibition on bypassing the common host stack budget with #[tokio::main].
- [`crates/meerkat-runtime/src/host_stack.rs:31-38,86-116,132-196`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-runtime/src/host_stack.rs) One budget/override parser; thread_stack_size for workers and stack_size for the dedicated named main thread.
- [`crates/meerkat-session/src/ephemeral.rs:5400-5425`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/ephemeral.rs#L5400-L5425) Native session_task is spawned onto Tokio.

**Implementation (fixed):** Six owned hosts use run_host and Send+Sync errors. All six real executables reject invalid RKAT_WORKER_STACK_BYTES before any filesystem side effects and exit 1 promptly with missing auth. Shared finding's other eight wrappers belong to fix-B. No larger stack or claim of reproduced overflow/stack sufficiency.
Changed: `examples/001-hello-meerkat-rs/main.rs`, `examples/005-streaming-events-rs/main.rs`, `examples/006-custom-tools-rs/main.rs`, `examples/009-budget-and-retry-rs/main.rs`, `examples/011-hooks-guardrails-rs/main.rs`, `examples/012-skills-loading-rs/main.rs`.
- `./scripts/repo-cargo build --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --message-format=json`: **pass** Built six actual executables; paths recorded in fixed-A-binaries.jsonl. Isolated subprocess probes recorded in fixed-A-entry-probes.json verify invalid-stack rejection with untouched roots plus missing-auth exits. No live provider invoked.
- `./scripts/repo-cargo clippy --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills -- -D warnings`: **pass** All six scoped example targets pass; fixed-A-clippy.log.

**Implementation (fixed):** Owned eight-main subset now uses shared run_host wrappers with Send+Sync errors and unchanged async bodies. No custom runtime or stack budget, no stack-size increase. Agent A owns the other six mains.
Changed: `examples/013-context-compaction-rs/main.rs`, `examples/014-semantic-memory-rs/main.rs`, `examples/015-session-persistence-rs/main.rs`, `examples/017-mob-coding-swarm-rs/main.rs`, `examples/018-mob-research-team-rs/main.rs`, `examples/019-mob-pipeline-rs/main.rs`, `examples/024-host-mode-event-mesh-rs/main.rs`, `examples/025-full-stack-agent-rs/main.rs`.
- `Python per-file assertions for run_host, absent #[tokio::main] and absent thread_stack_size`: **pass** All eight owned mains use the shared host seam. Static proof only, not a linked stack-sufficiency claim.
- `Parent build all eight executables, then execute each with RKAT_WORKER_STACK_BYTES=invalid`: **blocked** Parent owns shared Cargo and executable checks. Expected invalid override refusal before credential, filesystem or provider work. Shared helper named-main-thread regression should also be run by parent.

**Independent fix review (fixed):** All 14 registered numbered mains now call run_host, preserving their async bodies and propagating both host and application errors through Send+Sync error returns. Independently linked and executed every host-entry refusal path. This verifies use of the shared seam, not that every live execution fits a particular stack size; the unchanged runtime helper's named-thread unit test was not separately rerun.
- [`examples/001-hello-meerkat-rs/main.rs:30-34`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/001-hello-meerkat-rs/main.rs#L30-L34) Fallible shared host entry.
- [`examples/005-streaming-events-rs/main.rs:30-32`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/005-streaming-events-rs/main.rs#L30-L32) Fallible shared host entry.
- [`examples/006-custom-tools-rs/main.rs:155-157`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/006-custom-tools-rs/main.rs#L155-L157) Fallible shared host entry.
- [`examples/009-budget-and-retry-rs/main.rs:27-29`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L27-L29) Fallible shared host entry.
- [`examples/011-hooks-guardrails-rs/main.rs:78-80`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/main.rs#L78-L80) Fallible shared host entry.
- [`examples/012-skills-loading-rs/main.rs:46-50`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L46-L50) Fallible shared host entry.
- [`examples/013-context-compaction-rs/main.rs:26-28`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/main.rs#L26-L28) Fallible shared host entry.
- [`examples/014-semantic-memory-rs/main.rs:37-39`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/014-semantic-memory-rs/main.rs#L37-L39) Fallible shared host entry retained after approved full-body test extraction.
- [`examples/015-session-persistence-rs/main.rs:26-30`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/015-session-persistence-rs/main.rs#L26-L30) Fallible shared host entry.
- [`examples/017-mob-coding-swarm-rs/main.rs:90-92`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/main.rs#L90-L92) Fallible shared host entry.
- [`examples/018-mob-research-team-rs/main.rs:89-91`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/018-mob-research-team-rs/main.rs#L89-L91) Fallible shared host entry.
- [`examples/019-mob-pipeline-rs/main.rs:135-137`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L135-L137) Fallible shared host entry retained after approved full-body test extraction.
- [`examples/024-host-mode-event-mesh-rs/main.rs:40-42`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/024-host-mode-event-mesh-rs/main.rs#L40-L42) Fallible shared host entry retained after approved full-body test extraction.
- [`examples/025-full-stack-agent-rs/main.rs:126-128`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L126-L128) Fallible shared host entry.
- [`crates/meerkat-runtime/src/host_stack.rs:133-194`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-runtime/src/host_stack.rs#L133-L194) Owner implementation applies the resolved budget to runtime threads and the dedicated main-future thread; run_host validates the environment first.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo build --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --message-format=json`: **pass** All 14 linked artifacts independently executed with RKAT_WORKER_STACK_BYTES=invalid-final-review-value: 14/14 exit 1, InvalidOverride, empty stdout, no timeout. Combined tests and Clippy also passed.

**Independent fix review (fixed):** B's eight entry points use the shared run_host seam with Send+Sync fallible results, complementing A's six. All eight were independently linked and actually refused an invalid override before their async bodies. No custom stack increase or stack-sufficiency assertion.
- [`examples/013-context-compaction-rs/main.rs:26-28`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/013-context-compaction-rs/main.rs#L26-L28) run_host wrapper.
- [`examples/014-semantic-memory-rs/main.rs:37-39`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/014-semantic-memory-rs/main.rs#L37-L39) run_host wrapper retained after body extraction.
- [`examples/015-session-persistence-rs/main.rs:26-30`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/015-session-persistence-rs/main.rs#L26-L30) run_host wrapper.
- [`examples/017-mob-coding-swarm-rs/main.rs:90-92`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/017-mob-coding-swarm-rs/main.rs#L90-L92) run_host wrapper.
- [`examples/018-mob-research-team-rs/main.rs:89-91`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/018-mob-research-team-rs/main.rs#L89-L91) run_host wrapper.
- [`examples/019-mob-pipeline-rs/main.rs:135-137`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/019-mob-pipeline-rs/main.rs#L135-L137) run_host wrapper retained after whole-pipeline extraction.
- [`examples/024-host-mode-event-mesh-rs/main.rs:40-42`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/024-host-mode-event-mesh-rs/main.rs#L40-L42) run_host wrapper retained after body extraction.
- [`examples/025-full-stack-agent-rs/main.rs:126-128`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/025-full-stack-agent-rs/main.rs#L126-L128) run_host wrapper.
- [`crates/meerkat-runtime/src/host_stack.rs:133-194`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-runtime/src/host_stack.rs#L133-L194) Resolved budget remains owned by runtime and is applied to worker and main-future threads.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo build --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --message-format=json`: **pass** B's 8/8 executables returned exit 1 with InvalidOverride and empty stdout when run with RKAT_WORKER_STACK_BYTES=invalid-final-review-value. Combined tests/Clippy passed. Unchanged host helper's separate named-main-thread unit test was not rerun.

<a id="a02"></a>

### A02: The supposedly in-memory hello example persists transcripts in the user data root

**Severity:** medium. **Verdict:** partial. **Examples:** 001.

**Original proof:** Static exact build path: documented --features jsonl-store -> build_ephemeral_service -> FactoryAgentBuilder with no default store -> AgentFactory JSONL feature fallback -> JsonlStore::init/save at the user-global realm-derived directory. A live run was deliberately not used to mutate the user's global state.

- [`examples/001-hello-meerkat-rs/README.md:7,14-15`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/001-hello-meerkat-rs/README.md) Describes an in-memory SessionService constructor while requiring jsonl-store in the run command.
- [`examples/001-hello-meerkat-rs/main.rs:7,29-39`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/001-hello-meerkat-rs/main.rs) Uses a user-global default_state_root-derived path and passes no in-memory session-store override.
- [`crates/meerkat/src/service_factory.rs:1111-1123,1391-1396,1548-1556`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/service_factory.rs) build_ephemeral_service constructs FactoryAgentBuilder with default_session_store=None; it does not force a memory component store.
- [`crates/meerkat/src/factory.rs:6199-6226`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs#L6199-L6226) Without an override or custom store, jsonl-store wins over memory-store and initializes JsonlStore at factory.store_path.
- [`crates/meerkat-core/src/agent/state.rs:735-746`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L735-L746) Without a checkpointer the standalone agent still saves its Session through its AgentSessionStore.
- [`crates/meerkat-store/src/jsonl.rs:139-164`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-store/src/jsonl.rs#L139-L164) JsonlStore::init creates the persistent directory; this is not a volatile implementation.

**Independent challenge:** The documented jsonl-store feature really does select a persistent JSONL component store at the global-data-root-derived factory path. However, the SessionService's in-memory lifecycle/substrate description is technically true, and main.rs explicitly comments that the root is user-global. Thus this is misleading/incomplete user-facing durability guidance, not proof that build_ephemeral_service violates its API contract or that ephemeral service must force all components into memory.

**Accepted correction:** Correct the example's durability story, without redesigning EphemeralSessionService. A minimal accepted fix explicitly distinguishes volatile service lifecycle from retained JSONL transcripts in the README, identifies the example-owned path and cleanup/retention behavior, and scopes any disk demonstration to an owned directory. Alternatively, preserve the intended in-memory lesson by explicitly injecting a memory/ephemeral component store and aligning its run/features documentation; do not rely on the constructor name to choose storage.

- [`examples/001-hello-meerkat-rs/README.md:7,12-16`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/001-hello-meerkat-rs/README.md) Calls the constructor in-memory but the run command enables jsonl-store and the README discloses no retention.
- [`examples/001-hello-meerkat-rs/main.rs:31-39`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/001-hello-meerkat-rs/main.rs#L31-L39) Counterevidence to wholly hidden intent: source explicitly says user-global data-dir root, then supplies no store override.
- [`crates/meerkat/src/surface/embedded.rs:54-58`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/surface/embedded.rs#L54-L58) The service really is EphemeralSessionService; the helper does not replace the factory component store.
- [`crates/meerkat/src/service_factory.rs:1111-1124,1391-1397,1548-1556`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/service_factory.rs) No default store override is injected by this constructor.
- [`crates/meerkat/src/factory.rs:6199-6228`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs#L6199-L6228) JSONL is the selected feature fallback when neither request nor factory supplies a store.
- [`crates/meerkat-store/src/jsonl.rs:157-164`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-store/src/jsonl.rs#L157-L164) init creates a directory; the index path is on disk.
- [`crates/meerkat-core/src/agent/state.rs:735-746`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L735-L746) Absent a checkpointer, agent save uses its component store.

**Implementation (fixed):** Narrowed durability repair: volatile service lifecycle plus explicitly injected scoped JSONL component store. Example prints its owned current-directory scratch path and removes transcripts/index on normal completion or returned error after service drops. Forced-termination retention caveat documented.
Changed: `examples/001-hello-meerkat-rs/main.rs`, `examples/001-hello-meerkat-rs/tests.rs`, `examples/001-hello-meerkat-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 001 scoped_jsonl_persists_content_not_service_lifecycle_then_cleans_up passes using capturing fake client and actual scoped_factory. Verifies JSONL/index files, forgotten lifecycle with files still retained, and final guard cleanup. fixed-A-tests.log.

**Independent fix review (fixed):** The narrowed scoped-JSONL option is implemented rather than claiming EphemeralSessionService is disk-free. Explicit store/runtime roots are inside the guarded current-directory tree, and the README distinguishes retained component bytes from volatile lifecycle and forced-termination leftovers.
- [`examples/001-hello-meerkat-rs/main.rs:36-55`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/001-hello-meerkat-rs/main.rs#L36-L55) Explicit initialized JsonlStore, runtime_root and session_store injection; guard precedes the service.
- [`examples/001-hello-meerkat-rs/README.md:12-24`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/001-hello-meerkat-rs/README.md#L12-L24) Exact transcript/index paths, normal/error cleanup and no automatic lifecycle recovery are documented.
- [`examples/001-hello-meerkat-rs/tests.rs:51-106`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/001-hello-meerkat-rs/tests.rs#L51-L106) Real service with synthetic client writes JSONL/index, preserves files after service drop, starts a fresh empty lifecycle, and removes the owned tree on guard drop.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** scoped_jsonl_persists_content_not_service_lifecycle_then_cleans_up passed against actual scoped_factory/service/store code. No provider call.

<a id="a03"></a>

### A03: The custom streaming handler buffers deltas instead of flushing them

**Severity:** low. **Verdict:** confirmed. **Examples:** 005.

**Original proof:** Precise static I/O proof: Option B's print!("{delta}") writes through Rust's line-buffered stdout, with no flush in the event loop. A sequence of text deltas without a newline remains buffered until a newline/final print or buffer fill, unlike Option A. No live timing measurement was performed.

- [`examples/005-streaming-events-rs/README.md:3-4,11`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/005-streaming-events-rs/README.md) Promises real-time event processing and presents custom event processing as a streaming pattern.
- [`examples/005-streaming-events-rs/main.rs:81-95`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/005-streaming-events-rs/main.rs#L81-L95) Option B prints each delta without a newline and never flushes stdout; the completed-turn message is on stderr.
- [`crates/meerkat/src/sdk.rs:732-746`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/sdk.rs#L732-L746) The built-in logger used by Option A explicitly flushes stdout after each TextDelta.

**Independent challenge:** The custom branch prints newline-free deltas without flushing. An independent actual-Rust std-only subprocess wrote a first delta and an stderr readiness marker, then waited for input: the first stdout delta was not readable after 250 ms; an explicit final flush exposed firstsecond. This directly demonstrates the I/O defect without relying on provider timing. It is intermittent with real text because newlines can flush naturally.

**Accepted correction:** Flush stdout after each custom-handler text delta, propagating or explicitly reporting I/O errors without stranding the event consumer. Keep event/statistics handling otherwise unchanged.

- [`examples/005-streaming-events-rs/main.rs:81-95,109-110`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/005-streaming-events-rs/main.rs) print! in the delta branch has no flush; completion is reported on stderr and the eventual println occurs after processor completion.
- [`crates/meerkat/src/sdk.rs:737-746`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/sdk.rs#L737-L746) The built-in logger explicitly flushes each text delta, confirming the intended contrasting behavior.

**Implementation (fixed):** Actual custom event processor flushes every delta, propagates I/O errors, closes its receiver on failure and is joined even when agent execution errors.
Changed: `examples/005-streaming-events-rs/main.rs`, `examples/005-streaming-events-rs/tests.rs`, `examples/005-streaming-events-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 005 both tests pass: real consumer through buffered Unix socket exposes first newline-free delta before next event/channel close; broken flush error returns and closes receiver.

**Independent fix review (fixed):** The actual custom event processor flushes each delta and propagates I/O failure. Its receiver drops on error; the caller joins it even when the agent returns an error.
- [`examples/005-streaming-events-rs/main.rs:34-58`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/005-streaming-events-rs/main.rs#L34-L58) write_all and flush are fallible per TextDelta; channel ownership is scoped to the processor.
- [`examples/005-streaming-events-rs/main.rs:104-125`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/005-streaming-events-rs/main.rs#L104-L125) Actual custom-handler path joins before propagating the saved run result.
- [`examples/005-streaming-events-rs/tests.rs:5-62`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/005-streaming-events-rs/tests.rs#L5-L62) Buffered Unix socket observes the first delta while channel/task remain open; BrokenPipe is returned and closes the receiver.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Both real-processor tests passed, including the Unix pipe-like buffering check on this macOS host.

<a id="a04"></a>

### A04: Weather tool accepts unsupported units and silently labels Celsius values as those units

**Severity:** medium. **Verdict:** confirmed. **Examples:** 006.

**Original proof:** Dispatch get_weather with {"city":"Tokyo","unit":"kelvin"}. The inspected branch selects 28.0, preserves unit="kelvin", and returns is_error=false. Thus the result is 28 kelvin rather than rejecting an unsupported unit. This is a direct deterministic branch trace; the Rust dispatcher was not executed in this audit lane.

- [`examples/006-custom-tools-rs/main.rs:34-45,86-111`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/006-custom-tools-rs/main.rs) The documented celsius/fahrenheit argument is a free String. Every value except the exact string fahrenheit takes the Celsius branch, but the original arbitrary unit is echoed into a successful result.
- [`crates/meerkat-core/src/types.rs:1102-1117`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/types.rs#L1102-L1117) ToolCallView::parse_args only runs serde deserialization; a String accepts kelvin or a typo.
- [`crates/meerkat-tools/src/schema.rs:11-17`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-tools/src/schema.rs#L11-L17) schema_for derives the Rust field shape, so the String field does not acquire an enum restriction from its documentation comment.

**Independent challenge:** The Rust field is String, not an enum; its prose comment does not constrain serde or the schema. Dispatching unit=kelvin succeeds through the non-fahrenheit branch and labels the unchanged Tokyo Celsius value 28 as kelvin. No downstream validation can rescue a schema that itself admits every string. This is incorrect successful output, not merely loose schema style.

**Accepted correction:** Make the weather unit a serde/schemars enum with lowercase celsius/fahrenheit wire values and Celsius default, or reject unsupported values explicitly before constructing a successful response. Preserve the existing simulated weather behavior.

- [`examples/006-custom-tools-rs/main.rs:34-45,86-112`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/006-custom-tools-rs/main.rs) String deserialization accepts unsupported names; only fahrenheit converts, and the supplied name is echoed into a non-error result.
- [`crates/meerkat-core/src/types.rs:1109-1117`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/types.rs#L1109-L1117) parse_args is serde_json::from_str, not semantic weather-unit validation.
- [`crates/meerkat-tools/src/schema.rs:11-17`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-tools/src/schema.rs#L11-L17) Schema generation derives the Rust shape through tool_input_schema_for.

**Implementation (fixed):** WeatherUnit is a lowercase serde/schemars enum with Celsius default; arbitrary labels rejected without changing simulated weather.
Changed: `examples/006-custom-tools-rs/main.rs`, `examples/006-custom-tools-rs/tests.rs`, `examples/006-custom-tools-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 006 real dispatcher passes omitted/celsius/fahrenheit values, rejects kelvin/typo/case/empty values; actual emitted schema restricts the enum and leaves defaulted unit optional.

**Independent fix review (fixed):** WeatherUnit is the single typed contract for parsing, schema and serialization. Unsupported strings can no longer become successful mislabeled Celsius values.
- [`examples/006-custom-tools-rs/main.rs:35-50`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/006-custom-tools-rs/main.rs#L35-L50) Lowercase Celsius/Fahrenheit enum with Celsius default.
- [`examples/006-custom-tools-rs/main.rs:89-115`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/006-custom-tools-rs/main.rs#L89-L115) Dispatcher uses exhaustive enum conversion and serializes the typed unit.
- [`examples/006-custom-tools-rs/tests.rs:6-63`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/006-custom-tools-rs/tests.rs#L6-L63) Real dispatch covers omitted/Celsius/Fahrenheit and rejects kelvin, typo, case mismatch and empty value; actual schema is restricted.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** weather_units_default_convert_and_reject_unknown_values and weather_schema_restricts_unit_wire_values passed.

<a id="a05"></a>

### A05: Budget demo misses actual exhaustion and reports arbitrary failures as expected success

**Severity:** high. **Verdict:** confirmed. **Examples:** 009.

**Original proof:** Static current-contract proof plus existing test inspection: the core token_budget_exhausted_after_llm_call_routes_through_authority test uses limit=100 and measured usage=1000, requires Ok, and checks terminal_cause_kind=BudgetExhausted. The example's Ok branch discards that fact. Conversely any agent2 provider/store failure entering Err is printed as expected exhaustion and converted to process success.

- [`examples/009-budget-and-retry-rs/main.rs:104-121`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L104-L121) All Ok results are printed as a truncated response without checking terminal_cause_kind. Every Err is labeled Budget exhausted (expected), and main then returns Ok(()).
- [`crates/meerkat-core/src/agent/state.rs:5810-5837`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L5810-L5837) After recording usage, budget exhaustion is sent to the turn authority and returns build_result rather than an ordinary provider error.
- [`crates/meerkat-core/src/agent/state.rs:17361-17430`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L17361-L17430) Existing regression tests explicitly pin token/tool-call exhaustion to Ok with terminal_cause_kind=BudgetExhausted, not Err(TokenBudgetExceeded).
- [`examples/009-budget-and-retry-rs/README.md:6-11`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/README.md#L6-L11) The example advertises correct handling of budget exhaustion returned by the agent run.

**Independent challenge:** The current machine-owned token-budget terminal path produces a successful RunResult tagged BudgetExhausted, so matching only Err misses it. The example also converts every actual Err from agent2.run into the misleading expected-budget message and then returns success. The existing deterministic core regression agrees with the implementation. I inspected that test but did not run it.

**Accepted correction:** Recognize BudgetExhausted on successful results, report ordinary completion distinctly, and propagate unrelated agent errors. Only handle time-budget errors as expected when explicitly matching the current typed error/cause contract. Do not infer exhaustion from text or numeric usage alone.

- [`examples/009-budget-and-retry-rs/main.rs:104-121`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L104-L121) Ok ignores terminal_cause_kind; every Err is relabeled expected exhaustion and swallowed.
- [`crates/meerkat-core/src/agent/state.rs:5810-5837`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L5810-L5837) Measured budget exhaustion applies BudgetLimitExceeded and returns build_result.
- [`crates/meerkat-core/src/agent/state.rs:6919-6956`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L6919-L6956) Successful results carry the public terminal cause; hard failures return typed TerminalFailure.
- [`crates/meerkat-core/src/agent/state.rs:17361-17397`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L17361-L17397) The existing mock limit=100/usage=1000 regression specifically requires Ok plus BudgetExhausted.

**Implementation (fixed):** Budget presentation recognizes successful typed BudgetExhausted, distinguishes completion, handles typed time-budget failures only, and propagates unrelated errors.
Changed: `examples/009-budget-and-retry-rs/main.rs`, `examples/009-budget-and-retry-rs/tests.rs`, `examples/009-budget-and-retry-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 009 real-agent fake-provider tests pass normal completion, measured token exhaustion, non-retryable provider failure, zero per-turn time budget and unrelated internal error through actual describe_run.

**Independent fix review (fixed):** The exhaustion demonstration recognizes successful typed BudgetExhausted separately from normal completion, accepts only the two explicit time-budget error shapes, and returns unrelated failures unchanged.
- [`examples/009-budget-and-retry-rs/main.rs:48-73`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L48-L73) Typed result/error classification, no text matching or numeric exhaustion inference.
- [`examples/009-budget-and-retry-rs/main.rs:149-153`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L149-L153) Actual tight-budget run invokes describe_run and propagates errors.
- [`examples/009-budget-and-retry-rs/tests.rs:55-120`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/tests.rs#L55-L120) Real agent with synthetic provider exercises ordinary completion, measured overshoot returning Ok/BudgetExhausted, auth failure, and zero per-turn duration.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Normal/exhaustion, permanent-provider-failure, and typed-time-budget tests all passed. This is deterministic error propagation, not a live provider failure test.

<a id="a06"></a>

### A06: Budget documentation promises a hard cumulative token ceiling the API does not enforce

**Severity:** medium. **Verdict:** confirmed. **Examples:** 009.

**Original proof:** The existing core regression's limit=100 / usage=1000 is a direct counterexample to usage<=max_tokens. Static request preparation does not clamp output to cumulative remainder, and provider input usage also counts after completion. The test was inspected, not rerun, because parent owns Rust builds.

- [`examples/009-budget-and-retry-rs/README.md:6-18`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/README.md#L6-L18) Calls BudgetLimits hard caps and max_tokens a hard cap on cumulative token usage.
- [`examples/009-budget-and-retry-rs/main.rs:43-48`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L43-L48) The with_max_tokens comment repeats Hard cap on total tokens.
- [`crates/meerkat-core/src/agent/state.rs:5004-5010,5810-5837`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs) A request's output max is bounded by configured/model limits, not remaining cumulative token budget; measured usage is recorded after the provider call and only then checked.
- [`crates/meerkat-core/src/agent/state.rs:17361-17391`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs#L17361-L17391) The existing budget regression intentionally allows a call reporting 1000 tokens against a 100-token cumulative limit.
- [`crates/meerkat-core/src/budget.rs:13-32,37-48`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/budget.rs) max_duration is an agent-lifetime horizon, while max_turn_duration is the distinct per-run aggregate ceiling.

**Independent challenge:** The request output limit is selected from max_tokens_per_turn and the model profile, not the remaining cumulative BudgetLimits tokens. Usage is observed after the call, so the documented hard cumulative cap is false. The supplied existing 100-limit/1000-usage test is a direct permitted overshoot. max_duration also starts at Budget construction while begin_turn changes only the per-turn epoch. These are documentation distinctions; no core-budget redesign is warranted.

**Accepted correction:** Replace hard-billing-cap language with measured exhaustion/continuation-threshold language, including in-flight overshoot and dependence on measured usage. Distinguish max_tokens_per_turn output bounds, lifetime max_duration, and per-run max_turn_duration. Preserve the existing API semantics.

- [`examples/009-budget-and-retry-rs/README.md:6-18`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/README.md#L6-L18) Labels tokens a hard cumulative cap and does not distinguish lifetime from per-run timing.
- [`examples/009-budget-and-retry-rs/main.rs:44-48`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L44-L48) Repeats hard total-token cap in the teaching comment.
- [`crates/meerkat-core/src/agent/state.rs:5004-5010,5810-5837,17361-17397`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/state.rs) Per-request output limit and post-call cumulative accounting; existing regression intentionally overshoots.
- [`crates/meerkat-core/src/budget.rs:13-32,196-219,656-690`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/budget.rs) Distinct epoch mechanics and the existing begin_turn_rearms_only_the_turn_horizon regression.

**Implementation (fixed):** Replaced hard cumulative/billing-cap promises with measured exhaustion threshold and in-flight overshoot guidance. Distinguishes request output bound, agent-lifetime duration and rearmed per-run duration without changing core semantics.
Changed: `examples/009-budget-and-retry-rs/main.rs`, `examples/009-budget-and-retry-rs/tests.rs`, `examples/009-budget-and-retry-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 009 requires real measured usage above 100-token threshold plus BudgetExhausted. No hand-implemented budget algorithm.

**Independent fix review (fixed):** README and source comments accurately describe measured continuation/exhaustion thresholds and in-flight overshoot, not guaranteed billing ceilings. Per-request output, lifetime duration and rearmed per-run duration are distinguished.
- [`examples/009-budget-and-retry-rs/README.md:14-35`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/README.md#L14-L35) Threshold, missing accounting, output bound and separate time horizons are explicit.
- [`examples/009-budget-and-retry-rs/main.rs:91-96`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L91-L96) Former hard-cap comment now states measured overshoot semantics.
- [`crates/meerkat-core/src/budget.rs:17-60`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/budget.rs#L17-L60) Owning budget contract independently confirms lifetime versus per-turn horizons.
- [`examples/009-budget-and-retry-rs/tests.rs:79-98`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/tests.rs#L79-L98) Measured actual agent usage exceeds 100 while returning typed BudgetExhausted.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Deterministic measured overshoot and per-turn time-budget tests passed. Lifetime rearm semantics additionally checked against the unchanged owner source, not reimplemented in a test.

<a id="a07"></a>

### A07: The budget example can panic while truncating valid non-ASCII model output

**Severity:** medium. **Verdict:** confirmed. **Examples:** 009.

**Original proof:** Counterexample: result.text = "a".repeat(99) + "é" has 101 bytes and byte 100 is inside é. The exact Rust expression &result.text[..result.text.len().min(100)] therefore panics. An in-memory Python UTF-8 boundary probe reproduced the split (payload[:100].decode raises UnicodeDecodeError); this was not represented as execution of the Rust example.

- [`examples/009-budget-and-retry-rs/main.rs:108-112`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L108-L112) Slices a UTF-8 String at min(byte_len,100), which need not be a character boundary.

**Independent challenge:** Independently executed the exact Rust slicing expression in a std-only probe. It did not panic for empty, short ASCII, or 101-byte ASCII text; it did panic for 99 ASCII bytes followed by é and for 99 ASCII bytes followed by an emoji. Thus a successful provider result can fail in output formatting.

**Accepted correction:** Truncate at a valid character boundary or use an explicitly character-counted preview. Append the ellipsis only when truncation occurs. Keep this separate from A05's terminal classification.

- [`examples/009-budget-and-retry-rs/main.rs:108-112`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs#L108-L112) The min(100) index is a byte index without a UTF-8 boundary check; the ellipsis is unconditional.

**Implementation (fixed):** Unicode-scalar-counted preview cuts at a valid boundary and appends ellipsis only on truncation.
Changed: `examples/009-budget-and-retry-rs/main.rs`, `examples/009-budget-and-retry-rs/tests.rs`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 009 actual preview helper passes empty/short/exact/long ASCII, accent and emoji crossing cutoff, and zero cutoff. Also passed standalone extraction of the same function/test earlier.

**Independent fix review (fixed):** Preview counts Unicode scalar values and slices only at a char_indices boundary; an ellipsis appears only when content is omitted.
- [`examples/009-budget-and-retry-rs/main.rs:40-46`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L40-L46) Actual preview helper is Unicode-safe, including max_chars=0.
- [`examples/009-budget-and-retry-rs/tests.rs:123-140`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/tests.rs#L123-L140) Empty, short, exact-boundary ASCII, long text, accented and emoji cutoff cases exercise the helper.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** preview_is_unicode_safe_and_ellipsis_is_truthful passed.

<a id="a08"></a>

### A08: The first budget demo store is permanently leaked by TempDir::keep

**Severity:** low. **Verdict:** confirmed. **Examples:** 009.

**Original proof:** Static resource-lifetime proof: TempDir::keep converts the auto-cleaned temporary directory into a persistent PathBuf; no cleanup for that path exists anywhere in the 122-line source. Each successful start creates a new retained session store, including transcripts/index files after a run.

- [`examples/009-budget-and-retry-rs/main.rs:31-40,84-93`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs) The first store calls tempfile::tempdir()?.keep(), abandoning automatic cleanup and never removing or reporting the retained directory. The second store correctly retains its TempDir guard.

**Independent challenge:** The first TempDir is consumed by keep() into a PathBuf before creating sessions, so its automatic cleanup guard no longer exists. There is no explicit removal anywhere in the example. The second store deliberately retains its guard, showing cleanup is already the local pattern. 'Permanently' should mean not cleaned up by this program, not that operating-system maintenance can never remove it.

**Accepted correction:** Retain the first TempDir guard for the full lifetime of its agent/store, mirroring the second store. Do not add persistence or an ad hoc deletion path.

- [`examples/009-budget-and-retry-rs/main.rs:31-40,84-93,121`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/009-budget-and-retry-rs/main.rs) One keep(), no remove_dir/remove_dir_all or retained first guard; the second directory remains guarded.

**Implementation (fixed):** Both stores retain TempDir guards through agent lifetime; narrow scoped_store_dir helper is shared by actual setup and RAII regression.
Changed: `examples/009-budget-and-retry-rs/main.rs`, `examples/009-budget-and-retry-rs/tests.rs`, `examples/009-budget-and-retry-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 009 calls actual setup helper twice, runs deterministic agents, verifies both directory trees disappear after successful and provider-error returns.

**Independent fix review (fixed):** Both TempDir guards remain in the async body until after their stores/agents, and keep() is removed. No independent ad-hoc deletion or persistence behavior was introduced.
- [`examples/009-budget-and-retry-rs/main.rs:31-38`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L31-L38) Setup returns an owned guard plus nested store path.
- [`examples/009-budget-and-retry-rs/main.rs:80-88`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L80-L88) First guard is retained before its factory/store/agent.
- [`examples/009-budget-and-retry-rs/main.rs:132-147`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/main.rs#L132-L147) Second guard is retained in the same way.
- [`examples/009-budget-and-retry-rs/tests.rs:141-168`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/009-budget-and-retry-rs/tests.rs#L141-L168) Success and provider-error returns leave neither guarded tree behind; uses the actual setup helper.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** guarded_stores_are_cleaned_up_on_success_and_error passed under an owned current-directory root.

<a id="a09"></a>

### A09: Printed cost-tracker command neither consumes the hook payload nor returns a valid hook response

**Severity:** medium. **Verdict:** confirmed. **Examples:** 011.

**Original proof:** Executed the extracted producer without its file redirection: subprocess.run(['bash','-c','echo $HOOK_PAYLOAD'], input='{"point":"turn_boundary","turn_number":1,"session_id":"synthetic-audit"}', text=True, capture_output=True, env={'PATH':'/usr/bin:/bin'}, timeout=5). Result: exit 0, stdout='\n', not JSON. The original redirected command would append that empty line to its log and leave hook stdout empty, so the engine reports invalid command hook response. No printed /tmp path was written.

- [`examples/011-hooks-guardrails-rs/main.rs:275-285,301-303`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/main.rs) The printed background command is echo $HOOK_PAYLOAD redirected to a log, although the same reference says command hooks receive JSON on stdin and return JSON on stdout.
- [`crates/meerkat-hooks/src/lib.rs:693-725,789-809,893-900`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-hooks/src/lib.rs) The engine serializes HookInvocation, sends it on stdin, only adds explicitly configured environment entries, and requires stdout to deserialize as RuntimeHookResponse.

**Independent challenge:** The hook protocol serializes the invocation to stdin and requires JSON on stdout; it does not synthesize HOOK_PAYLOAD. I extracted and executed the actual producer with a clean environment and synthetic stdin: exit 0, stdout only newline, input ignored. In the printed full command that newline is redirected into the log and stdout is empty, so the response cannot parse. Depending on subprocess timing it may instead fail while writing to a child that never reads stdin; either way this is not a functioning observer. No forbidden printed log path was opened.

**Accepted correction:** Replace the printed cost-tracker command with a bounded stdin-consuming observer that logs appropriate synthetic/invocation data to an explicit owned path and emits a valid JSON response on stdout. Remove reliance on HOOK_PAYLOAD and the system-temporary log path.

- [`examples/011-hooks-guardrails-rs/main.rs:275-285,301-303`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/main.rs) The cost tracker contradicts the stdin/stdout protocol printed immediately below it.
- [`crates/meerkat-hooks/src/lib.rs:693-725,789-809,893-900`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-hooks/src/lib.rs) JSON invocation goes to piped stdin; only configured env additions are supplied; stdout must deserialize.
- [`crates/meerkat-hooks/src/lib.rs:99-109`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-hooks/src/lib.rs#L99-L109) RuntimeHookResponse contains an optional defaulted decision, so {} is a valid no-op observer response.

**Implementation (fixed):** Printed cost tracker runs included bounded stdin-consuming Python observer, logs identity/usage to explicit owned path and returns valid {} JSON. Uses post_llm_response for actual usage; no imaginary environment payload or system temporary log path.
Changed: `examples/011-hooks-guardrails-rs/main.rs`, `examples/011-hooks-guardrails-rs/cost_tracker.py`, `examples/011-hooks-guardrails-rs/test_cost_tracker.py`, `examples/011-hooks-guardrails-rs/tests.rs`, `examples/011-hooks-guardrails-rs/README.md`.
- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s examples/011-hooks-guardrails-rs -p 'test_*.py'`: **pass** 2 actual subprocess tests pass: stdin/log/response correctness and malformed/oversized input rejection without success response.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 011 actual printed TOML's command executes through DefaultHookEngine, has no failure_reason, logs expected invocation identity/usage and excludes assistant text. Only script path resolution and owned log destination differ.

**Independent fix review (fixed):** The documented observer now reads bounded stdin, appends only bounded invocation identity/usage to an explicit owned path, and returns valid {} JSON. The actual script is executed through DefaultHookEngine, not replaced by a toy observer.
- [`examples/011-hooks-guardrails-rs/main.rs:279-293`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/main.rs#L279-L293) post_llm_response command points to the included Python script and explicit log argument.
- [`examples/011-hooks-guardrails-rs/cost_tracker.py:8-28`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/cost_tracker.py#L8-L28) 64-KiB input bound, 8-KiB record bound, identity/usage extraction, append and valid response.
- [`examples/011-hooks-guardrails-rs/tests.rs:35-91`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/tests.rs#L35-L91) Real configured runtime invoked; checks no failure and actual logged session/turn/usage without response text.
- [`examples/011-hooks-guardrails-rs/README.md:29-43`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/README.md#L29-L43) Owned path, retained log and template prerequisites disclosed.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** documented_cost_tracker_runs_through_hook_engine passed with synthetic invocation and isolated log.
- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s examples/011-hooks-guardrails-rs -p 'test_*.py' -v`: **pass** 2 tests passed, including invalid/oversized input yielding nonzero exit, no success response and no log.

<a id="a10"></a>

### A10: Hook reference advertises a removed rewrite capability and treats eight loop hooks as the full catalog

**Severity:** low. **Verdict:** partial. **Examples:** 011.

**Original proof:** In-memory source-enum extraction counted 14 HookPoint variants and only Observe/Guardrail for HookCapability. A config capability="rewrite" cannot deserialize into that two-variant enum. No claim is made that all 14 points fire in this standalone example: the distinction is exactly what the documentation should state.

- [`examples/011-hooks-guardrails-rs/main.rs:3,10-11,287-299`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/main.rs) Prints rewrite as a retired compatibility label under Hook capabilities and claims eight defined points; the printed point list even omits run_failed.
- [`examples/011-hooks-guardrails-rs/README.md:3-9,14-22`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/README.md) Presents eight interception points as the lifecycle catalog without distinguishing the additional runtime observation hooks.
- [`crates/meerkat-core/src/hooks.rs:47-64,89-103,112-119`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/hooks.rs) HookPoint currently has 14 variants (eight loop hooks plus six runtime/peer/interaction observations), and HookCapability has only Observe and Guardrail with no rewrite deserialization alias.
- [`crates/meerkat-core/src/config.rs:2452-2466`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/config.rs#L2452-L2466) HookEntryConfig takes the closed HookCapability directly and no longer has failure-policy compatibility fields.

**Independent challenge:** The rewrite compatibility label is obsolete: HookCapability has only Observe and Guardrail and no alias. The printed catalog also accidentally omits run_failed. However, there really are eight agent-loop hooks, which is the runnable example's stated scope, and every actual printed TOML registration uses a supported capability. The example need not exercise all 14 enum variants. Treat this as reference/copy drift, not evidence that its configured in-process hooks or the eight-point loop teaching are broken.

**Accepted correction:** Remove the stale rewrite/failure-policy-compatibility teaching, restore run_failed to the printed list, and qualify the eight as agent-loop hooks with a short pointer to additional runtime observation points. Do not turn this into a requirement to add six runtime demonstrations.

- [`examples/011-hooks-guardrails-rs/main.rs:3,10-11,106-138,287-299`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/main.rs) Actual registrations are supported; the reference has seven listed points, an obsolete rewrite label and a stale failure-policy lesson.
- [`examples/011-hooks-guardrails-rs/README.md:3-22`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/011-hooks-guardrails-rs/README.md#L3-L22) README does include run_failed and enumerates the eight legitimate agent-loop points.
- [`crates/meerkat-core/src/hooks.rs:47-64,89-103,112-119`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/hooks.rs) Fourteen total points, including six observation-only points; only two capabilities.
- [`crates/meerkat-core/src/config.rs:2452-2466`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/config.rs#L2452-L2466) HookEntryConfig has no failure-policy compatibility field. Unknown struct fields need not be claimed to fail deserialization; they are simply not an active compatibility contract.

**Implementation (fixed):** Narrowed copy repair: retired rewrite/failure-policy teaching removed, run_failed restored, eight points qualified as agent-loop-only with pointer/list of six runtime observations. No requirement invented for six runtime demos.
Changed: `examples/011-hooks-guardrails-rs/main.rs`, `examples/011-hooks-guardrails-rs/tests.rs`, `examples/011-hooks-guardrails-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 011 actual printed TOML deserializes to Config; supported capability checks, eight intended loop points and negative rewrite deserialization pass.

**Independent fix review (fixed):** The two intended loop-hook lists now include run_failed and distinguish the six additional runtime observation points. Removed rewrite/failure-policy teaching is not retained as a usable capability.
- [`examples/011-hooks-guardrails-rs/main.rs:295-317`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/main.rs#L295-L317) Eight loop points, runtime-only pointer and Observe/Guardrail reference.
- [`examples/011-hooks-guardrails-rs/README.md:7-27`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/README.md#L7-L27) Matching loop list and explicit six-point runtime distinction.
- [`crates/meerkat-core/src/hooks.rs:49-64`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/meerkat-core/src/hooks.rs#L49-L64) Owner enum confirms catalog.
- [`examples/011-hooks-guardrails-rs/tests.rs:7-32`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/011-hooks-guardrails-rs/tests.rs#L7-L32) Printed config parses through Config; advertised capabilities and loop names deserialize, rewrite does not.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** printed_configuration_and_loop_catalog_are_current passed. No claim of exercising six runtime observations.

<a id="a11"></a>

### A11: The filesystem skill is malformed, stored at an unloadable path, and bound to the wrong source UUID

**Severity:** high. **Verdict:** confirmed. **Examples:** 012.

**Original proof:** Static call-chain proof: inventory_section -> composite list -> filesystem list -> parse_skill_md -> SkillName::parse("Security Auditor") fails, so the filesystem entry is skipped and only the two inline skills remain. Changing only that frontmatter name is insufficient: key auditor loads root/auditor, not root/security/auditor; changing only the path is also insufficient for a project_local key because FilesystemSkillSource::new owns builtin UUID. An in-memory slug probe confirmed Security Auditor violates the exact slug grammar.

- [`examples/012-skills-loading-rs/main.rs:132-167,175-207,317-323`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs) Writes security/auditor/SKILL.md with name: Security Auditor, creates FilesystemSkillSource::new, then separately labels it with project_local UUID. The printed file format repeats invalid name: Shell Patterns.
- [`crates/meerkat-core/src/skills/mod.rs:83-115`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/skills/mod.rs#L83-L115) SkillName accepts only lowercase ASCII/digit/dash slugs, rejecting uppercase and spaces.
- [`crates/meerkat-skills/src/parser.rs:27-36,65-81`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/parser.rs) Frontmatter name must parse as SkillName and match the directory slug.
- [`crates/meerkat-skills/src/source/filesystem.rs:29-36,71-78,111-141,146-163`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/source/filesystem.rs) new assigns builtin UUID, list derives only the last directory component and skips invalid files, and load resolves root/<skill_name>/SKILL.md rather than the nested relative path.
- [`crates/meerkat-skills/src/source/composite.rs:98-134`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/source/composite.rs#L98-L134) NamedSource labels provenance but does not rewrite the source's emitted keys or force its load method to use the named identity.

**Independent challenge:** There are three independently verified barriers to the advertised filesystem skill: invalid frontmatter slug, a nested directory that listing names auditor but load looks up at root/auditor, and the filesystem source's builtin UUID conflicting with the NamedSource project's UUID. NamedSource only supplies provenance; it does not rewrite emitted keys or the source's load identity. The printed Shell Patterns frontmatter has the same invalid-slug issue. The inline display names are not a counterexample: those documents are constructed directly, not parsed as filesystem frontmatter.

**Accepted correction:** Use a direct-child lowercase skill slug with matching frontmatter; initialize FilesystemSkillSource with the same project UUID used in its registered identity. Fix the printed SKILL.md and comments on addressing/shadowing. Do not modify source-library resolution rules to accommodate this example.

- [`examples/012-skills-loading-rs/main.rs:132-174,183-207,317-323`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs) Nested filesystem path, non-slug frontmatter, default filesystem constructor and separately assigned project identity; misleading automatic cross-source layering comment.
- [`crates/meerkat-core/src/skills/mod.rs:83-115`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/skills/mod.rs#L83-L115) SkillName's lowercase ASCII/digit/dash grammar independently rejects Security Auditor and Shell Patterns.
- [`crates/meerkat-skills/src/parser.rs:27-36,65-81`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/parser.rs) Filesystem frontmatter must satisfy SkillName and match the directory slug.
- [`crates/meerkat-skills/src/source/filesystem.rs:29-49,71-78,111-164`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/source/filesystem.rs) new chooses builtin identity; list derives the final path component; load rejects a different UUID and addresses a direct child.
- [`crates/meerkat-skills/src/source/composite.rs:98-134`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/source/composite.rs#L98-L134) Loads delegate keys unchanged and shadowing is on canonical SkillKey, not bare equal names from different UUIDs.

**Implementation (fixed):** Direct-child security-auditor directory/frontmatter share lowercase slug; filesystem source and NamedSource share project UUID. Printed shell-patterns slug fixed; full canonical-key identity replaces path/display-name shadowing claims.
Changed: `examples/012-skills-loading-rs/main.rs`, `examples/012-skills-loading-rs/tests.rs`, `examples/012-skills-loading-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 012 actual skill_engine lists exactly three canonical keys; load_from_source uses canonical source UUID, loads filesystem body; healthy invalid_count=0; printed SKILL.md parses.

**Independent fix review (fixed):** Filesystem path/frontmatter use the same direct-child slug and both filesystem source and named-source identity use project_local. Full canonical keys, rather than names/paths alone, determine identity.
- [`examples/012-skills-loading-rs/main.rs:126-163`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L126-L163) security-auditor direct child with matching lowercase name and explicit source UUID.
- [`examples/012-skills-loading-rs/main.rs:166-203`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L166-L203) NamedSource registry carries the matching UUID; comments distinguish same-name different-source skills.
- [`examples/012-skills-loading-rs/tests.rs:93-121`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/tests.rs#L93-L121) Exact three canonical keys, listed filesystem key load/body, and zero invalid healthy source.
- [`examples/012-skills-loading-rs/tests.rs:124-135`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/tests.rs#L124-L135) Exact printed shell-patterns SKILL.md parses with matching slug.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** Canonical list/load/health regression and actual printed SKILL.md parser regression passed.

<a id="a12"></a>

### A12: The skills run never supplies the model either an inventory or an activated skill body

**Severity:** high. **Verdict:** confirmed. **Examples:** 012.

**Original proof:** Precise static facade path: AgentBuilder::with_skill_engine -> skill_engine_override -> AgentFactory computes inventory, but effective_builtins=false excludes it from extra_sections. No preload_skills are present, and EmptyToolDispatcher means no skill loading tool exists. agent.run receives only the ordinary Rust-review prompt and no typed skill references. The separately printed resolved_body never enters the transcript. No model run was used to infer skill application from plausible-looking output.

- [`examples/012-skills-loading-rs/main.rs:8-16,221-257,261-284,310-315`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs) Inventory and resolved review body are printed to the host terminal only. The agent uses the default facade builder, an EmptyToolDispatcher, no preload key, and no pending per-turn skill reference, despite claiming prompt augmentation and on-demand activation.
- [`crates/meerkat/src/lib.rs:162`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/lib.rs#L162) The public AgentBuilder is the facade wrapper, not the low-level core builder; this audit followed that wrapper to avoid falsely claiming the factory is bypassed.
- [`crates/meerkat/src/agent_builder.rs:43-55,191-195,273-304`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/agent_builder.rs) with_skill_engine sets the factory override and build routes through AgentFactory; it does not itself activate a key or enable builtins.
- [`crates/meerkat/src/factory.rs:3305-3318,5827,6107-6113,6623-6644,6662-6676`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) The default factory has builtins=false; inventory is only appended when effective_builtins is true, and preloaded bodies require preload_skills. The explicit EmptyToolDispatcher is preserved as the tool override.
- [`crates/meerkat-core/src/agent/runner.rs:2439-2475`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/runner.rs#L2439-L2475) Per-turn activation consumes typed pending_skill_references; the runtime explicitly does not parse slash refs from prompt text.
- [`crates/meerkat-tools/src/dispatcher.rs:35-47`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-tools/src/dispatcher.rs#L35-L47) EmptyToolDispatcher exposes no browse/load tool that could compensate by letting the model fetch a skill.

**Independent challenge:** Independent facade tracing agrees that with_skill_engine registers an engine but does not activate anything. Default factory builtins are false, so the computed inventory is not appended. The explicit EmptyToolDispatcher survives construction, no preload keys exist, and the per-turn resolver only consumes pending typed references, which this example never sets. Printing the rendered skill to the host terminal does not send it to the LLM. This is a real failed skills demonstration, not a forbidden low-level-builder bypass.

**Accepted correction:** Activate the exact canonical review skill through a supported typed per-turn reference or factory preload. Explain registration, inventory visibility and body activation as separate steps. The minimal fix can retain EmptyToolDispatcher and use explicit per-turn activation, removing the untrue inventory/on-demand-tool claim; adding broad builtin/tool access is not required.

- [`examples/012-skills-loading-rs/main.rs:221-257,261-284`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs) Resolved content is printed, then an ordinary review prompt runs with an empty dispatcher and no activation.
- [`crates/meerkat/src/agent_builder.rs:43-55,191-195,273-304`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/agent_builder.rs) The facade builder correctly routes through AgentFactory; the skill setter only fills an override.
- [`crates/meerkat/src/factory.rs:3305-3318,5760-5765,6107-6113,6534-6540,6623-6677`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) Builtins disabled, engine override preserved, empty dispatcher preserved, no preload, inventory gated off.
- [`crates/meerkat-core/src/agent/runner.rs:2439-2505`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/agent/runner.rs#L2439-L2505) Per-turn typed references yield SkillContext blocks and SkillsResolved; ordinary slash text is not parsed here.

**Implementation (fixed):** Exact review key activated through pending_skill_references. Empty dispatcher retained; registration, host inventory printing and body activation explained separately without slash parsing or discovery-tool claims.
Changed: `examples/012-skills-loading-rs/main.rs`, `examples/012-skills-loading-rs/tests.rs`, `examples/012-skills-loading-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 012 capturing fake LLM sees actual review body in exactly one canonical SkillContext; matching SkillsResolved emitted; unrelated bodies and SkillResolutionFailed absent.

**Independent fix review (fixed):** The run explicitly activates the canonical review key using typed pending_skill_references. Registration and terminal inventory printing are no longer claimed to imply model visibility or on-demand tools.
- [`examples/012-skills-loading-rs/main.rs:237-260`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L237-L260) EmptyToolDispatcher retained, typed review activation immediately precedes run.
- [`examples/012-skills-loading-rs/main.rs:315-322`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L315-L322) Reference distinguishes typed activation, SkillsResolved, SkillContext and host inventory.
- [`examples/012-skills-loading-rs/tests.rs:157-209`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/tests.rs#L157-L209) Capturing real-agent test sees exactly the canonical review SkillContext/body and SkillsResolved, not unrelated bodies.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** typed_activation_reaches_model_with_canonical_key_and_event passed; no inference from model prose or terminal inventory.

<a id="a13"></a>

### A13: Printed skills configuration uses an obsolete top-level array and transport keys

**Severity:** medium. **Verdict:** confirmed. **Examples:** 012.

**Original proof:** Extracted the printed Config block and parsed it with Python tomllib: parsed['skills'] is a list. Current Config.skills requires the SkillsConfig table containing repositories, and each repository requires typed identity plus the type transport tag. This is a data-shape validation/static Rust-deserialization proof, not a claim that Python validates the Rust schema.

- [`examples/012-skills-loading-rs/main.rs:326-333`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs#L326-L333) Prints [[skills]], source="path"/"git", path/url, without repository names or source UUIDs.
- [`crates/meerkat-core/src/config.rs:58`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/config.rs#L58) Config.skills is a SkillsConfig structure, not Vec<...>.
- [`crates/meerkat-core/src/skills_config.rs:19-40,68-92,111-126`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/skills_config.rs) SkillsConfig has repositories; each SkillRepositoryConfig requires name and source_uuid and flattens SkillRepoTransport, tagged with type (filesystem/git/http/stdio), not source.

**Independent challenge:** Independently extracted the exact printed block and parsed it with tomllib: skills is a two-element array with source/path or source/url. Current Config expects a SkillsConfig table containing repositories, with repository name, source_uuid and a type-tagged transport. This is not a harmless descriptive label mismatch; copying that TOML supplies the wrong structural type. Python parsing established the TOML shape only, not Rust deserialization.

**Accepted correction:** Replace only the advertised configuration with valid [[skills.repositories]] entries, stable illustrative source UUIDs, names, and filesystem/git transport tags. Keep README labels explicitly descriptive and do not invoke Git/network access for this teaching block.

- [`examples/012-skills-loading-rs/main.rs:326-333`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs#L326-L333) The printed block uses obsolete [[skills]] and source keys.
- [`crates/meerkat-core/src/config.rs:58`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/config.rs#L58) Config.skills is SkillsConfig.
- [`crates/meerkat-core/src/skills_config.rs:19-40,68-126`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-core/src/skills_config.rs) repositories is the array; each entry requires name/source_uuid and flattened type-tagged transport.

**Implementation (fixed):** Printed skills.repositories entries use names, stable illustrative source UUIDs, and filesystem/git type tags. README labels explicitly descriptive; no network fetch.
Changed: `examples/012-skills-loading-rs/main.rs`, `examples/012-skills-loading-rs/tests.rs`, `examples/012-skills-loading-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 012 exact printed TOML deserializes through meerkat::Config; both repository names, UUIDs and typed transport values verified.

**Independent fix review (fixed):** The displayed configuration uses current repositories entries, stable illustrative UUIDs, source names and typed filesystem/git tags. The README labels source adapters rather than obsolete TOML source values.
- [`examples/012-skills-loading-rs/main.rs:346-358`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L346-L358) Actual printed SKILLS_CONFIG has current shape.
- [`examples/012-skills-loading-rs/tests.rs:136-154`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/tests.rs#L136-L154) Real Config deserialization asserts both repository names, UUIDs and transport variants.
- [`examples/012-skills-loading-rs/README.md:16-27`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/README.md#L16-L27) Adapter labels and no-network scope are explicit.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** printed_skill_and_configuration_parse_through_real_types passed; no Git/HTTP fetch performed.

<a id="a14"></a>

### A14: The printed shell skill preload command omits the capability it requires

**Severity:** medium. **Verdict:** partial. **Examples:** 012.

**Original proof:** Static default-config CLI path: rkat run --skill shell-patterns -> Safe -> shell=false -> available capabilities omit shell -> DefaultSkillEngine::resolve_and_render returns CapabilityUnavailable -> factory returns Failed to preload skill. No CLI invocation was made because it could initialize a real realm/auth path.

- [`examples/012-skills-loading-rs/main.rs:335-336`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs#L335-L336) Prints rkat run --skill shell-patterns without a workspace/full tools preset.
- [`crates/meerkat-tools/src/lib.rs:151-162`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-tools/src/lib.rs#L151-L162) The registered shell-patterns skill requires both builtins and shell.
- [`crates/meerkat-cli/src/main.rs:701-716,4098,4423-4443`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-cli/src/main.rs) Fresh runs default to Safe, which has shell=false; processing a bare --skill slug does not enable shell.
- [`crates/meerkat/src/factory.rs:3582-3599,6623-6642`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) The factory capability set removes Shell when disabled and treats preload resolution failure as a build error.
- [`crates/meerkat-skills/src/engine.rs:120-140`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/engine.rs#L120-L140) resolve_and_render rejects a skill with any unavailable capability.

**Independent challenge:** The core mismatch is confirmed: Safe disables shell while shell-patterns requires it. But the reported exact failure path is wrong for the current fresh CLI. CLI creates Session::new and carries it as SessionBuildOptions.resume_session=Some; the service/factory preserves that value. Factory's resume filter asks list_skills, which filters out shell-patterns, emits SkillResolutionFailed/NotFound and clears the preload before resolve_and_render. Thus the command can continue without the requested skill rather than necessarily returning Failed to preload skill. A direct non-resume factory preload does take the claimed CapabilityUnavailable error branch. Narrow the issue to an unusable advertised preload under Safe.

**Accepted correction:** Use the embedded builtin-utilities-workflow skill, whose builtins-only requirements match Safe, for the minimal least-privilege preload command; or explicitly use --tools workspace with shell-patterns and explain shell access. Do not modify CLI/factory recovery semantics as part of this example repair.

- [`examples/012-skills-loading-rs/main.rs:335-336`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/examples/012-skills-loading-rs/main.rs#L335-L336) The advertised command requests shell-patterns without a shell-capable preset.
- [`crates/meerkat-cli/src/main.rs:701-716,4098,4149-4162,11218-11232,11249-11269,11410-11437`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-cli/src/main.rs) Safe resolves shell=false; those booleans and the builtin skill key reach a build carrying the precreated Session as resume_session=Some.
- [`crates/meerkat/src/factory.rs:926-939,970-984,1014,3810-3834`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) CreateSessionRequest preserves resume_session and preload; a machine-precreated empty session is accepted without metadata.
- [`crates/meerkat-session/src/persistent.rs:10365-10388,10411-10464`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-session/src/persistent.rs) Fresh-generation materialization preserves the supplied Session; it does not convert this to a non-resume factory build.
- [`crates/meerkat-tools/src/lib.rs:151-162`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-tools/src/lib.rs#L151-L162) shell-patterns requires builtins and shell.
- [`crates/meerkat-skills/src/engine.rs:82-93,120-140,163-177`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat-skills/src/engine.rs) Direct rendering rejects missing capabilities, but list_skills filters unavailable entries.
- [`crates/meerkat/src/factory.rs:3582-3599,6534-6621`](https://github.com/lukacf/meerkat/blob/56208b9e6cee078f52c43af8f6b36660bc93eeb4/meerkat/src/factory.rs) Shell is removed from available capabilities; the resume branch drops unavailable preload keys and emits a resolution failure before rendering.

**Implementation (fixed):** Least-privilege printed command preloads builtin-utilities-workflow under Safe-equivalent builtins=true/shell=false. No CLI/factory recovery changes. Regression now uses real surface::materialize_session, build_runtime_backed_service and default_persistent_executor: shared protocol binds the precreated session, admits/stamps the first ContentTurn and commits it. No hand-authored authority metadata.
Changed: `examples/012-skills-loading-rs/main.rs`, `examples/012-skills-loading-rs/tests.rs`, `examples/012-skills-loading-rs/README.md`.
- `./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --example 001-hello-meerkat --example 005-streaming-events --example 006-custom-tools --example 009-budget-and-retry --example 011-hooks-guardrails --example 012-skills-loading --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills --no-fail-fast`: **pass** 012 extracts printed slug, materializes a precreated Session via the real surface protocol, captures LLM instructional body, exports live session to assert canonical active_skills, and asserts no SkillResolutionFailed. Runtime unregisters at end. This is deterministic shared-substrate proof, not a live CLI/provider run.

**Independent fix review (fixed):** The printed least-privilege command now selects builtin-utilities-workflow, while shell-patterns is separately disclosed as requiring shell access. The regression covers the real runtime-backed precreated-session materialization seam, not merely direct factory preload or a zero exit code.
- [`examples/012-skills-loading-rs/main.rs:330-343`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L330-L343) Printed command is identified as Safe-compatible; shell sample retains explicit shell capability.
- [`examples/012-skills-loading-rs/main.rs:360-361`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/main.rs#L360-L361) Actual command uses builtin-utilities-workflow.
- [`examples/012-skills-loading-rs/tests.rs:213-293`](https://github.com/lukacf/meerkat/blob/185182c73c554a262ac7f4f32acbc09f2963eb3b/examples/012-skills-loading-rs/tests.rs#L213-L293) Extracts exact command skill, builtins=true/shell=false, Session::new, real materialize_session/default_persistent_executor; asserts canonical active_skills, actual body and no SkillResolutionFailed, then unregisters session.
- `CARGO_BUILD_JOBS=4 ./scripts/repo-cargo test --locked -p meerkat -p meerkat-mob --examples --features meerkat/jsonl-store,meerkat/session-compaction,meerkat/memory-store-session,meerkat/skills`: **pass** documented_builtin_preload_survives_cli_precreated_session_shape passed with deterministic provider. This exercises the CLI-used materialization shape, not an actual rkat subprocess or live provider.
