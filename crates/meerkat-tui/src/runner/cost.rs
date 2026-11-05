//! The cost readout on the runner: the price table (overrides, endpoint
//! prices, the community list refreshed in the background) and the
//! per-session token totals by model, persisted in the session's cost
//! record ([`super::cost_store`]).
//!
//! The UI counts every `TurnCompleted` usage under the model the provider
//! accounted it to (the identity `TurnUsage` carries), every classifier
//! call under the classifier model and every compaction (estimated input
//! plus the summary), and sends the totals as
//! [`crate::ui::Effect::CostSnapshot`]; the runner stores them next to the
//! session. A resumed session's record is reconciled with its authoritative
//! cumulative usage: usage the record does not account for is attributed to
//! the session's model and the readout becomes approximate (`~`). A session
//! the TUI never recorded (created by another surface) starts from its
//! cumulative usage, approximate.

use std::path::PathBuf;
use std::sync::Arc;

use meerkat_core::Session;
use tokio::task::JoinHandle;

use super::Runner;
use super::cost_store::{load_cost, save_cost};
use crate::pricing::{CommunityPrices, FETCH_TIMEOUT, ModelPrice, PricingError, refresh_community};
use crate::ui::{Msg, SessionCost, SessionCostSnapshot, StatusUpdate};

/// A community price-list refresh running in the background.
pub(super) type PriceFetch = JoinHandle<Result<CommunityPrices, PricingError>>;

/// Start refreshing the community list from `url` (cached at `cache`).
pub(super) fn spawn_price_fetch(url: String, cache: Option<PathBuf>) -> PriceFetch {
    tokio::spawn(async move { refresh_community(&url, cache.as_deref(), FETCH_TIMEOUT).await })
}

/// The cost totals of `session`: its record reconciled with its
/// authoritative cumulative usage, else that usage attributed to `model`
/// (approximate).
pub(super) fn session_cost(
    realm_root: &std::path::Path,
    session: &Session,
    model: &str,
) -> SessionCost {
    let usage = session.total_usage();
    match load_cost(realm_root, &session.id().to_string()) {
        Some(mut recorded) => {
            recorded.reconcile(model, &usage);
            recorded
        }
        None => SessionCost::derived(model, &usage),
    }
}

impl Runner {
    /// Send the current price table to the UI.
    pub(super) fn push_prices(&self) {
        self.send(Msg::Status(StatusUpdate::Prices(Arc::clone(&self.prices))));
    }

    /// Replace the endpoint (LiteLLM `/model/info`) prices and tell the UI.
    pub(super) fn set_endpoint_prices(&mut self, prices: Vec<(String, ModelPrice)>) {
        if prices.is_empty() && self.prices.is_empty() {
            return;
        }
        Arc::make_mut(&mut self.prices).set_endpoint_prices(prices);
        self.push_prices();
    }

    /// The community refresh finished: use the new list (the task wrote the
    /// cache) or keep the old one.
    pub(super) fn price_fetch_finished(
        &mut self,
        joined: Result<Result<CommunityPrices, PricingError>, tokio::task::JoinError>,
    ) {
        self.price_fetch = None;
        match joined {
            Ok(Ok(community)) => {
                tracing::debug!(models = community.len(), "community price list refreshed");
                Arc::make_mut(&mut self.prices).set_community(community);
                self.push_prices();
            }
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "could not fetch the community price list");
            }
            Err(err) => tracing::warn!(error = %err, "price-list task failed"),
        }
    }

    /// Answer [`crate::ui::Effect::CostSnapshot`]: store the totals next to
    /// the live session.
    pub(super) fn save_cost_snapshot(&self, snapshot: &SessionCostSnapshot) {
        if let Err(err) = save_cost(
            &self.runtime.scope.realm_root(),
            &self.session.id().to_string(),
            snapshot,
        ) {
            tracing::warn!(error = %err, "could not record the session's cost");
        }
    }
}
