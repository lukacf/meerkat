//! Governance settings of a realm's native composition, read from the
//! realm's ordinary `config.toml` with the rest of [`crate::Config`]: the
//! stock governed host's owner-selected review rules and its explicit
//! reviewer, and the approver ids the operator of an `rkat-rpc` stdio
//! connection decides durable decisions for.
//!
//! ```toml
//! [[governance.review.rules]]
//! target = { kind = "mcp_operation", server = "mail", operation = "send" }
//! tier = "r3"
//! consent_assurance = "person_authenticated"
//!
//! [[governance.review.rules]]
//! target = { kind = "mcp_server", server = "calendar" }
//! tier = "r2"
//!
//! [governance.review.reviewer]
//! identity = { model = "<model-id>", provider = "anthropic" }
//! max_output_tokens = 1024
//!
//! [governance.decisions]
//! operator_approver_ids = ["operator", "team:owners"]
//! ```

use serde::{Deserialize, Deserializer, Serialize};

use crate::approval::ApprovalAssurance;
use crate::approval::decision::ApproverId;
use crate::config::ConfigError;
use crate::session::SessionLlmIdentity;

/// Governance of a realm's native composition. By default a stock governed
/// realm keeps every operation at its local R1 tier and calls no reviewer,
/// and the stdio operator decides for the approver id `operator`.
/// A present section ([`crate::Config::governance`]), even an empty one,
/// replaces an inherited one as a whole.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct GovernanceConfig {
    /// The review the stock governed host requires.
    #[serde(skip_serializing_if = "StockReviewConfig::is_empty")]
    pub review: StockReviewConfig,
    /// The approver ids the stdio operator decides durable decisions for.
    #[serde(skip_serializing_if = "DecisionGovernanceConfig::is_default")]
    pub decisions: DecisionGovernanceConfig,
}

impl GovernanceConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.review.is_empty() && self.decisions.is_default()
    }

    /// # Errors
    /// [`ConfigError::Validation`] for a malformed review section or an
    /// operator approver id that is not an approver id.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.review.validate()?;
        self.decisions.validate()
    }
}

/// The durable decisions the operator of an `rkat-rpc` stdio connection
/// decides: those addressed to one of `operator_approver_ids`. The operator
/// is the process's operating-system user (on a governed pipe, the
/// composition's operator); the connection presents it the open decisions
/// of these approver ids when they open while the connection is open and
/// when its client lists them, and it decides only a decision the
/// connection presented, while the decision's approver id is still listed
/// here.
///
/// Changing the list is approver-authority administration, so it lives in
/// the host-administered governance section: an ordinary config write can
/// neither add, change nor remove it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct DecisionGovernanceConfig {
    /// The approver ids the operator acts for. Absent, it is `["operator"]`;
    /// an explicitly empty list presents nothing and decides nothing.
    pub operator_approver_ids: Vec<String>,
}

/// The approver id the stdio operator acts for when the realm states none.
const DEFAULT_OPERATOR_APPROVER_ID: &str = "operator";

impl Default for DecisionGovernanceConfig {
    fn default() -> Self {
        Self {
            operator_approver_ids: vec![DEFAULT_OPERATOR_APPROVER_ID.to_owned()],
        }
    }
}

impl DecisionGovernanceConfig {
    /// Whether this is the default: the operator acts for `operator` only.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the operator acts for `approver_id`.
    #[must_use]
    pub fn lists(&self, approver_id: &str) -> bool {
        self.operator_approver_ids
            .iter()
            .any(|listed| listed == approver_id)
    }

    /// Every listed entry is an approver id.
    ///
    /// # Errors
    /// [`ConfigError::Validation`] naming the first entry that is not one.
    pub fn validate(&self) -> Result<(), ConfigError> {
        match self
            .operator_approver_ids
            .iter()
            .find(|entry| ApproverId::new(entry.as_str()).is_err())
        {
            Some(entry) => Err(ConfigError::Validation(format!(
                "governance.decisions.operator_approver_ids: {entry:?} is not an approver id \
                 (non-empty, at most 256 bytes, no whitespace or control characters)"
            ))),
            None => Ok(()),
        }
    }
}

/// The stock governed host's review requirements: owner-selected rules that
/// raise the review tier of exact native operation targets above the stock
/// rules' own R1, and the explicit model that reviews R2 operations.
///
/// A rule only raises a tier. It never allows an operation the stock rules
/// refuse or leave unmapped, and it never lowers the tier another owner
/// requires; the strictest requirement of every matching rule and owner
/// applies. Without a reviewer an R2 operation is refused locally with typed
/// feedback; nothing falls back to R1 or to another model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct StockReviewConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<StockReviewRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<StockReviewerSelection>,
}

impl StockReviewConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.reviewer.is_none()
    }

    /// Every rule names a complete target and appears once, and states the
    /// consent assurance exactly when it requires R3; the reviewer names a
    /// model, has an output budget and carries no per-session provider
    /// parameters.
    ///
    /// # Errors
    /// [`ConfigError::Validation`] naming the first malformed entry.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::BTreeSet::new();
        for rule in &self.rules {
            if !rule.target.is_complete() {
                return Err(ConfigError::Validation(
                    "governance.review.rules target names must not be empty".into(),
                ));
            }
            if !seen.insert(&rule.target) {
                return Err(ConfigError::Validation(
                    "governance.review.rules names the same target more than once".into(),
                ));
            }
            match (rule.tier, rule.consent_assurance) {
                (StockReviewTier::R3, None) => {
                    return Err(ConfigError::Validation(
                        "governance.review.rules: an r3 rule must state consent_assurance".into(),
                    ));
                }
                (StockReviewTier::R2, Some(_)) => {
                    return Err(ConfigError::Validation(
                        "governance.review.rules: an r2 rule takes no consent_assurance".into(),
                    ));
                }
                _ => {}
            }
        }
        if let Some(reviewer) = &self.reviewer {
            if reviewer.identity.model.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "governance.review.reviewer.identity.model must not be empty".into(),
                ));
            }
            if reviewer.identity.provider_params.is_some() {
                return Err(ConfigError::Validation(
                    "governance.review.reviewer.identity must not carry provider_params".into(),
                ));
            }
            if reviewer.max_output_tokens == 0 {
                return Err(ConfigError::Validation(
                    "governance.review.reviewer.max_output_tokens must be greater than 0".into(),
                ));
            }
        }
        Ok(())
    }
}

/// One owner-selected rule: the review tier `target`'s operations require,
/// and for R3 the weakest approver assurance its human consent accepts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StockReviewRule {
    pub target: StockReviewTarget,
    pub tier: StockReviewTier,
    /// Required for an R3 rule and refused for an R2 rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_assurance: Option<StockConsentAssurance>,
}

/// The weakest authenticated approver assurance an R3 rule's human consent
/// accepts. Ordered by strength: where several R3 rules match one operation,
/// the strongest they state applies. A delegated service credential never
/// decides human consent, whatever a rule states.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StockConsentAssurance {
    // Plain comments on the variants keep the schema one string enum.
    // The device or network trust of a declared local-operator posture (the
    // stdio operator's own pipe). A person-authenticated approver satisfies
    // it too.
    DeviceTrusted,
    // The person was authenticated on the actual decision event.
    PersonAuthenticated,
}

impl StockConsentAssurance {
    /// Whether an approver authenticated with `assurance` satisfies it.
    #[must_use]
    pub fn accepts(self, assurance: ApprovalAssurance) -> bool {
        match assurance {
            ApprovalAssurance::PersonAuthenticated => true,
            ApprovalAssurance::DeviceTrusted => self == Self::DeviceTrusted,
            ApprovalAssurance::ServiceCredential => false,
        }
    }

    /// The configuration name of the assurance.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeviceTrusted => "device_trusted",
            Self::PersonAuthenticated => "person_authenticated",
        }
    }

    /// The assurance of a configuration name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "device_trusted" => Some(Self::DeviceTrusted),
            "person_authenticated" => Some(Self::PersonAuthenticated),
            _ => None,
        }
    }
}

/// A required review tier above the stock rules' own R1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StockReviewTier {
    // Plain comments on the variants keep the schema one string enum.
    // A model reviews the exact operation before it runs.
    R2,
    // A human consents to the exact operation before it runs.
    R3,
}

/// An exact native operation target, by the names its owner registers it
/// under, never a model-facing display name. A selector may name a server
/// or bundle that is connected or registered only later: nothing is
/// resolved or discovered when the configuration loads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StockReviewTarget {
    /// Every operation of one configured MCP server, by its configured name.
    McpServer { server: String },
    /// One raw operation of one configured MCP server.
    McpOperation { server: String, operation: String },
    /// One operation of a tool bundle a trusted host registered under its
    /// stable name.
    ToolBundleOperation { bundle: String, operation: String },
}

impl StockReviewTarget {
    fn is_complete(&self) -> bool {
        match self {
            Self::McpServer { server } => !server.trim().is_empty(),
            Self::McpOperation { server, operation } => {
                !server.trim().is_empty() && !operation.trim().is_empty()
            }
            Self::ToolBundleOperation { bundle, operation } => {
                !bundle.trim().is_empty() && !operation.trim().is_empty()
            }
        }
    }
}

/// The explicit model that reviews R2 operations: a session LLM identity
/// (model, provider and its configured route or auth binding) and its
/// output budget. There is no default model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StockReviewerSelection {
    /// Read strictly: a misspelled field anywhere in the identity is
    /// refused, never dropped.
    #[serde(deserialize_with = "configured_identity")]
    pub identity: SessionLlmIdentity,
    pub max_output_tokens: u32,
}

/// The reviewer identity as a governance document states it: every field of
/// a [`SessionLlmIdentity`] and nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredIdentity {
    model: String,
    provider: crate::Provider,
    #[serde(default)]
    self_hosted_server_id: Option<String>,
    #[serde(default)]
    provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
    #[serde(default)]
    auth_binding: Option<ConfiguredAuthBinding>,
}

/// The reviewer identity's auth binding as a governance document states it:
/// every field of a [`crate::AuthBindingRef`] and nothing else.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfiguredAuthBinding {
    realm: crate::connection::RealmId,
    binding: crate::connection::BindingId,
    #[serde(default)]
    profile: Option<crate::connection::ProfileId>,
    #[serde(default)]
    origin: crate::connection::BindingOrigin,
}

fn configured_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<SessionLlmIdentity, D::Error> {
    let identity = ConfiguredIdentity::deserialize(deserializer)?;
    Ok(SessionLlmIdentity {
        model: identity.model,
        provider: identity.provider,
        self_hosted_server_id: identity.self_hosted_server_id,
        provider_params: identity.provider_params,
        auth_binding: identity.auth_binding.map(|binding| crate::AuthBindingRef {
            realm: binding.realm,
            binding: binding.binding,
            profile: binding.profile,
            origin: binding.origin,
        }),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::Config;

    const DOCUMENTED: &str = r#"
[[governance.review.rules]]
target = { kind = "mcp_operation", server = "mail", operation = "send" }
tier = "r3"
consent_assurance = "person_authenticated"

[[governance.review.rules]]
target = { kind = "mcp_server", server = "calendar" }
tier = "r2"

[governance.review.reviewer]
identity = { model = "test-anthropic-second", provider = "anthropic" }
max_output_tokens = 1024
"#;

    fn mcp_operation(server: &str, operation: &str) -> StockReviewTarget {
        StockReviewTarget::McpOperation {
            server: server.into(),
            operation: operation.into(),
        }
    }

    fn rule(
        target: StockReviewTarget,
        tier: StockReviewTier,
        consent_assurance: Option<StockConsentAssurance>,
    ) -> StockReviewRule {
        StockReviewRule {
            target,
            tier,
            consent_assurance,
        }
    }

    fn review(rules: Vec<StockReviewRule>) -> StockReviewConfig {
        StockReviewConfig {
            rules,
            reviewer: None,
        }
    }

    fn refusal(result: Result<(), ConfigError>) -> String {
        match result {
            Err(ConfigError::Validation(message)) => message,
            other => panic!("expected a validation refusal, got {other:?}"),
        }
    }

    #[test]
    fn the_documented_section_reads_and_validates() {
        let (config, _) = Config::from_persisted_toml(DOCUMENTED).expect("documented section");
        let governance = config.governance.as_ref().expect("the stated section");
        let review = &governance.review;
        assert_eq!(
            review.rules,
            vec![
                rule(
                    mcp_operation("mail", "send"),
                    StockReviewTier::R3,
                    Some(StockConsentAssurance::PersonAuthenticated),
                ),
                rule(
                    StockReviewTarget::McpServer {
                        server: "calendar".into()
                    },
                    StockReviewTier::R2,
                    None,
                ),
            ]
        );
        let reviewer = review.reviewer.as_ref().expect("reviewer");
        assert_eq!(reviewer.identity.model, "test-anthropic-second");
        assert_eq!(reviewer.max_output_tokens, 1024);
        governance
            .validate()
            .expect("the documented section is valid");
        // Without the section nothing is stated and nothing is serialized.
        let absent = Config::default();
        assert_eq!(absent.governance, None);
        let written = toml::to_string(&absent).expect("serialize");
        assert!(!written.contains("governance"), "{written}");
    }

    /// An explicitly empty section is stated, not absent: it reads back as
    /// present and is written back as present, so it keeps resetting what
    /// a parent states.
    #[test]
    fn an_explicitly_empty_section_stays_present_through_write_and_readback() {
        for document in ["[governance]\n", "[governance.review]\nrules = []\n"] {
            let (config, _) = Config::from_persisted_toml(document).expect(document);
            assert_eq!(
                config.governance,
                Some(GovernanceConfig::default()),
                "{document}"
            );
            let written = toml::to_string(&config).expect("serialize");
            let (read_back, _) = Config::from_persisted_toml(&written).expect("read back");
            assert_eq!(
                read_back.governance,
                Some(GovernanceConfig::default()),
                "{written}"
            );
        }
    }

    #[test]
    fn an_r3_rule_states_its_consent_assurance_and_an_r2_rule_none() {
        let target = mcp_operation("mail", "send");
        assert!(
            refusal(review(vec![rule(target.clone(), StockReviewTier::R3, None)]).validate())
                .contains("an r3 rule must state consent_assurance")
        );
        assert!(
            refusal(
                review(vec![rule(
                    target.clone(),
                    StockReviewTier::R2,
                    Some(StockConsentAssurance::DeviceTrusted),
                )])
                .validate()
            )
            .contains("an r2 rule takes no consent_assurance")
        );
        review(vec![rule(
            target,
            StockReviewTier::R3,
            Some(StockConsentAssurance::DeviceTrusted),
        )])
        .validate()
        .expect("an r3 rule with its assurance");
    }

    #[test]
    fn a_rule_names_a_complete_target_once() {
        let assurance = Some(StockConsentAssurance::PersonAuthenticated);
        let repeated = review(vec![
            rule(
                mcp_operation("mail", "send"),
                StockReviewTier::R3,
                assurance,
            ),
            rule(mcp_operation("mail", "send"), StockReviewTier::R2, None),
        ]);
        assert!(refusal(repeated.validate()).contains("more than once"));
        for incomplete in [
            StockReviewTarget::McpServer { server: " ".into() },
            mcp_operation("mail", ""),
            StockReviewTarget::ToolBundleOperation {
                bundle: String::new(),
                operation: "send".into(),
            },
        ] {
            assert!(
                refusal(review(vec![rule(incomplete, StockReviewTier::R2, None)]).validate())
                    .contains("must not be empty")
            );
        }
        // The same name under two target kinds is two targets.
        let distinct = review(vec![
            rule(
                StockReviewTarget::McpServer {
                    server: "mail".into(),
                },
                StockReviewTier::R2,
                None,
            ),
            rule(
                mcp_operation("mail", "send"),
                StockReviewTier::R3,
                assurance,
            ),
        ]);
        distinct.validate().expect("two target kinds");
    }

    #[test]
    fn the_reviewer_names_a_model_and_an_output_budget() {
        let (config, _) = Config::from_persisted_toml(DOCUMENTED).expect("documented section");
        let selection = config
            .governance
            .expect("the stated section")
            .review
            .reviewer
            .expect("reviewer");
        let with_reviewer = |selection: StockReviewerSelection| StockReviewConfig {
            rules: Vec::new(),
            reviewer: Some(selection),
        };
        let mut unnamed = selection.clone();
        unnamed.identity.model = "  ".into();
        assert!(refusal(with_reviewer(unnamed).validate()).contains("model must not be empty"));
        let mut unbudgeted = selection.clone();
        unbudgeted.max_output_tokens = 0;
        assert!(refusal(with_reviewer(unbudgeted).validate()).contains("greater than 0"));
        with_reviewer(selection)
            .validate()
            .expect("a named model with a budget");
    }

    #[test]
    fn unknown_review_fields_are_refused_when_read() {
        for document in [
            "[governance.review]\nunknown = 1\n",
            "[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"mail\" }\ntier = \"r2\"\nextra = true\n",
            "[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"mail\", operation = \"send\" }\ntier = \"r2\"\n",
            "[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"mail\" }\ntier = \"r1\"\n",
            // A misspelled identity field is refused, never dropped: at the
            // identity and inside its auth binding.
            "[governance.review.reviewer]\nmax_output_tokens = 64\nidentity = { model = \"m\", provider = \"anthropic\", auth_bindng = { realm = \"r\", binding = \"b\" } }\n",
            "[governance.review.reviewer]\nmax_output_tokens = 64\nidentity = { model = \"m\", provider = \"anthropic\", auth_binding = { realm = \"r\", binding = \"b\", profle = \"p\" } }\n",
        ] {
            assert!(Config::from_persisted_toml(document).is_err(), "{document}");
        }
    }

    #[test]
    fn a_present_section_replaces_the_inherited_one_whole() {
        let mut config = Config::default();
        config.merge_toml_str(DOCUMENTED).expect("parent layer");
        // A layer without the section keeps the inherited one.
        config
            .merge_toml_str("max_tokens = 2048\n")
            .expect("unrelated layer");
        let inherited = config.governance.clone().expect("inherited section");
        assert_eq!(inherited.review.rules.len(), 2);
        assert!(inherited.review.reviewer.is_some());
        // A layer with the section replaces it as a whole: the inherited
        // reviewer and rules are gone, never merged.
        config
            .merge_toml_str(
                "[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"files\" }\ntier = \"r2\"\n",
            )
            .expect("child layer");
        let replaced = config.governance.clone().expect("replaced section");
        assert_eq!(
            replaced.review.rules,
            vec![rule(
                StockReviewTarget::McpServer {
                    server: "files".into()
                },
                StockReviewTier::R2,
                None,
            )]
        );
        assert_eq!(replaced.review.reviewer, None);
        // An explicitly empty section resets it: nothing is inherited.
        config
            .merge_toml_str("[governance]\n")
            .expect("resetting layer");
        assert_eq!(config.governance, Some(GovernanceConfig::default()));
    }

    /// A misspelled field anywhere in the reviewer identity is refused; the
    /// identity a document states reads exactly.
    #[test]
    fn the_reviewer_identity_is_read_exactly() {
        let (config, _) = Config::from_persisted_toml(
            "[governance.review.reviewer]\nmax_output_tokens = 64\n\
             identity = { model = \"m\", provider = \"anthropic\", \
             auth_binding = { realm = \"r\", binding = \"b\", profile = \"p\" } }\n",
        )
        .expect("an exact identity");
        let identity = config
            .governance
            .and_then(|governance| governance.review.reviewer)
            .expect("reviewer")
            .identity;
        let binding = identity.auth_binding.expect("auth binding");
        assert_eq!(binding.realm.as_str(), "r");
        assert_eq!(binding.binding.as_str(), "b");
        assert_eq!(
            binding.profile.map(|profile| profile.as_str().to_owned()),
            Some("p".to_owned())
        );
    }

    fn decisions_of(document: &str) -> DecisionGovernanceConfig {
        let (config, _) = Config::from_persisted_toml(document).expect(document);
        config.governance.unwrap_or_default().decisions
    }

    /// Without a stated list the operator decides for `operator` only, and
    /// the default is never written out.
    #[test]
    fn an_absent_operator_list_is_operator_only() {
        for document in [
            "max_tokens = 1024\n",
            "[governance]\n",
            "[governance.decisions]\n",
        ] {
            let decisions = decisions_of(document);
            assert_eq!(decisions, DecisionGovernanceConfig::default(), "{document}");
            assert_eq!(decisions.operator_approver_ids, vec!["operator".to_owned()]);
            assert!(decisions.is_default());
            assert!(decisions.lists("operator"));
            assert!(!decisions.lists("team:owners"));
            decisions.validate().expect("the default list is valid");
        }
        let (config, _) =
            Config::from_persisted_toml("[governance.decisions]\n").expect("stated section");
        let written = toml::to_string(&config).expect("serialize");
        assert!(!written.contains("operator_approver_ids"), "{written}");
    }

    /// An explicitly empty list is stated, not absent: the operator decides
    /// for nobody, and the empty list survives a write and a read back.
    #[test]
    fn an_explicitly_empty_operator_list_lists_nobody() {
        let document = "[governance.decisions]\noperator_approver_ids = []\n";
        let decisions = decisions_of(document);
        assert!(decisions.operator_approver_ids.is_empty());
        assert!(!decisions.is_default());
        assert!(!decisions.lists("operator"));
        decisions.validate().expect("an empty list is valid");
        let (config, _) = Config::from_persisted_toml(document).expect(document);
        assert!(!config.governance.as_ref().expect("stated").is_empty());
        let written = toml::to_string(&config).expect("serialize");
        assert_eq!(decisions_of(&written), decisions, "{written}");
    }

    #[test]
    fn an_operator_list_entry_that_is_not_an_approver_id_is_refused() {
        let oversized = "x".repeat(257);
        for entry in ["", "two words", "tab\there", oversized.as_str()] {
            let decisions = DecisionGovernanceConfig {
                operator_approver_ids: vec!["operator".to_owned(), entry.to_owned()],
            };
            assert!(
                refusal(decisions.validate())
                    .contains("governance.decisions.operator_approver_ids"),
                "{entry:?}"
            );
            let governance = GovernanceConfig {
                decisions,
                ..GovernanceConfig::default()
            };
            assert!(
                refusal(governance.validate())
                    .contains("governance.decisions.operator_approver_ids"),
                "{entry:?}"
            );
        }
        // Unknown fields of the section are refused when read.
        for document in [
            "[governance.decisions]\nunknown = 1\n",
            "[governance.decisions]\noperator_approver_ids = \"operator\"\n",
        ] {
            assert!(Config::from_persisted_toml(document).is_err(), "{document}");
        }
    }

    /// A stated list reads exactly, in order, and survives a TOML write and
    /// a JSON round trip unchanged.
    #[test]
    fn a_stated_operator_list_round_trips() {
        let document =
            "[governance.decisions]\noperator_approver_ids = [\"operator\", \"team:owners\"]\n";
        let decisions = decisions_of(document);
        assert_eq!(
            decisions.operator_approver_ids,
            vec!["operator".to_owned(), "team:owners".to_owned()]
        );
        assert!(decisions.lists("team:owners"));
        assert!(!decisions.lists("team:others"));
        decisions.validate().expect("a valid list");
        let (config, _) = Config::from_persisted_toml(document).expect(document);
        let written = toml::to_string(&config).expect("serialize");
        assert_eq!(decisions_of(&written), decisions, "{written}");
        let governance = config.governance.expect("stated");
        let json = serde_json::to_value(&governance).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "decisions": { "operator_approver_ids": ["operator", "team:owners"] } })
        );
        let read: GovernanceConfig = serde_json::from_value(json).expect("read back");
        assert_eq!(read, governance);
    }

    #[test]
    fn a_consent_assurance_accepts_only_a_human_at_least_as_strongly_authenticated() {
        use StockConsentAssurance::{DeviceTrusted, PersonAuthenticated};
        let cases = [
            (DeviceTrusted, ApprovalAssurance::DeviceTrusted, true),
            (DeviceTrusted, ApprovalAssurance::PersonAuthenticated, true),
            (DeviceTrusted, ApprovalAssurance::ServiceCredential, false),
            (PersonAuthenticated, ApprovalAssurance::DeviceTrusted, false),
            (
                PersonAuthenticated,
                ApprovalAssurance::PersonAuthenticated,
                true,
            ),
            (
                PersonAuthenticated,
                ApprovalAssurance::ServiceCredential,
                false,
            ),
        ];
        for (accepted, assurance, accepts) in cases {
            assert_eq!(
                accepted.accepts(assurance),
                accepts,
                "{accepted:?} for {assurance:?}"
            );
        }
        assert!(DeviceTrusted < PersonAuthenticated);
        for assurance in [DeviceTrusted, PersonAuthenticated] {
            assert_eq!(
                StockConsentAssurance::from_name(assurance.as_str()),
                Some(assurance)
            );
        }
        assert_eq!(StockConsentAssurance::from_name("service_credential"), None);
    }
}
