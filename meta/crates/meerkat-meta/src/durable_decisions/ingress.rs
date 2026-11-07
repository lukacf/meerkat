//! The decision ingresses a launch declares (`runtime_options.decisions`).
//!
//! A declared ingress is a host channel, such as a chat, that authenticates
//! the people who decide through it and attests them with a stdio
//! `meta/decisions/decide`. The declaration holds no approver mapping: the
//! approver map in the access configuration lists which of its people hold
//! which approver ids (`ingress_subjects`, `people`). Its clock skew is at
//! most one hour.
//!
//! ```json
//! {"ingresses": [{"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000}]}
//! ```

use serde::{Deserialize, Serialize};

use crate::access::{AccessConfigError, AccessController, OPERATOR_INGRESS, is_ingress_name};

/// What a declared ingress attests: a person authenticated on the event.
const ATTESTS_PERSON: &str = "person";

/// The largest clock skew an ingress may declare: one hour.
const MAX_CLOCK_SKEW_MS: u64 = 3_600_000;

/// The decision ingresses a launch declares. Each name is an ingress token
/// (1 to 64 of `a-z`, `0-9`, `.`, `_` and `-`), declared once, and never the
/// built-in `operator`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    try_from = "RawDecisionIngressDeclarations",
    into = "RawDecisionIngressDeclarations"
)]
pub struct DecisionIngressDeclarations {
    ingresses: Vec<DecisionIngressDeclaration>,
}

/// One declared ingress: its name, and how far ahead of this host's clock
/// the time it observed an event may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionIngressDeclaration {
    ingress: String,
    max_clock_skew_ms: u64,
}

impl DecisionIngressDeclaration {
    /// The ingress's name, which the decisions it attests record as their
    /// ingress.
    pub fn ingress(&self) -> &str {
        &self.ingress
    }

    /// How far, in milliseconds, the time the ingress observed an event may
    /// be ahead of this host's clock: at most one hour.
    pub fn max_clock_skew_ms(&self) -> u64 {
        self.max_clock_skew_ms
    }
}

impl DecisionIngressDeclarations {
    /// The declaration of `ingress`, if the launch declares it.
    pub fn get(&self, ingress: &str) -> Option<&DecisionIngressDeclaration> {
        self.ingresses
            .iter()
            .find(|declaration| declaration.ingress == ingress)
    }

    /// The declared ingresses, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = &DecisionIngressDeclaration> {
        self.ingresses.iter()
    }

    /// Whether the launch declares no ingress.
    pub fn is_empty(&self) -> bool {
        self.ingresses.is_empty()
    }
}

/// A refused ingress declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecisionIngressError {
    InvalidName(String),
    ReservedName,
    UnsupportedAttests {
        ingress: String,
        attests: String,
    },
    Duplicate(String),
    /// The declared clock skew is more than one hour.
    SkewAboveOneHour {
        ingress: String,
        max_clock_skew_ms: u64,
    },
}

impl std::fmt::Display for DecisionIngressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(ingress) => write!(
                f,
                "ingress {ingress:?} is not an ingress name (1 to 64 of a-z, 0-9, '.', '_' and '-')"
            ),
            Self::ReservedName => f.write_str(
                "ingress \"operator\" is built in (the stdio operator); declare another name",
            ),
            Self::UnsupportedAttests { ingress, attests } => write!(
                f,
                "ingress {ingress:?}: attests {attests:?} is not supported (attests \"person\")"
            ),
            Self::Duplicate(ingress) => write!(f, "ingress {ingress:?} is declared twice"),
            Self::SkewAboveOneHour {
                ingress,
                max_clock_skew_ms,
            } => write!(
                f,
                "ingress {ingress:?}: max_clock_skew_ms {max_clock_skew_ms} is more than one hour \
                 ({MAX_CLOCK_SKEW_MS})"
            ),
        }
    }
}

impl std::error::Error for DecisionIngressError {}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDecisionIngressDeclarations {
    #[serde(default)]
    ingresses: Vec<RawDecisionIngressDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDecisionIngressDeclaration {
    ingress: String,
    attests: String,
    max_clock_skew_ms: u64,
}

impl TryFrom<RawDecisionIngressDeclarations> for DecisionIngressDeclarations {
    type Error = DecisionIngressError;

    fn try_from(raw: RawDecisionIngressDeclarations) -> Result<Self, Self::Error> {
        let mut ingresses: Vec<DecisionIngressDeclaration> = Vec::new();
        for declaration in raw.ingresses {
            if declaration.ingress == OPERATOR_INGRESS {
                return Err(DecisionIngressError::ReservedName);
            }
            if !is_ingress_name(&declaration.ingress) {
                return Err(DecisionIngressError::InvalidName(declaration.ingress));
            }
            if declaration.attests != ATTESTS_PERSON {
                return Err(DecisionIngressError::UnsupportedAttests {
                    ingress: declaration.ingress,
                    attests: declaration.attests,
                });
            }
            if declaration.max_clock_skew_ms > MAX_CLOCK_SKEW_MS {
                return Err(DecisionIngressError::SkewAboveOneHour {
                    ingress: declaration.ingress,
                    max_clock_skew_ms: declaration.max_clock_skew_ms,
                });
            }
            if ingresses
                .iter()
                .any(|declared| declared.ingress == declaration.ingress)
            {
                return Err(DecisionIngressError::Duplicate(declaration.ingress));
            }
            ingresses.push(DecisionIngressDeclaration {
                ingress: declaration.ingress,
                max_clock_skew_ms: declaration.max_clock_skew_ms,
            });
        }
        Ok(Self { ingresses })
    }
}

impl From<DecisionIngressDeclarations> for RawDecisionIngressDeclarations {
    fn from(declarations: DecisionIngressDeclarations) -> Self {
        Self {
            ingresses: declarations
                .ingresses
                .into_iter()
                .map(|declaration| RawDecisionIngressDeclaration {
                    ingress: declaration.ingress,
                    attests: ATTESTS_PERSON.to_string(),
                    max_clock_skew_ms: declaration.max_clock_skew_ms,
                })
                .collect(),
        }
    }
}

/// Bind the launch's declared ingresses to its access configuration: an
/// approver map that names another ingress (the built-in `operator` aside)
/// is refused now, and so is every later edit that would name one.
///
/// # Errors
/// [`AccessConfigError::UnknownIngress`] for the first such ingress.
pub fn check_approver_ingresses(
    access: Option<&AccessController>,
    declarations: &DecisionIngressDeclarations,
) -> Result<(), AccessConfigError> {
    match access {
        Some(access) => access.declare_decision_ingresses(
            declarations
                .iter()
                .map(|declaration| declaration.ingress().to_string()),
        ),
        None => Ok(()),
    }
}
