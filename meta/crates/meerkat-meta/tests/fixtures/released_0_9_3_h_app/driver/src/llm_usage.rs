//! Normalized provider token accounting for Meta's LLM test doubles.
//!
//! # Why this file exists
//!
//! meerkat 0.8.22 split provider usage into typed per-turn and cumulative
//! carriers. `Agent::commit_calling_llm_response` now runs
//! `TurnUsage::try_from_usage(result.usage)` and fails the turn closed with
//! `normalized_provider_accounting_unavailable` when the stream carried no
//! `LlmEvent::UsageUpdate`, then runs `validate_provider_turn_usage_identity`
//! and fails closed with `normalized_provider_accounting_identity_mismatch`
//! when the accounting's `(provider, model)` is not the ACTIVE session
//! identity. The CHANGELOG states it plainly: "Custom adapters must emit
//! matching provider/model accounting or the turn fails closed."
//!
//! `LlmStreamResult::new` still accepts a flat `Usage`, and `LlmEvent::Done`
//! is still constructible on its own, so a test double that omits usage
//! COMPILES CLEANLY and then fails on every single agent turn it drives. That
//! is exactly the defect meerkat hit in its own fixtures, and it recurred here
//! at scale.
//!
//! # The contract this file exists to make unbreakable
//!
//! Do not hand-roll `LlmEvent::Done` in a Meta test double. Call
//! [`usage_then_done`] (or [`usage_then_done_with`]) and yield BOTH returned
//! events. The helper cannot produce a `Done` without the `UsageUpdate` that
//! precedes it, so a double built through it cannot regress the way 28 of
//! Meta's doubles regressed at once. A helper that cannot emit `Done`
//! without usage is a contract; 28 correct copies is a convention, and the
//! 29th copy breaks it.
//!
//! # Why the provider is derived from the model and not from `provider()`
//!
//! `LlmClient::provider()` is NOT the identity the check compares against.
//! Meta installs the mob-wide default client through
//! `ProviderAgnosticLlmClient`, which reports `Provider::Other` precisely so
//! one stub can serve members across providers
//! (`src/mob_handle_runtime.rs`). `AgentFactory` therefore skips its
//! raw-override provider guard (`Other` is meerkat's typed "serves any
//! provider" claim) and binds `LlmClientAdapter` to the CANONICAL provider
//! resolved from the model. A double that hardcoded `Provider::OpenAI` while
//! running on a `claude-*` profile would bind fine and then fail the identity
//! check at commit time.
//!
//! So the canonical answer comes from the same authority the factory used:
//! `meerkat_models::infer_provider(&request.model)`. `client_declared` is only
//! the fallback for UNCATALOGUED models, where `AgentFactory`'s own last
//! resort is likewise `llm_client_override.provider()`
//! (`meerkat/src/factory.rs`, `resolve_provider_from_registry`).
//!
//! Included from both integration tests (`#[path = "support/llm_usage.rs"]`)
//! and the crate's own `#[cfg(test)]` doubles (via
//! `src/mob_handle_runtime.rs`), so there is exactly ONE definition of this
//! contract in the repo.
//!
//! # The same rule for meerkat's own `TestClient`
//!
//! Meta's hand-written doubles are not the only offenders.
//! `meerkat_client::TestClient::default()` DOES backfill a missing
//! `UsageUpdate` (`synthesize_usage`), but it declares
//! `Provider::Other` - so it clears `normalized_provider_accounting_unavailable`
//! and then trips `normalized_provider_accounting_identity_mismatch` the
//! moment the profile model is CATALOGUED. Every Meta profile is
//! `model = "gpt-5.5"`, whose canonical owner is `Provider::OpenAI`, so the
//! adapter is bound to `OpenAI` while the synthesized accounting says
//! `Other`. That is why meerkat 0.8.22 added
//! `TestClient::for_provider(provider)` and why its own turn-driving fixtures
//! (`meerkat-mob/tests/support/mod.rs`) use it.
//!
//! Rule for Meta: a `TestClient` installed as `default_llm_client` (or any
//! other turn-driving seam) must be `TestClient::for_provider(P)` where `P`
//! is the CANONICAL owner of the profile's model - `Provider::OpenAI` for
//! `gpt-5.5`. Keep `TestClient::default()` ONLY where the profile model is
//! UNCATALOGUED with no `[models.<id>]` entry, because there the canonical
//! provider itself resolves to `Other` and `default()` is the matching claim.
//! `TestClient::new(events)` leaves `synthesize_usage` OFF entirely: those
//! call sites must carry an explicit `UsageUpdate` built from [`turn_usage`].

#![allow(dead_code)]

use meerkat_client::{LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::{Provider, StopReason, TurnUsage, Usage};

/// The `(provider, model)` identity `validate_provider_turn_usage_identity`
/// will compare the emitted accounting against.
///
/// Catalogued models answer from the canonical catalog - the same authority
/// `AgentFactory::resolve_provider_from_registry` consults first. Uncatalogued
/// models fall back to the client's own declaration, which is what the factory
/// falls back to as well.
pub fn accounting_provider(model: &str, client_declared: Provider) -> Provider {
    meerkat_models::infer_provider(model).unwrap_or(client_declared)
}

/// Normalized per-turn accounting for one scripted turn.
///
/// `host_declared` is the honest convention for a test double: it declares
/// `usage.input_tokens` as an inclusive presented-input total rather than
/// pretending to reconstruct a provider's disjoint cache components.
pub fn turn_usage(model: &str, client_declared: Provider, usage: Usage) -> TurnUsage {
    TurnUsage::host_declared(accounting_provider(model, client_declared), model, usage)
}

/// The `UsageUpdate` event for a scripted turn that reports no token counts.
pub fn usage_event(request: &LlmRequest, client_declared: Provider) -> LlmEvent {
    usage_event_with(request, client_declared, Usage::default())
}

/// The `UsageUpdate` event for a scripted turn that reports real token counts.
///
/// Use this whenever the test asserts on anything downstream of
/// `presented_tokens` - budget, `last_input_tokens`, or the compaction
/// trigger. `Usage::default()` reports zero presented tokens, which silently
/// disarms those assertions.
pub fn usage_event_with(request: &LlmRequest, client_declared: Provider, usage: Usage) -> LlmEvent {
    LlmEvent::UsageUpdate {
        usage: turn_usage(&request.model, client_declared, usage),
    }
}

/// The terminal pair every scripted success path must yield, in order.
///
/// Yield BOTH, `[0]` then `[1]`. This is the whole point of the helper: there
/// is no way to get the `Done` out of it without the `UsageUpdate`.
pub fn usage_then_done(
    request: &LlmRequest,
    client_declared: Provider,
    stop_reason: StopReason,
) -> [LlmEvent; 2] {
    usage_then_done_with(request, client_declared, Usage::default(), stop_reason)
}

/// [`usage_then_done`] for a turn that must report real token counts.
pub fn usage_then_done_with(
    request: &LlmRequest,
    client_declared: Provider,
    usage: Usage,
    stop_reason: StopReason,
) -> [LlmEvent; 2] {
    [
        usage_event_with(request, client_declared, usage),
        LlmEvent::Done {
            outcome: LlmDoneOutcome::Success { stop_reason },
        },
    ]
}

/// Normalized accounting for a double that implements `AgentLlmClient`
/// directly and returns an `LlmStreamResult` instead of a wire event stream.
///
/// A different rule applies at this seam, deliberately. `AgentFactory` guards
/// `agent_llm_client_override` with an EXACT identity equality
/// (`client.provider() == provider && client.model() == build_config.model`),
/// so an agent-level double's own `provider()`/`model()` are guaranteed to be
/// the canonical identity - pass them straight in. There is no
/// `Provider::Other` escape hatch at this seam, so nothing to infer.
pub fn agent_turn_usage(provider: Provider, model: &str, usage: Usage) -> Usage {
    TurnUsage::host_declared(provider, model, usage).into_inner()
}
