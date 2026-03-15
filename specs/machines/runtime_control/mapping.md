# RuntimeControlMachine Mapping Note

## Rust implementation anchors

Primary anchors in today's code:

- `meerkat-runtime/src/runtime_state.rs`
- `meerkat-runtime/src/state_machine.rs`
- `meerkat-runtime/src/runtime_loop.rs`
- `meerkat-runtime/src/session_adapter.rs`
- `meerkat-runtime/src/traits.rs`
- `meerkat-runtime/src/policy_table.rs`

## What the formal model abstracts away

- concrete input payload content
- concrete queue contents and per-input ledger internals
- store I/O
- transport/channel implementation details
- executor internals below `RunEvent`

The model keeps:

- runtime lifecycle state
- run ownership
- wake/process scheduling intent
- admission/control sequencing
- preemption rules

## Where current code is only a precursor

1. runtime control and ingress still share an implementation owner

Today, `RuntimeDriver` plus `RuntimeSessionAdapter` embody both control and
ingress concerns. The formal split is architectural: `RuntimeControlMachine`
owns lifecycle/control/admission coordination, while `RuntimeIngressMachine`
owns the admitted queue/ledger.

2. the canonical runtime path already exists, but legacy bypasses still do too

`RuntimeLoop` is the intended owner of idle/wake/dequeue/apply coordination, but
`meerkat-core` still has a legacy host-mode loop outside this path. The formal
machine contract describes the converged `0.5` path.

3. control-plane precedence is clearer in the contract than in current code

Current runtime loop handles control on a separate channel via `tokio::select!`.
The formal contract makes that precedence explicit: control is out-of-band and
may preempt ordinary work scheduling.

## Proof vs test split

Best candidates for model-checked properties:

- runtime-state transition legality
- run-id ownership invariants
- retired/stopped/destroyed admission rules
- control-plane precedence over ordinary work

Best candidates for Rust-side tests:

- channel closure behavior
- executor error propagation
- completion registry resolution on shutdown
- exact `RuntimeSessionAdapter` surface semantics
