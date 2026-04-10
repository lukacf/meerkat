# Two-Kernel Research

Working folder for architecture research on collapsing the current machine surface into a smaller number of closed semantic kernels.

Current working direction:

- `MeerkatMachine`: single-session interactive runtime kernel
- `MobMachine`: multi-agent orchestration kernel

Likely artifacts to keep here:

- top-level machine milestone notes (`M1 = MeerkatMachine`, `M2 = MobMachine`)
- machine-by-machine collapse matrix
- cutover gate and semantic-freeze checklist
- Meerkat cutover checklist
- Meerkat interrupt freeze note
- Meerkat detached-wake freeze note
- Meerkat turn / ops / barrier freeze note
- Meerkat peer-ingress freeze note
- Meerkat tool-surface freeze note
- Meerkat drain / keep-alive freeze note
- Meerkat input/effect alphabet
- Meerkat lowering map
- Meerkat ownership decisions
- top-level exact-current MeerkatMachine freeze note
- top-level target-state MeerkatMachine freeze note
- Meerkat proof-obligations handoff for TLA+
- Meerkat transition catalog for target-state proof work
- Meerkat state schema and canonical initial state
- Meerkat derived predicates for target-state proof work
- Meerkat coverage matrix for target alphabet and regions
- Meerkat glossary for frozen target terminology
- Meerkat fairness assumptions for target-state liveness work
- top-level exact-current MobMachine freeze note
- top-level target-state MobMachine freeze note
- Mob input/effect alphabet
- Mob lowering map
- Mob ownership decisions
- Mob cutover checklist
- Mob proof-obligations handoff for TLA+
- Mob final proof handoff and audit envelope
- Mob final freeze closeout
- Mob final package audit
- Mob target self-containment audit
- Mob final traceability audit
- Mob proof-coverage handoff for TLA+
- Mob effect-coverage handoff for target effects
- Mob flow-family coverage handoff for target flows
- Mob refinement map from exact-current snapshot to target regions
- Mob refinement-delta handoff against exact-current behavior
- Mob transition catalog for target-state proof work
- Mob state schema and canonical initial state
- Mob derived predicates for target-state proof work
- Mob coverage matrix for target alphabet and regions
- Mob glossary for frozen target terminology
- Mob fairness assumptions for target-state liveness work
- experimental target-state TLA+ scaffold and bounded TLC configs
- implementation progress log
- Meerkat kernel shape
- Meerkat owned-facts ledger
- Meerkat internal state machine sketch
- identity-native abstract member contract between Meerkat and Mob
- owned-facts ledger
- bridge alphabet
- generation / epoch mapping
- boundary and perimeter notes
- proof-surface reduction sketches

Current assumption:

- scheduling remains outside the present kernel boundary unless we explicitly decide to promote it into a separate third kernel

Current freeze status:

- `M1 = MeerkatMachine` has a frozen target package plus bounded TLC base and
  stress passes
- `M2 = MobMachine` now also has a frozen target package plus bounded TLC base
  and stress passes, including flow dependency-ready dispatch, explicit
  work/step coupling, explicit quorum contribution state, terminal run work
  cleanup, task/run terminal-binding cleanup, per-identity history alignment,
  explicit per-step dispatch-mode state, a closed target-state cutover
  checklist, explicit proof / effect / refinement coverage handoffs, and an
  explicit proof handoff that distinguishes canonical bounded passes from a
  wider exploratory audit envelope
