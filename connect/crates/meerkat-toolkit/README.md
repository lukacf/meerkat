# Application contracts

This crate currently implements host-attested authority and typed service/frontend readiness checks. It is not yet the complete optional MobKit setup feature. The full target remains [revision 8](../../docs/spec/meerkat-toolkit-capability-acquisition.md).

`validate_request` preserves the original request, supplied context and labeled caller observations without selecting packages or roles. Its model-facing JSON has no authority or routing fields. The authenticated host supplies the target, correlation, original-task continuation, environment and limits; current owner checks gate exact context references. Separate correlation and full-input digests permit a future atomic admission owner to distinguish exact retries from changed requests. Validation alone does not create an operation, spawn agents or persist a decision. Referenced content must be checked again before a helper receives it, and request serialization belongs in access-controlled storage rather than logs or a public UI.

`AuthorityIssuer` is a trusted local-host HMAC issuer. Signed ceilings retain exact action/resource and service operation/resource relationships, frontend participant/control rights and permitted role lifetimes. Each receiver verifies the sealed envelope using current host context. A cached verified signature still requires current host policy and receiver context at every enforcement boundary. This local mechanism does not implement the Meerkat cross-mob issuer/delegation protocol.

`validate_proposal` checks a requested binding against the authenticated setup ceiling. A policy owner must still grant it. `verify_readiness` obtains current grants and runtime/interaction receipts and validates their complete intent/grant digests. A service receipt cannot establish frontend readiness, and partial results stay partial.

Role lifetime and setup-request validity are separate. A short setup task can be authorized to establish a member- or deployment-scoped role. `observe_readiness` queries current owner evidence after setup ends; it cannot create roles or authorize use, and its caller must enforce management/status visibility. Every subsequent service operation and frontend interaction requires its own current authorization.

The owner traits are trusted composition ports. Only adapters to actual policy and runtime owners can establish live grants, account identity, incarnation/catalog freshness and audience-side readiness. Test fixtures demonstrate validator behavior, not live owner conformance.

Run `./scripts/repo-cargo test --manifest-path native-candidate/Cargo.toml -p meerkat-toolkit` from the repository root. This explicitly selects the [native candidate workspace](../../native-candidate/README.md) and requires its retained development prerequisites; it does not certify a released native integration. Review findings and their dispositions are in `docs/reviews/`.
