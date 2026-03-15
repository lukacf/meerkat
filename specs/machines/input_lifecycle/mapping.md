# InputLifecycleMachine Mapping Note

This note maps the normative `0.5` `InputLifecycleMachine` contract onto
current `0.4` anchors.

## Rust anchors

- lifecycle state/value types:
  - `meerkat-runtime/src/input_state.rs`
- lifecycle transition validator:
  - `meerkat-runtime/src/input_machine.rs`
- runtime-wide owner/context:
  - `meerkat-runtime/src/input_ledger.rs`
  - `meerkat-runtime/src/driver/ephemeral.rs`
  - `meerkat-runtime/src/driver/persistent.rs`

## What is already aligned

- the lifecycle state set already exists in `InputLifecycleState`
- terminal states are already explicit and closed
- `InputStateMachine` already rejects all terminal-to-anything transitions
- `AppliedPendingConsumption -> Queued` is already a hard forbidden transition
- terminal outcomes are already explicit and typed

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- payload persistence fields on `InputState`
- timestamps and history-entry contents
- policy snapshot and durability metadata
- reconstruction sources for derived inputs
- crash-recovery store receipts

Those are important implementation details, but they refine the same lifecycle
contract rather than changing the machine semantics.

## Important `0.5` framing

The current `InputStateMachine` struct is reducer/validator-shaped.

That is not a contradiction with the `0.5` term `InputLifecycleMachine`.
The named machine is the owned lifecycle concern as a whole:

- `InputState` is the authoritative lifecycle record
- `InputStateMachine` is the validator/transition helper
- `InputLedger` and runtime drivers are the runtime owners around it

The formal contract names the machine boundary first, then maps current code
onto that boundary.

## Known `0.4` divergence

- queue ownership and lifecycle ownership are still partly folded together in
  runtime driver code
- recovery semantics are split across driver/store logic rather than living in
  one explicit lifecycle owner
- supersede/coalesce queue semantics exist in policy/runtime helpers but are
  not yet uniformly surfaced as first-class lifecycle operations everywhere

