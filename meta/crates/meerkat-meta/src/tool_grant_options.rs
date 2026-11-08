//! The launch option for scoped tool grants: `runtime_options.tool_grants`
//! on the stdio gateway, and the init key `tool_grants` on `meta_gateway`.
//!
//! `{max_lifetime_ms}` selects the longest window a grant may have. The
//! gateway sets it as its runtime's grant policy at init, before any session
//! exists ([`meerkat_runtime::MeerkatMachine::set_tool_grant_policy`]);
//! without it the runtime's default of 24 hours stands. Every grant has a
//! fixed deadline, so a grant "until revoked" is a grant at the host's
//! maximum, which the host renews before it ends.

use std::time::Duration;

use serde_json::{Value, json};

/// The `error.data.kind` of an init refused for its maximum lifetime.
pub const TOOL_GRANT_LIFETIME_INVALID_KIND: &str = "tool_grant_lifetime_invalid";

/// The largest maximum lifetime, in milliseconds: 2^53 - 1, which every SDK
/// reads exactly.
pub const MAX_TOOL_GRANT_LIFETIME_MS: u64 = (1 << 53) - 1;

/// The parsed launch option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolGrantLaunchOptions {
    /// The longest window a grant may have.
    pub max_lifetime: Duration,
}

/// Why the launch option refuses the launch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolGrantLaunchOptionsError {
    /// The option is not a JSON object, or names a field it does not know.
    Malformed(String),
    /// `max_lifetime_ms` is not a positive whole number of milliseconds at
    /// most 2^53 - 1.
    LifetimeInvalid {
        /// The option's name in the launch parameters.
        field: &'static str,
    },
}

impl std::fmt::Display for ToolGrantLaunchOptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(message) => f.write_str(message),
            Self::LifetimeInvalid { field } => write!(
                f,
                "{field}.max_lifetime_ms must be a positive whole number of milliseconds, at \
                 most {MAX_TOOL_GRANT_LIFETIME_MS}"
            ),
        }
    }
}

impl std::error::Error for ToolGrantLaunchOptionsError {}

impl ToolGrantLaunchOptionsError {
    /// The refusal's `error.data`, for a refusal with a kind.
    #[must_use]
    pub fn data(&self) -> Option<Value> {
        match self {
            Self::Malformed(_) => None,
            Self::LifetimeInvalid { .. } => {
                Some(json!({ "kind": TOOL_GRANT_LIFETIME_INVALID_KIND }))
            }
        }
    }
}

impl ToolGrantLaunchOptions {
    /// Parse the option `field` from its launch value; `None` when the launch
    /// names no option or no maximum.
    ///
    /// # Errors
    /// [`ToolGrantLaunchOptionsError::Malformed`] for a value that is not an
    /// object or a field the option does not know, and
    /// [`ToolGrantLaunchOptionsError::LifetimeInvalid`] for a maximum that is
    /// not a positive whole number of milliseconds within 2^53 - 1.
    pub fn parse(
        value: Option<&Value>,
        field: &'static str,
    ) -> Result<Option<Self>, ToolGrantLaunchOptionsError> {
        let Some(value) = value else {
            return Ok(None);
        };
        let object = value.as_object().ok_or_else(|| {
            ToolGrantLaunchOptionsError::Malformed(format!("{field} must be a JSON object"))
        })?;
        let unknown = object
            .keys()
            .filter(|key| key.as_str() != "max_lifetime_ms")
            .map(String::as_str)
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            return Err(ToolGrantLaunchOptionsError::Malformed(format!(
                "unsupported {field} fields: {}",
                unknown.join(", ")
            )));
        }
        let Some(raw) = object.get("max_lifetime_ms") else {
            return Ok(None);
        };
        let max_lifetime_ms = raw
            .as_u64()
            .filter(|ms| (1..=MAX_TOOL_GRANT_LIFETIME_MS).contains(ms))
            .ok_or(ToolGrantLaunchOptionsError::LifetimeInvalid { field })?;
        Ok(Some(Self {
            max_lifetime: Duration::from_millis(max_lifetime_ms),
        }))
    }

    /// The runtime's grant policy under this option.
    #[must_use]
    pub fn policy(&self) -> meerkat_runtime::tool_grant::ToolGrantPolicy {
        meerkat_runtime::tool_grant::ToolGrantPolicy {
            max_lifetime: self.max_lifetime,
        }
    }

    /// Set this option as `machine`'s grant policy.
    pub fn apply(&self, machine: &meerkat_runtime::MeerkatMachine) {
        machine.set_tool_grant_policy(self.policy());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const FIELD: &str = "runtime_options.tool_grants";

    #[test]
    fn a_maximum_within_the_bounds_sets_the_policy() {
        let parsed = ToolGrantLaunchOptions::parse(
            Some(&json!({ "max_lifetime_ms": 34_560_000_000_u64 })),
            FIELD,
        )
        .expect("a 400-day maximum parses")
        .expect("the option names a maximum");
        assert_eq!(parsed.max_lifetime, Duration::from_millis(34_560_000_000));
        assert_eq!(parsed.policy().max_lifetime_ms(), 34_560_000_000);
        let largest = ToolGrantLaunchOptions::parse(
            Some(&json!({ "max_lifetime_ms": MAX_TOOL_GRANT_LIFETIME_MS })),
            FIELD,
        )
        .expect("2^53 - 1 parses");
        assert!(largest.is_some());
    }

    #[test]
    fn an_absent_option_or_maximum_leaves_the_default() {
        assert_eq!(ToolGrantLaunchOptions::parse(None, FIELD), Ok(None));
        assert_eq!(
            ToolGrantLaunchOptions::parse(Some(&json!({})), FIELD),
            Ok(None)
        );
    }

    #[test]
    fn a_maximum_out_of_bounds_is_refused_with_its_kind() {
        for value in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!(MAX_TOOL_GRANT_LIFETIME_MS + 1),
            json!("86400000"),
            json!(null),
        ] {
            let error =
                ToolGrantLaunchOptions::parse(Some(&json!({ "max_lifetime_ms": value })), FIELD)
                    .expect_err("refused");
            assert_eq!(
                error.data(),
                Some(json!({ "kind": TOOL_GRANT_LIFETIME_INVALID_KIND })),
                "{value}"
            );
        }
    }

    #[test]
    fn a_malformed_option_is_refused_without_a_kind() {
        for value in [json!([]), json!({ "max_lifetime": 1 })] {
            let error = ToolGrantLaunchOptions::parse(Some(&value), FIELD).expect_err("refused");
            assert_eq!(error.data(), None, "{value}");
        }
    }
}
