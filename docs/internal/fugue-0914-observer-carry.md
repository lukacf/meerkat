# Fugue's custom 0.9.14 runtime

This branch is based on the released `v0.9.14` commit
`abdabeb8e97805385e71d66da7e78136c4b0e68c`. It carries the native flow run
observer feature from `a3db0ef2bc6d4787878b5e6a7e0057323eb42d80` for Fugue.
It is a custom source revision, not the published 0.9.14 runtime.

The feature admits a keyed flow with explicit role targets and a bound
observer, permits that member to read only its observed run, and delivers a
terminal wake through the native continuation owner. It preserves the
observer's retained-work identity and replay key. It does not grant manage
scope or replace native completion with application polling.

The previous Fugue runtime, `74e62b9c7c46b2b8fc0b3b8ace576fd3f9a28838`,
already wrote mob schema 13 with `mob_run_observers`. This carry retains the
same migration and table, the exact version 12 predecessor verifier, and
the existing observer serialization. The fixture
`crates/meerkat-mob/tests/fixtures/fugue-74e62b9c-mob-v13.sql` freezes that
old source's initializer and event-route index. Its reopen test checks that
an existing observer row remains byte-identical across two opens.

Published 0.9.14 supports mob schema 12 and refuses these schema 13 stores.
It also cannot consume the carried `MobFlowRun` continuation producer.
A rollback therefore needs a compatible runtime or the complete pre-upgrade
backup. Do not drop the observer table, rewrite the ledger, or reset state.
Before a later Core upgrade, check whether upstream migration 13 adopts this
same schema or requires an explicit adoption step; its number alone is not
proof of compatibility.

The peer-lifecycle prompt change from the old custom branch is already in
0.9.14 and is not reapplied here. The runtime changes introduced since 0.9.5
remain in place. Machine inputs and transitions are appended after the
0.9.14 definitions, and the generated artifacts are regenerated from that
combined source.

Observer binding is a host operation requiring `SendCommand`. It refuses
the agent lane and closed admission: binding an already-terminal run can
wake the observer even though it does not start another flow run.

Source provenance and local tests do not establish deployment
acceptance. Fugue's release records must identify the exact carried revision,
the actual bundle, and the results of its installation and recovery checks.
