//! Pure, monotonic restriction composition for governed authorization.
//!
//! These values are policy inputs, not grants or authenticated permits. Owners
//! still validate issuers, current authority, relationships, source attributes
//! and operation entry. Identifiers are exact owner-declared values: no prefix,
//! wildcard, domain inference or arbitrary policy-expression implication is used.

use std::collections::BTreeSet;

use meerkat_core::auth::PrincipalRef;
use serde::{Deserialize, Serialize};

/// Why a required restriction cannot yet be evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedConstraint {
    /// The required fact was not supplied. This is not an unrestricted value.
    Absent,
    /// The fact is present but its meaning or value is unknown.
    Unknown,
    /// The required authority or fact is temporarily unavailable.
    Unavailable,
}

/// An exact set bound. An empty `Exact` set permits no values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "bound",
    content = "values",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SetBound<T: Ord> {
    Unrestricted,
    Exact(BTreeSet<T>),
}

impl<'de, T: Ord + Deserialize<'de>> Deserialize<'de> for SetBound<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Empty struct variants reject the reserved payload field even when
        // its value is null. Serde's adjacent unit decoder accepts that shape.
        #[derive(Deserialize)]
        #[serde(tag = "bound", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire<T: Ord> {
            Unrestricted {},
            Exact { values: BTreeSet<T> },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Unrestricted {} => Self::Unrestricted,
            Wire::Exact { values } => Self::Exact(values),
        })
    }
}

/// One conjunctive set restriction, including any unresolved contributing facts.
///
/// Known bounds are retained even when an unresolved fact prevents use. There
/// is intentionally no `Default`: an omitted field cannot become unrestricted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactRestriction<T: Ord> {
    bound: SetBound<T>,
    unresolved: BTreeSet<UnresolvedConstraint>,
}

/// Result of matching a value against bounds, not an authorization decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundMatch {
    Matches,
    OutsideBounds,
    Unresolved(BTreeSet<UnresolvedConstraint>),
}

impl<T: Ord> ExactRestriction<T> {
    /// The mathematical identity for conjunction; this does not issue a grant.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            bound: SetBound::Unrestricted,
            unresolved: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn exact(values: impl IntoIterator<Item = T>) -> Self {
        Self {
            bound: SetBound::Exact(values.into_iter().collect()),
            unresolved: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn unresolved(fact: UnresolvedConstraint) -> Self {
        Self {
            bound: SetBound::Unrestricted,
            unresolved: BTreeSet::from([fact]),
        }
    }

    /// Inspect bounds for diagnostics only; unresolved facts still prevent use.
    #[must_use]
    pub fn bound(&self) -> &SetBound<T> {
        &self.bound
    }

    #[must_use]
    pub fn unresolved_facts(&self) -> &BTreeSet<UnresolvedConstraint> {
        &self.unresolved
    }

    #[must_use]
    pub fn check(&self, value: &T) -> BoundMatch {
        if !self.unresolved.is_empty() {
            return BoundMatch::Unresolved(self.unresolved.clone());
        }
        if self.known_bound_matches(value) {
            BoundMatch::Matches
        } else {
            BoundMatch::OutsideBounds
        }
    }

    fn known_bound_matches(&self, value: &T) -> bool {
        match &self.bound {
            SetBound::Unrestricted => true,
            SetBound::Exact(values) => values.contains(value),
        }
    }
}

impl<T: Ord + Clone> ExactRestriction<T> {
    /// Retain both operands' restrictions. No operand can replace its parent.
    #[must_use]
    pub fn conjoin(&self, other: &Self) -> Self {
        let bound = match (&self.bound, &other.bound) {
            (SetBound::Unrestricted, bound) | (bound, SetBound::Unrestricted) => bound.clone(),
            (SetBound::Exact(left), SetBound::Exact(right)) => {
                SetBound::Exact(left.intersection(right).cloned().collect())
            }
        };
        Self {
            bound,
            unresolved: self.unresolved.union(&other.unresolved).copied().collect(),
        }
    }
}

/// An exact feature-owned action. Neither component supports wildcard matching.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRef {
    pub feature: String,
    pub action: String,
}

/// A resource namespace owned by the declared canonical authority.
///
/// Equal namespace text under different authorities denotes different domains.
/// Construction conveys a claim; owners must validate the declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDomain {
    pub authority: PrincipalRef,
    pub namespace: String,
}

/// An exact processing principal or authority-declared processing route.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "processor", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProcessorRef {
    Principal {
        principal: PrincipalRef,
    },
    Route {
        authority: PrincipalRef,
        route_id: String,
    },
}

/// An exact recipient or a destination governed by a separately validated contract.
///
/// A destination ID is not proof of its current audience, retention or account
/// binding. It cannot implicitly enable a dynamic-audience deployment profile.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "audience", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudienceRef {
    Principal {
        principal: PrincipalRef,
    },
    Destination {
        authority: PrincipalRef,
        destination_id: String,
    },
}

/// Lifetime bounds in Unix milliseconds. Windows are start-inclusive/end-exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "bound",
    content = "window",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LifetimeBound {
    Unrestricted,
    Window {
        not_before_ms: u64,
        expires_at_ms: u64,
    },
    Empty,
}

impl<'de> Deserialize<'de> for LifetimeBound {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Window {
            not_before_ms: u64,
            expires_at_ms: u64,
        }
        #[derive(Deserialize)]
        #[serde(tag = "bound", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Unrestricted {},
            Window { window: Window },
            Empty {},
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Unrestricted {} => Self::Unrestricted,
            Wire::Window { window } => Self::Window {
                not_before_ms: window.not_before_ms,
                expires_at_ms: window.expires_at_ms,
            },
            Wire::Empty {} => Self::Empty,
        })
    }
}

/// Lifetime conjunction preserving unresolved restrictions.
///
/// The raw projection cannot mutate a checked value:
/// ```compile_fail
/// use meerkat_authorization_contracts::constraints::LifetimeRestriction;
/// let mut lifetime = LifetimeRestriction::window(1, 2);
/// lifetime.expires_at_ms = 100;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct LifetimeRestriction {
    view: LifetimeRestrictionView,
}

/// Immutable projection of the exact bound, usable by generated field guards.
/// The scalar window fields are derived from `bound`, never decoded separately.
/// They are meaningful for `Window`; other variants retain their own semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LifetimeRestrictionView {
    pub bound: LifetimeBound,
    pub unresolved: BTreeSet<UnresolvedConstraint>,
    #[serde(skip)]
    pub not_before_ms: u64,
    #[serde(skip)]
    pub expires_at_ms: u64,
}

impl std::ops::Deref for LifetimeRestriction {
    type Target = LifetimeRestrictionView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl<'de> Deserialize<'de> for LifetimeRestriction {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            bound: LifetimeBound,
            unresolved: BTreeSet<UnresolvedConstraint>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::from_parts(wire.bound, wire.unresolved))
    }
}

impl LifetimeRestriction {
    fn from_parts(bound: LifetimeBound, unresolved: BTreeSet<UnresolvedConstraint>) -> Self {
        let (not_before_ms, expires_at_ms) = match bound {
            LifetimeBound::Unrestricted => (0, u64::MAX),
            LifetimeBound::Window {
                not_before_ms,
                expires_at_ms,
            } => (not_before_ms, expires_at_ms),
            LifetimeBound::Empty => (0, 0),
        };
        Self {
            view: LifetimeRestrictionView {
                bound,
                unresolved,
                not_before_ms,
                expires_at_ms,
            },
        }
    }

    #[must_use]
    pub fn bound(&self) -> LifetimeBound {
        self.bound
    }

    #[must_use]
    pub fn unresolved_facts(&self) -> &BTreeSet<UnresolvedConstraint> {
        &self.unresolved
    }

    #[must_use]
    pub fn unrestricted() -> Self {
        Self::from_parts(LifetimeBound::Unrestricted, BTreeSet::new())
    }

    /// Inverted or zero-width windows remain restrictive, as an empty lifetime.
    #[must_use]
    pub fn window(not_before_ms: u64, expires_at_ms: u64) -> Self {
        Self::from_parts(
            if not_before_ms < expires_at_ms {
                LifetimeBound::Window {
                    not_before_ms,
                    expires_at_ms,
                }
            } else {
                LifetimeBound::Empty
            },
            BTreeSet::new(),
        )
    }

    #[must_use]
    pub fn unresolved(fact: UnresolvedConstraint) -> Self {
        Self::from_parts(LifetimeBound::Unrestricted, BTreeSet::from([fact]))
    }

    #[must_use]
    pub fn conjoin(&self, other: &Self) -> Self {
        let bound = match (self.bound, other.bound) {
            (LifetimeBound::Empty, _) | (_, LifetimeBound::Empty) => LifetimeBound::Empty,
            (LifetimeBound::Unrestricted, bound) | (bound, LifetimeBound::Unrestricted) => bound,
            (
                LifetimeBound::Window {
                    not_before_ms: left_start,
                    expires_at_ms: left_end,
                },
                LifetimeBound::Window {
                    not_before_ms: right_start,
                    expires_at_ms: right_end,
                },
            ) => Self::window(left_start.max(right_start), left_end.min(right_end)).bound,
        };
        Self::from_parts(
            bound,
            self.unresolved.union(&other.unresolved).copied().collect(),
        )
    }

    #[must_use]
    pub fn check(&self, now_ms: u64) -> BoundMatch {
        if !self.unresolved.is_empty() {
            return BoundMatch::Unresolved(self.unresolved.clone());
        }
        if self.known_bound_matches(now_ms) {
            BoundMatch::Matches
        } else {
            BoundMatch::OutsideBounds
        }
    }

    fn known_bound_matches(&self, now_ms: u64) -> bool {
        match self.bound {
            LifetimeBound::Unrestricted => true,
            LifetimeBound::Window {
                not_before_ms,
                expires_at_ms,
            } => not_before_ms <= now_ms && now_ms < expires_at_ms,
            LifetimeBound::Empty => false,
        }
    }
}

/// Remaining child-grant edges. Zero permits current use but no new child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(
    tag = "bound",
    content = "edges",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum DepthBound {
    Unrestricted,
    Remaining(u32),
}

impl<'de> Deserialize<'de> for DepthBound {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "bound", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Unrestricted {},
            Remaining { edges: u32 },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Unrestricted {} => Self::Unrestricted,
            Wire::Remaining { edges } => Self::Remaining(edges),
        })
    }
}

/// Checked depth value with an immutable raw projection.
/// ```compile_fail
/// use meerkat_authorization_contracts::constraints::DelegationDepth;
/// let mut depth = DelegationDepth::remaining(0);
/// depth.remaining_edges = 100;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DelegationDepth {
    view: DelegationDepthView,
}

/// Immutable raw depth projection. `remaining_edges` is derived only from the
/// exact bound and is meaningful for `Remaining`, never for `Unrestricted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DelegationDepthView {
    pub bound: DepthBound,
    pub unresolved: BTreeSet<UnresolvedConstraint>,
    #[serde(skip)]
    pub remaining_edges: u64,
}

impl std::ops::Deref for DelegationDepth {
    type Target = DelegationDepthView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl<'de> Deserialize<'de> for DelegationDepth {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            bound: DepthBound,
            unresolved: BTreeSet<UnresolvedConstraint>,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self::from_parts(wire.bound, wire.unresolved))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DelegationFailure {
    #[error("delegation depth is exhausted")]
    Exhausted,
    #[error("delegation depth has unresolved authority facts")]
    Unresolved(BTreeSet<UnresolvedConstraint>),
}

impl DelegationDepth {
    fn from_parts(bound: DepthBound, unresolved: BTreeSet<UnresolvedConstraint>) -> Self {
        let remaining_edges = match bound {
            DepthBound::Unrestricted => 0,
            DepthBound::Remaining(edges) => u64::from(edges),
        };
        Self {
            view: DelegationDepthView {
                bound,
                unresolved,
                remaining_edges,
            },
        }
    }

    #[must_use]
    pub fn bound(&self) -> DepthBound {
        self.bound
    }

    #[must_use]
    pub fn unresolved_facts(&self) -> &BTreeSet<UnresolvedConstraint> {
        &self.unresolved
    }

    #[must_use]
    pub fn unrestricted() -> Self {
        Self::from_parts(DepthBound::Unrestricted, BTreeSet::new())
    }

    #[must_use]
    pub fn remaining(edges: u32) -> Self {
        Self::from_parts(DepthBound::Remaining(edges), BTreeSet::new())
    }

    #[must_use]
    pub fn unresolved(fact: UnresolvedConstraint) -> Self {
        Self::from_parts(DepthBound::Unrestricted, BTreeSet::from([fact]))
    }

    /// Compose bounds at the same depth; this does not consume a delegation edge.
    #[must_use]
    pub fn conjoin(&self, other: &Self) -> Self {
        let bound = match (self.bound, other.bound) {
            (DepthBound::Unrestricted, bound) | (bound, DepthBound::Unrestricted) => bound,
            (DepthBound::Remaining(left), DepthBound::Remaining(right)) => {
                DepthBound::Remaining(left.min(right))
            }
        };
        Self::from_parts(
            bound,
            self.unresolved.union(&other.unresolved).copied().collect(),
        )
    }

    /// A zero remaining depth still permits matching current-use bounds, but an
    /// unresolved depth does not. Neither result supplies execution authority.
    #[must_use]
    pub fn check_current_use(&self) -> BoundMatch {
        if self.unresolved.is_empty() {
            BoundMatch::Matches
        } else {
            BoundMatch::Unresolved(self.unresolved.clone())
        }
    }

    /// Compute a child's inherited depth. The grant owner must separately check
    /// permission to delegate and all current grant ancestors.
    ///
    /// # Errors
    /// Returns an error when depth is exhausted or unresolved.
    pub fn for_child(&self) -> Result<Self, DelegationFailure> {
        if !self.unresolved.is_empty() {
            return Err(DelegationFailure::Unresolved(self.unresolved.clone()));
        }
        match self.bound {
            DepthBound::Unrestricted => Ok(Self::unrestricted()),
            DepthBound::Remaining(0) => Err(DelegationFailure::Exhausted),
            DepthBound::Remaining(edges) => Ok(Self::remaining(edges - 1)),
        }
    }
}

/// Supported conjunctive restriction algebra, deliberately not an ABAC language.
///
/// These fields are Cartesian ceilings. Correlated action/resource alternatives
/// stay with their policy owner; projecting such a rule into independent sets
/// would discard meaning and is not a supported conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRestrictions {
    pub actions: ExactRestriction<ActionRef>,
    pub resource_domains: ExactRestriction<ResourceDomain>,
    pub processors: ExactRestriction<ProcessorRef>,
    pub audiences: ExactRestriction<AudienceRef>,
    pub lifetime: LifetimeRestriction,
    pub delegation_depth: DelegationDepth,
}

/// The exact values supplied by operation owners for pure bound matching.
#[derive(Debug, Clone, Copy)]
pub struct OperationRestrictionValues<'a> {
    pub action: &'a ActionRef,
    pub resource_domain: &'a ResourceDomain,
    pub processor: &'a ProcessorRef,
    pub audience: &'a AudienceRef,
    pub now_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestrictionDimension {
    Action,
    ResourceDomain,
    Processor,
    Audience,
    Lifetime,
    DelegationDepth,
}

/// A bound mismatch or missing fact. No hidden resource identifiers are exposed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RestrictionFailure {
    #[error("operation is outside the {dimension:?} bound")]
    OutsideBounds { dimension: RestrictionDimension },
    #[error("the {dimension:?} restriction has unresolved facts")]
    Unresolved {
        dimension: RestrictionDimension,
        facts: BTreeSet<UnresolvedConstraint>,
    },
}

impl ExecutionRestrictions {
    /// Explicit mathematical identity, never a missing-authority fallback.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self {
            actions: ExactRestriction::unrestricted(),
            resource_domains: ExactRestriction::unrestricted(),
            processors: ExactRestriction::unrestricted(),
            audiences: ExactRestriction::unrestricted(),
            lifetime: LifetimeRestriction::unrestricted(),
            delegation_depth: DelegationDepth::unrestricted(),
        }
    }

    #[must_use]
    pub fn conjoin(&self, other: &Self) -> Self {
        Self {
            actions: self.actions.conjoin(&other.actions),
            resource_domains: self.resource_domains.conjoin(&other.resource_domains),
            processors: self.processors.conjoin(&other.processors),
            audiences: self.audiences.conjoin(&other.audiences),
            lifetime: self.lifetime.conjoin(&other.lifetime),
            delegation_depth: self.delegation_depth.conjoin(&other.delegation_depth),
        }
    }

    /// Match every declared dimension without consulting mutable authority.
    /// Success is only a necessary condition for authorization, not a permit.
    ///
    /// # Errors
    /// Returns a typed failure for any excluded value or unresolved constraint.
    pub fn check_bounds(
        &self,
        values: OperationRestrictionValues<'_>,
    ) -> Result<(), RestrictionFailure> {
        self.bound_facts(values)
            .into_iter()
            .find_map(|(dimension, facts)| {
                if !facts.unresolved.is_empty() {
                    Some(RestrictionFailure::Unresolved {
                        dimension,
                        facts: facts.unresolved.clone(),
                    })
                } else if !facts.known_matches {
                    Some(RestrictionFailure::OutsideBounds { dimension })
                } else {
                    None
                }
            })
            .map_or(Ok(()), Err)
    }

    /// Report every known exclusion and unresolved fact, including both within
    /// one dimension. Conjunction must not lose a known empty ceiling merely
    /// because another contributor leaves that same dimension unresolved.
    /// This computes claims only, without consulting live authority.
    #[must_use]
    pub fn all_bound_failures(
        &self,
        values: OperationRestrictionValues<'_>,
    ) -> Vec<RestrictionFailure> {
        let mut failures = Vec::new();
        for (dimension, facts) in self.bound_facts(values) {
            if !facts.known_matches {
                failures.push(RestrictionFailure::OutsideBounds { dimension });
            }
            if !facts.unresolved.is_empty() {
                failures.push(RestrictionFailure::Unresolved {
                    dimension,
                    facts: facts.unresolved.clone(),
                });
            }
        }
        failures
    }

    fn bound_facts(
        &self,
        values: OperationRestrictionValues<'_>,
    ) -> [(RestrictionDimension, BoundFacts<'_>); 6] {
        [
            (
                RestrictionDimension::Action,
                BoundFacts {
                    known_matches: self.actions.known_bound_matches(values.action),
                    unresolved: &self.actions.unresolved,
                },
            ),
            (
                RestrictionDimension::ResourceDomain,
                BoundFacts {
                    known_matches: self
                        .resource_domains
                        .known_bound_matches(values.resource_domain),
                    unresolved: &self.resource_domains.unresolved,
                },
            ),
            (
                RestrictionDimension::Processor,
                BoundFacts {
                    known_matches: self.processors.known_bound_matches(values.processor),
                    unresolved: &self.processors.unresolved,
                },
            ),
            (
                RestrictionDimension::Audience,
                BoundFacts {
                    known_matches: self.audiences.known_bound_matches(values.audience),
                    unresolved: &self.audiences.unresolved,
                },
            ),
            (
                RestrictionDimension::Lifetime,
                BoundFacts {
                    known_matches: self.lifetime.known_bound_matches(values.now_ms),
                    unresolved: &self.lifetime.unresolved,
                },
            ),
            (
                RestrictionDimension::DelegationDepth,
                BoundFacts {
                    known_matches: true,
                    unresolved: &self.delegation_depth.unresolved,
                },
            ),
        ]
    }

    /// Compute attenuation only; this cannot issue or authenticate a child grant.
    ///
    /// # Errors
    /// Returns an error if the parent's remaining delegation depth is unresolved
    /// or exhausted. All other unresolved facts remain in the resulting bounds.
    pub fn for_child(&self, requested: &Self) -> Result<Self, DelegationFailure> {
        let inherited_depth = self.delegation_depth.for_child()?;
        let mut child = self.conjoin(requested);
        child.delegation_depth = inherited_depth.conjoin(&requested.delegation_depth);
        Ok(child)
    }
}

struct BoundFacts<'a> {
    known_matches: bool,
    unresolved: &'a BTreeSet<UnresolvedConstraint>,
}
