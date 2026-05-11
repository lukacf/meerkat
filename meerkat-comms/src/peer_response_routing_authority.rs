//! Authority seam for inherited peer-response routing.
//!
//! Inbound peer requests carry a handling mode that terminal responses inherit
//! when the responder does not supply an explicit override. The runtime keeps
//! this as a narrow request/response routing fact instead of exposing a raw
//! map that shell code can reinterpret.

use meerkat_core::types::HandlingMode;
use meerkat_core::{PeerCorrelationId, ResponseStatus};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct PeerResponseRoutingAuthority {
    inherited_handling_modes: HashMap<PeerCorrelationId, HandlingMode>,
}

impl PeerResponseRoutingAuthority {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record_inbound_request(
        &mut self,
        corr_id: PeerCorrelationId,
        handling_mode: HandlingMode,
    ) {
        self.inherited_handling_modes.insert(corr_id, handling_mode);
    }

    pub(crate) fn response_handling_mode(
        &self,
        corr_id: PeerCorrelationId,
        status: ResponseStatus,
        explicit: Option<HandlingMode>,
    ) -> Option<HandlingMode> {
        explicit.or_else(|| {
            Self::is_terminal(status)
                .then(|| self.inherited_handling_modes.get(&corr_id).copied())
                .flatten()
        })
    }

    pub(crate) fn terminal_response_sent(&mut self, corr_id: PeerCorrelationId) {
        self.inherited_handling_modes.remove(&corr_id);
    }

    pub(crate) fn clear(&mut self, corr_id: PeerCorrelationId) {
        self.inherited_handling_modes.remove(&corr_id);
    }

    #[cfg(test)]
    pub(crate) fn contains_inbound_request(&self, corr_id: PeerCorrelationId) -> bool {
        self.inherited_handling_modes.contains_key(&corr_id)
    }

    fn is_terminal(status: ResponseStatus) -> bool {
        matches!(status, ResponseStatus::Completed | ResponseStatus::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_responses_inherit_recorded_request_mode() {
        let corr_id = PeerCorrelationId::from_uuid(uuid::Uuid::new_v4());
        let mut authority = PeerResponseRoutingAuthority::new();
        authority.record_inbound_request(corr_id, HandlingMode::Steer);

        assert_eq!(
            authority.response_handling_mode(corr_id, ResponseStatus::Accepted, None),
            None
        );
        assert_eq!(
            authority.response_handling_mode(corr_id, ResponseStatus::Completed, None),
            Some(HandlingMode::Steer)
        );
        assert_eq!(
            authority.response_handling_mode(
                corr_id,
                ResponseStatus::Completed,
                Some(HandlingMode::Queue)
            ),
            Some(HandlingMode::Queue)
        );

        authority.terminal_response_sent(corr_id);
        assert_eq!(
            authority.response_handling_mode(corr_id, ResponseStatus::Completed, None),
            None
        );
    }
}
