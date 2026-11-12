//! Per-request resolution of the provider-neutral `reasoning_effort` level,
//! and the report of what a run's requests were sent with.
//!
//! A request copy that carries the level is resolved here, right before it
//! is handed to the client that serves it: against that client's route
//! observation and the catalog row of the model the request names. The
//! level leaves the copy, the route's typed field is written in its place,
//! and the observation the level was resolved against stays on the copy as
//! the route witness, so the client can tell a route that changed in
//! between. A refusal leaves the copy as it was and reaches no client.
//!
//! One function serves every request path: the agent's request
//! preparation, the compaction summary call and the run-start report.

use crate::lifecycle::run_primitive::{ProviderParamsOverride, ProviderTag};
use crate::model_profile::capabilities::{EffortLevel, ModelCapabilities};
use crate::{
    AgentLlmClient, ModelProfileWitness, Provider, ReasoningEffortDecidedBy,
    ReasoningEffortRefusal, ReasoningEffortReport, ReasoningEffortResolution, ReasoningEffortSent,
    ReasoningResolveInput, ReasoningResolveMode, ReasoningRouteObservation,
    resolve_reasoning_effort,
};

/// What one request's level is resolved against: what the client that
/// serves the request observes about its route, the provider tag that
/// client applies when a request carries none, and the model the request
/// names with its catalog row.
pub(crate) struct RequestReasoningTarget<'a> {
    pub(crate) observation: Option<ReasoningRouteObservation>,
    pub(crate) default_tag: Option<ProviderTag>,
    pub(crate) provider: Provider,
    pub(crate) model: &'a str,
    pub(crate) catalog_row: Option<&'a ModelCapabilities>,
}

impl<'a> RequestReasoningTarget<'a> {
    /// The target of a request `client` serves for the model `profile`
    /// describes. Without a profile the client names its own provider and
    /// model, and the model has no catalog row.
    pub(crate) fn of<C>(client: &'a C, profile: Option<&'a ModelProfileWitness>) -> Self
    where
        C: AgentLlmClient + ?Sized,
    {
        Self {
            observation: client.reasoning_route(),
            default_tag: client.default_provider_tag(),
            provider: profile.map_or_else(|| client.provider(), ModelProfileWitness::provider),
            model: profile.map_or_else(|| client.model(), ModelProfileWitness::model),
            catalog_row: profile.and_then(ModelProfileWitness::catalog_capabilities),
        }
    }

    fn input(
        &self,
        requested: EffortLevel,
        decided_by: ReasoningEffortDecidedBy,
    ) -> ReasoningResolveInput<'_> {
        ReasoningResolveInput::new(
            requested,
            self.provider,
            self.model,
            self.observation.as_ref(),
            self.catalog_row,
            decided_by,
        )
    }
}

/// `params` as the client will send them, for judging a level: a copy that
/// carries no provider tag takes the client's default tag, the tag the
/// client would otherwise apply by itself, so the level is written beside
/// the client's own fields and its conflicts are judged on what is sent.
pub(crate) fn with_client_default_tag(
    params: &mut Option<ProviderParamsOverride>,
    default_tag: Option<&ProviderTag>,
) {
    let Some(default_tag) = default_tag else {
        return;
    };
    let params = params.get_or_insert_with(ProviderParamsOverride::default);
    if params.provider_tag.is_none() {
        params.provider_tag = Some(default_tag.clone());
    }
}

/// Resolve the level `params`, one request's own copy, carries for
/// `target`. `Ok(None)` when the copy carries no level: the copy is not
/// touched.
///
/// `session_level` is the level the session itself is configured with. A
/// copy whose level differs from it carries the level a reasoning
/// preference put in its place for this turn: that level is resolved the
/// way a preference is (it replaces a provider-specific effort, and it
/// never fails the request), and when the route does not take it the
/// session's own level is resolved instead, strictly.
///
/// On success the copy carries the route's typed field, no neutral level,
/// and the observation the level was resolved against as its route witness.
/// On refusal the copy is as it was.
pub(crate) fn resolve_request_reasoning_effort(
    target: &RequestReasoningTarget<'_>,
    session_level: Option<EffortLevel>,
    params: &mut Option<ProviderParamsOverride>,
) -> Result<Option<ReasoningEffortResolution>, Box<ReasoningEffortRefusal>> {
    let Some(level) = params.as_ref().and_then(|params| params.reasoning_effort) else {
        return Ok(None);
    };
    let mut copy = params.clone();
    if let Some(copy) = copy.as_mut() {
        copy.reasoning_effort = None;
    }
    with_client_default_tag(&mut copy, target.default_tag.as_ref());

    let preferred_over = session_level.filter(|session_level| *session_level != level);
    let preferred = preferred_over.and_then(|session_level| {
        let mut preferred_copy = copy.clone();
        resolve_reasoning_effort(
            target.input(level, ReasoningEffortDecidedBy::Preference),
            &mut preferred_copy,
            ReasoningResolveMode::Preference,
        )
        .ok()
        .map(|mut resolution| {
            // The request named the session's level; the preference
            // decided the level that is sent.
            resolution.requested = session_level;
            (resolution, preferred_copy)
        })
    });
    let resolution = match preferred {
        Some((resolution, preferred_copy)) => {
            copy = preferred_copy;
            resolution
        }
        None => resolve_reasoning_effort(
            target.input(
                preferred_over.unwrap_or(level),
                ReasoningEffortDecidedBy::Request,
            ),
            &mut copy,
            ReasoningResolveMode::Strict,
        )
        .map_err(Box::new)?,
    };
    if let Some(copy) = copy.as_mut() {
        copy.reasoning_route_witness = target.observation.clone();
    }
    *params = copy;
    Ok(Some(resolution))
}

/// Count one provider request sent with `resolution` into a run's report:
/// one more request on the row of the same resolution, or a new row, in
/// first-use order.
pub(crate) fn record_sent_reasoning_effort(
    report: &mut Option<ReasoningEffortReport>,
    resolution: &ReasoningEffortResolution,
) {
    let report = report.get_or_insert_with(|| ReasoningEffortReport {
        requested: resolution.requested,
        sent: Vec::new(),
    });
    let same = |row: &ReasoningEffortSent| {
        row.level == resolution.level
            && row.provider == resolution.provider
            && row.model == resolution.model
            && row.route == resolution.route
            && row.fields == resolution.fields
            && row.facts == resolution.facts
            && row.decided_by == resolution.decided_by
    };
    match report.sent.iter_mut().find(|row| same(row)) {
        Some(row) => row.requests = row.requests.saturating_add(1),
        None => report.sent.push(ReasoningEffortSent {
            level: resolution.level,
            provider: resolution.provider,
            model: resolution.model.clone(),
            route: resolution.route.clone(),
            fields: resolution.fields.clone(),
            facts: resolution.facts,
            decided_by: resolution.decided_by,
            requests: 1,
        }),
    }
}
