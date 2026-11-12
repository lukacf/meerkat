//! Reports of the provider-neutral `reasoning_effort` level a run's
//! requests were sent with.

use crate::{ReasoningEffortReport, ReasoningEffortResolution, ReasoningEffortSent};

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
