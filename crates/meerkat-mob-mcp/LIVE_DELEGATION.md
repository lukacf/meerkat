# Live delegation execution placement

This composition is available with the `meerkat-mob-mcp/openai-live` feature.

The ClientContext coordinator defaults to a durable executor fork. Hosts that
want the channel-bound member itself to perform confirmed voice work opt in at
the feature composer:

```rust,ignore
use meerkat_mob_mcp::live_delegation::{
    LiveDelegationExecutionPolicy,
    compose_experimental_live_delegation_coordinator_with_policy,
};

let coordinator = compose_experimental_live_delegation_coordinator_with_policy(
    runtime,
    mobs,
    LiveDelegationExecutionPolicy::ExistingMember,
);
```

Pass this same coordinator to the live provider's channel activator/control
composition. The old composer and `ExperimentalLiveDelegationCoordinator::new`
continue to select `DurableFork`. This policy affects ClientContext delegation,
not the separate Responses/function-bridge execution contract.

## Existing-member contract

- The channel-bound source identity and session remain unchanged. No member is
  spawned, forked, wired, reconfigured, or retired. The text model, tools, and
  other member construction options remain the existing member's own policy.
- Only confirmed canonical final-user input, generated worker admission, and
  released consequential-execution authority may reach the execution service.
- Work goes through MobMachine's exact bounded SubmitWork lane with the live
  operation's stable delivery identity. Text and voice may be admitted
  concurrently; the existing session serializes queued execution normally.
  This is a normal committed source-session turn, not a noncommitting
  Responses operation; existing committed-boundary projection hooks still run.
  The real voice utterance is already canonical before execution. The separate
  execution instruction is therefore committed with typed `InjectedContext`
  provenance and the original interaction ID, never as another conversational
  user row. Its explicit instruction identifies the already committed request
  even when ordinary text has since advanced the session. It is excluded from
  semantic-memory indexing; the original voice row remains unchanged.
  Autonomous members use explicit exact runtime-input admission without
  changing their ordinary inbox mode.
- Cancellation requires generated authority for the exact live operation. It
  abandons that queued input or cancels its exact running input through
  MeerkatMachine. It never invokes member-wide interruption or retirement.
- A channel close cancels only blocked work (the member handed the item back
  to wait for another item): a delegated turn already running finishes in the
  member's session, and a request admitted before the close that has not
  started yet runs after it, on the operation's own closed channel. Its
  request row, executor input and answer commit to the member's canonical
  session as for a turn that ran while the channel was open; words the user
  spoke to continue the request while it waited join its task, and the turn
  keeps the reasoning preference the channel sealed. The close first closes
  the channel's delegation intake in MeerkatMachine, under the channel's exact
  binding: a request that reaches the host after the close began is refused
  admission, and one admitted just before whose custody lands after the close
  is cancelled.
- Terminal, result eligibility, cancellation, and release remain generated
  lifecycle decisions. For borrowed members, authorized worker retirement means
  releasing delegation custody, not retiring the underlying member.
- Member custody is persisted with the generated operation. Restart recovery
  therefore preserves existing members even when the new host uses the default
  fork policy. Recovery observes exact durable work and never resubmits it or
  manufactures provider-delivery authority.
- The runtime's typed recovery image retains ClientContext operations as well
  as Responses operations. A cold restore does not restore a live transport;
  generated reconciliation fences completed recovered work as late and
  permanently ineligible for provider delivery. Recovery passes the Mob's
  durable terminal, a completed turn, also for a turn the coordinator recorded
  as a blocked hand-back, or as failed or cancelled with its work item: the
  recorded terminal settles the operation, as the close settles it, so a
  blocked hand-back is cancelled with its work item. A work item names its
  operation in its evidence, by which recovery finds it.
- Restart recovery never wakes a member. A member session the runtime holds no
  registration for at start (a member left dormant by lazy restore, an
  identity whose first wake is deferred, a released materialization) is not
  read: its operations stay in the session's durable machine state and are
  captured once, when the runtime registers the session again (the member's
  wake) or when the first live channel is prepared on it, whichever comes
  first, and then reconciled like any other. Live channel preparation waits
  for the start-up capture of the registered sessions for at most 30 seconds.
- Existing-member execution requires a local runtime-backed member and rejects
  member construction overrides. Unsupported admission fails explicitly; there
  is no fallback fork or direct `internal_turn` path.
