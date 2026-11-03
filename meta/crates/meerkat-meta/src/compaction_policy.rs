//! Host-level session-compaction policy — Meta's single owner of the
//! `meerkat::Config.compaction` slot.
//!
//! # Why this module exists
//!
//! Every Meta session-build path ends at
//! `meerkat::FactoryAgentBuilder::new(factory, config)`, and meerkat's
//! `AgentFactory::build_agent` unconditionally installs a
//! `meerkat_session::DefaultCompactor` built from `config.compaction`
//! (`meerkat/src/factory.rs`, `#[cfg(feature = "session-compaction")]` — the
//! feature is unified on in every Meta build because `meerkat-mob` declares
//! `meerkat = { features = [..., "session-compaction", ...] }`). A compactor
//! is therefore always present on Meta's built sessions.
//!
//! What was missing is the *policy*. Every Meta host handed meerkat a bare
//! `Config::default()`, and `Config::default()` leaves
//! `auto_compact_threshold_explicit = false`. That flag is load-bearing:
//! when it is false and the threshold still equals meerkat's documented
//! `100_000` default, `model_aware_compaction_config` replaces the number with
//! `context_window * 4 / 5` for the session's model. On a million-token model
//! (`gpt-5.6-sol` is catalogued at `1_050_000`) the effective trigger becomes
//! `840_000` tokens — compaction never fires in practice, the full transcript
//! is resent every turn, and turn latency grows without bound.
//!
//! Declaring a threshold through this module sets
//! `auto_compact_threshold_explicit`, which pins the number and disables that
//! scaling.
//!
//! # Precedence
//!
//! A mob profile's `auto_compact_threshold` still wins over a host-level
//! declaration: meerkat maps it to
//! `SessionBuildOptions::auto_compact_threshold_override`, which
//! `model_aware_compaction_config` consults before anything else. Strongest
//! first:
//!
//! 1. profile `auto_compact_threshold` (per member),
//! 2. this host-level policy (per runtime / per gateway),
//! 3. meerkat's model-aware default (`context_window * 4 / 5`).
//!
//! # Vocabulary
//!
//! There is deliberately no Meta-owned config type here. The declaration IS
//! [`meerkat_core::config::CompactionRuntimeConfig`] — the same type
//! `meerkat::Config.compaction` holds, with the same serde contract (a key
//! that is present is an explicit pin; a key that is absent inherits).

use meerkat_core::Config;
use meerkat_core::config::CompactionRuntimeConfig;

/// Transcript-history retention for every session Meta builds, unless the
/// host declares another value.
///
/// Core keeps the bodies of the newest `history_retained_rewrites` transcript
/// rewrites (default 4) and retires older ones after each compaction. Core
/// 0.9.4 refused the intra-turn checkpoint of a turn that compacted more than
/// `history_retained_rewrites` times ("failed to prove pending rewrite
/// suffix") and abandoned the turn, and this wider window kept long single
/// turns inside it. Core 0.9.5 removes that refusal: such a turn completes,
/// and its run keeps at most one rewrite beyond the retention in memory. Meta
/// keeps 64 as a retention choice, not for correctness.
pub const META_HISTORY_RETAINED_REWRITES: usize = 64;

/// Raise Core's default transcript retention to
/// [`META_HISTORY_RETAINED_REWRITES`].
///
/// `CompactionRuntimeConfig` cannot tell an inherited default from a
/// declared one, so Core's default value counts as unset here; any other
/// declared value is kept.
#[must_use]
pub fn with_meta_compaction_defaults(mut config: Config) -> Config {
    if config.compaction.history_retained_rewrites
        == meerkat_core::TranscriptHistoryRetention::DEFAULT_RETAINED_REWRITES
    {
        config.compaction.history_retained_rewrites = META_HISTORY_RETAINED_REWRITES;
    }
    config
}

/// Keys a host-level compaction declaration may carry.
///
/// Closed on purpose: `CompactionRuntimeConfig`'s own deserializer ignores
/// unknown keys, so a typo would silently inherit the model-aware default —
/// the exact failure mode this module exists to remove. Meta's config
/// surfaces reject the typo instead.
pub const COMPACTION_POLICY_KEYS: [&str; 5] = [
    "auto_compact_threshold",
    "recent_turn_budget",
    "max_summary_tokens",
    "min_turns_between_compactions",
    "history_retained_rewrites",
];

/// Parse a JSON host-level compaction declaration.
///
/// Failures are returned field-relative (`"must be a JSON object"`,
/// `"auto_compact_threshold must be greater than 0"`, ...) so each config
/// surface can prefix them with the path the operator actually typed. Absent
/// keys inherit meerkat's defaults; a present `auto_compact_threshold` pins
/// the threshold against model-aware scaling.
///
/// # Errors
///
/// Fails when the value is not a JSON object, carries a key outside
/// [`COMPACTION_POLICY_KEYS`], has a field of the wrong type, or declares a
/// threshold meerkat's own `Config::validate` would reject.
pub fn parse_compaction_policy(
    value: &serde_json::Value,
) -> Result<CompactionRuntimeConfig, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "must be a JSON object".to_string())?;
    let unsupported = object
        .keys()
        .filter(|key| !COMPACTION_POLICY_KEYS.contains(&key.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        return Err(format!(
            "carries unsupported fields: {}",
            unsupported.join(", ")
        ));
    }
    let policy: CompactionRuntimeConfig =
        serde_json::from_value(value.clone()).map_err(|error| format!("is invalid: {error}"))?;
    validate_compaction_policy(&policy)?;
    Ok(policy)
}

/// Reject a declaration meerkat's own `Config::validate` would reject.
///
/// Meta's hosts never call `Config::validate` (they compose a `Config` in
/// code rather than loading one), so the one compaction invariant meerkat
/// states — a non-zero threshold — is enforced here instead of being
/// discovered as a compaction storm at runtime.
///
/// # Errors
///
/// Fails when `auto_compact_threshold` or `history_retained_rewrites` is
/// zero.
pub fn validate_compaction_policy(policy: &CompactionRuntimeConfig) -> Result<(), String> {
    if policy.auto_compact_threshold == 0 {
        return Err("auto_compact_threshold must be greater than 0".to_string());
    }
    if policy.history_retained_rewrites == 0 {
        return Err("history_retained_rewrites must be greater than 0".to_string());
    }
    Ok(())
}

/// Install `policy` as the host-level compaction policy on `config`.
///
/// The resulting `config` is what Meta hands to
/// `meerkat::FactoryAgentBuilder`, so this is the moment the policy becomes
/// the compactor every session on that host is built with.
///
/// # Errors
///
/// Fails when [`validate_compaction_policy`] rejects the declaration; `config`
/// is left untouched in that case.
pub fn apply_compaction_policy(
    config: &mut Config,
    policy: &CompactionRuntimeConfig,
) -> Result<(), String> {
    validate_compaction_policy(policy)?;
    tracing::info!(
        auto_compact_threshold = policy.auto_compact_threshold,
        threshold_pinned = policy.auto_compact_threshold_explicit,
        recent_turn_budget = policy.recent_turn_budget,
        max_summary_tokens = policy.max_summary_tokens,
        min_turns_between_compactions = policy.min_turns_between_compactions,
        history_retained_rewrites = policy.history_retained_rewrites,
        "host compaction policy applied to the session-build config"
    );
    config.compaction = policy.clone();
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A declared window is kept, Core's default value counts as unset and
    /// becomes Meta's, and zero is refused like Core's own validation does.
    #[test]
    fn history_retention_is_declared_defaulted_and_never_zero() {
        let declared = parse_compaction_policy(&json!({ "history_retained_rewrites": 9 }))
            .expect("a declared window parses");
        assert_eq!(declared.history_retained_rewrites, 9);
        let zero = parse_compaction_policy(&json!({ "history_retained_rewrites": 0 }))
            .expect_err("a zero window is refused");
        assert!(
            zero.contains("history_retained_rewrites must be greater than 0"),
            "{zero}"
        );
        assert_eq!(
            with_meta_compaction_defaults(Config::default())
                .compaction
                .history_retained_rewrites,
            META_HISTORY_RETAINED_REWRITES
        );
        let mut config = Config::default();
        config.compaction.history_retained_rewrites = 9;
        assert_eq!(
            with_meta_compaction_defaults(config)
                .compaction
                .history_retained_rewrites,
            9
        );
        // The default never pins the threshold.
        assert!(
            !with_meta_compaction_defaults(Config::default())
                .compaction
                .auto_compact_threshold_explicit
        );
    }

    /// A declared threshold must set the explicit flag — that flag is the
    /// only thing standing between the declared number and meerkat's
    /// `context_window * 4 / 5` rewrite.
    #[test]
    fn declared_threshold_pins_against_model_aware_scaling() {
        let policy =
            parse_compaction_policy(&json!({ "auto_compact_threshold": 120_000 })).expect("parses");
        assert_eq!(policy.auto_compact_threshold, 120_000);
        assert!(
            policy.auto_compact_threshold_explicit,
            "a declared threshold must be explicit or meerkat rescales it to the model window",
        );

        let mut config = Config::default();
        assert!(
            !config.compaction.auto_compact_threshold_explicit,
            "the un-configured baseline is the inheriting form",
        );
        apply_compaction_policy(&mut config, &policy).expect("applies");
        assert_eq!(config.compaction.auto_compact_threshold, 120_000);
        assert!(config.compaction.auto_compact_threshold_explicit);
    }

    /// Tuning a non-threshold knob must NOT pin the threshold: the operator
    /// asked for a different retention budget, not for a fixed trigger.
    #[test]
    fn omitted_threshold_keeps_inheriting() {
        let policy = parse_compaction_policy(&json!({ "recent_turn_budget": 8 })).expect("parses");
        assert_eq!(policy.recent_turn_budget, 8);
        assert!(!policy.auto_compact_threshold_explicit);
        assert_eq!(
            policy.auto_compact_threshold,
            CompactionRuntimeConfig::default().auto_compact_threshold,
        );
    }

    #[test]
    fn zero_threshold_is_refused() {
        let error = parse_compaction_policy(&json!({ "auto_compact_threshold": 0 }))
            .expect_err("zero must fail closed");
        assert!(error.contains("greater than 0"), "{error}");
    }

    /// A typo must be a startup error. `CompactionRuntimeConfig`'s own
    /// deserializer would ignore it and silently inherit the model-aware
    /// default — a dead knob that reads as configured.
    #[test]
    fn unknown_keys_are_refused() {
        let error = parse_compaction_policy(&json!({ "auto_compact_treshold": 100 }))
            .expect_err("typos must fail closed");
        assert!(error.contains("auto_compact_treshold"), "{error}");

        let error = parse_compaction_policy(&json!("100000"))
            .expect_err("a scalar is not a compaction declaration");
        assert!(error.contains("JSON object"), "{error}");
    }

    #[test]
    fn wrong_field_type_is_refused() {
        let error = parse_compaction_policy(&json!({ "auto_compact_threshold": "lots" }))
            .expect_err("a string threshold must fail closed");
        assert!(error.contains("is invalid"), "{error}");
    }

    #[test]
    fn every_field_round_trips() {
        let policy = parse_compaction_policy(&json!({
            "auto_compact_threshold": 90_000,
            "recent_turn_budget": 6,
            "max_summary_tokens": 8192,
            "min_turns_between_compactions": 5,
        }))
        .expect("parses");
        assert_eq!(policy.auto_compact_threshold, 90_000);
        assert_eq!(policy.recent_turn_budget, 6);
        assert_eq!(policy.max_summary_tokens, 8192);
        assert_eq!(policy.min_turns_between_compactions, 5);
    }
}
