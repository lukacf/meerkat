//! Wave-c C-12 — the single canonical CLI-boundary parser that lifts
//! user-supplied `realm:binding[:profile]` input into a typed
//! [`ConnectionRef`].
//!
//! `meerkat_core::connection::ConnectionRef` is purely structural — by
//! wave-b design it carries no `parse` / `Display` impl that would let
//! an opaque string form ferry through the runtime. CLI input arrives
//! as the flat colon-delimited form; this module owns the entire
//! conversion from that flat form into the typed record
//! `{ realm: RealmId, binding: BindingId, profile: Option<ProfileId> }`.
//!
//! The tripwire at `meerkat-cli/tests/connection_ref_single_parser.rs`
//! enforces that exactly one `fn parse_connection_ref_user_input` lives
//! under `meerkat-cli/src/`. That is the structural guard against
//! ad-hoc colon-split parsing drifting back into handler code.
//!
//! ### Scope
//!
//! * `main.rs` still parses [`meerkat_providers::auth_store::TokenKey`]
//!   values via `split_once(':')` inside `interactive_logout`. That is
//!   NOT a ConnectionRef parse — `TokenKey` is the on-disk token key
//!   used by OAuth storage, which happens to share the same flat
//!   syntax but has different semantics (no profile component, no
//!   typed newtype validation requirement at that call site).
//! * `mcp.rs` splits HTTP header strings via `splitn(2, ':')`. That is
//!   NOT a ConnectionRef parse either.
//!
//! Both carve-outs are called out explicitly so future refactors don't
//! mistake them for drift and collapse them into this parser.

use meerkat_core::connection::{BindingId, ConnectionRef, IdentityError, ProfileId, RealmId};
use thiserror::Error;

/// Typed error returned by [`parse_connection_ref_user_input`].
///
/// Every variant carries the offending input (or component) so CLI
/// error reporting can show the user what went wrong without a
/// re-parse. `thiserror` keeps the message shape stable for snapshot
/// tests and docs.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CliError {
    /// The raw string had no colons at all. A ConnectionRef always
    /// needs at least `realm:binding`.
    #[error(
        "--connection-ref requires `realm:binding[:profile]`; got `{raw}` with no `:` separator"
    )]
    MissingBinding { raw: String },

    /// More than two colons in the input: realm:binding:profile:extra
    /// is not a valid form.
    #[error(
        "--connection-ref takes at most three components (`realm:binding[:profile]`); got `{raw}` with {extra_segments} extra segment(s)"
    )]
    TooManySegments { raw: String, extra_segments: usize },

    /// The realm component failed slug validation.
    #[error("--connection-ref realm component `{component}` is not a valid slug: {source}")]
    InvalidRealm {
        component: String,
        #[source]
        source: IdentityError,
    },

    /// The binding component failed slug validation.
    #[error("--connection-ref binding component `{component}` is not a valid slug: {source}")]
    InvalidBinding {
        component: String,
        #[source]
        source: IdentityError,
    },

    /// The profile component failed slug validation.
    #[error("--connection-ref profile component `{component}` is not a valid slug: {source}")]
    InvalidProfile {
        component: String,
        #[source]
        source: IdentityError,
    },
}

/// Parse a user-supplied `realm:binding[:profile]` string into a typed
/// [`ConnectionRef`].
///
/// This is the **sole** ConnectionRef parser in `meerkat-cli`. Every
/// CLI surface that accepts a `--connection-ref` flag must funnel the
/// raw string through this function at the argument-decoding boundary;
/// downstream code paths accept only the typed `ConnectionRef`.
///
/// ### Grammar
///
/// ```text
/// connection_ref := realm ":" binding [ ":" profile ]
/// realm          := slug
/// binding        := slug
/// profile        := slug
/// slug           := /[A-Za-z0-9][A-Za-z0-9._-]*/
/// ```
///
/// The slug grammar is enforced by
/// [`meerkat_core::connection::RealmId::parse`] and friends; this
/// function only splits on the first two `:` separators.
///
/// ### Errors
///
/// Returns a typed [`CliError`] variant describing *which* component
/// failed. Parse errors never panic — the CLI frontend formats them
/// and exits with a non-zero status via `anyhow::bail!` at the
/// clap-boundary call site.
pub fn parse_connection_ref_user_input(raw: &str) -> Result<ConnectionRef, CliError> {
    let trimmed = raw.trim();
    // Split on ':' — we allow up to 3 segments (realm, binding, profile).
    // Using splitn(4, …) gives us "at most 3 segments + one leftover"
    // so we can cleanly reject 4+-segment inputs with a typed error.
    let mut parts = trimmed.splitn(4, ':');
    let realm_str = parts
        .next()
        .expect("splitn always yields at least one element");
    let Some(binding_str) = parts.next() else {
        return Err(CliError::MissingBinding {
            raw: raw.to_owned(),
        });
    };
    let profile_str = parts.next();
    // If a fourth segment showed up, the input has too many colons.
    if let Some(extra) = parts.next() {
        // One extra found; count any further trailing colon-separated
        // segments by consulting the original string's colon count so
        // the error surfaces the actual overrun.
        let extra_segments = extra.matches(':').count() + 1;
        return Err(CliError::TooManySegments {
            raw: raw.to_owned(),
            extra_segments,
        });
    }

    let realm = RealmId::parse(realm_str).map_err(|source| CliError::InvalidRealm {
        component: realm_str.to_owned(),
        source,
    })?;
    let binding = BindingId::parse(binding_str).map_err(|source| CliError::InvalidBinding {
        component: binding_str.to_owned(),
        source,
    })?;
    let profile = match profile_str {
        None => None,
        Some(p) => Some(
            ProfileId::parse(p).map_err(|source| CliError::InvalidProfile {
                component: p.to_owned(),
                source,
            })?,
        ),
    };

    Ok(ConnectionRef {
        realm,
        binding,
        profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_realm_and_binding() {
        let cref = parse_connection_ref_user_input("dev:openai").expect("valid realm:binding");
        assert_eq!(cref.realm.as_str(), "dev");
        assert_eq!(cref.binding.as_str(), "openai");
        assert!(cref.profile.is_none());
    }

    #[test]
    fn parses_realm_binding_profile() {
        let cref = parse_connection_ref_user_input("prod:anthropic:paid")
            .expect("valid three-component form");
        assert_eq!(cref.realm.as_str(), "prod");
        assert_eq!(cref.binding.as_str(), "anthropic");
        assert_eq!(cref.profile.as_ref().map(|p| p.as_str()), Some("paid"));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let cref =
            parse_connection_ref_user_input("  dev:openai  ").expect("whitespace is trimmed");
        assert_eq!(cref.realm.as_str(), "dev");
        assert_eq!(cref.binding.as_str(), "openai");
    }

    #[test]
    fn rejects_missing_binding() {
        let err = parse_connection_ref_user_input("onlyrealm").expect_err("no colon");
        assert!(matches!(err, CliError::MissingBinding { .. }));
    }

    #[test]
    fn rejects_four_segments() {
        let err =
            parse_connection_ref_user_input("a:b:c:d").expect_err("fourth segment is rejected");
        assert!(matches!(err, CliError::TooManySegments { .. }));
    }

    #[test]
    fn rejects_invalid_realm_character() {
        let err = parse_connection_ref_user_input("dev$:openai")
            .expect_err("`$` is not a valid slug char");
        assert!(matches!(err, CliError::InvalidRealm { .. }));
    }

    #[test]
    fn rejects_invalid_binding_character() {
        let err = parse_connection_ref_user_input("dev:open ai")
            .expect_err("space is not a valid slug char");
        assert!(matches!(err, CliError::InvalidBinding { .. }));
    }

    #[test]
    fn rejects_invalid_profile_character() {
        let err = parse_connection_ref_user_input("dev:openai:pa!d")
            .expect_err("`!` is not a valid slug char");
        assert!(matches!(err, CliError::InvalidProfile { .. }));
    }

    #[test]
    fn rejects_empty_realm() {
        let err = parse_connection_ref_user_input(":openai").expect_err("empty realm");
        assert!(matches!(err, CliError::InvalidRealm { .. }));
    }

    #[test]
    fn rejects_empty_binding() {
        let err = parse_connection_ref_user_input("dev:").expect_err("empty binding");
        assert!(matches!(err, CliError::InvalidBinding { .. }));
    }
}
