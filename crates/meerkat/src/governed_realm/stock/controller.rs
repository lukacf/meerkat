//! Stock rkat's controller pin: the actual selected controller client a
//! session's governed binding was made with, the one the stock factory built
//! for the session and its host passed to [`super::StockGovernance::bind_prompt`],
//! [`super::StockGovernance::bind_caller_prompt`] or
//! [`super::StockGovernance::bind_host_outcome`]. The ingress pins that client
//! on the input it admits (`NativeIngressContext::with_controller_client`).
//!
//! The stock composition keeps each session runtime's newest pin (the
//! client of the binding with the highest destination generation) and hands
//! it, for a retained resume of that runtime's work only, to the scoped
//! original composition's retained-resume check as its
//! [`RetainedControllerOwner`]. The pin is the client itself, never rebuilt
//! from a selection: a request for any other selection gets no client.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_core::authorization::ControllerReadinessFailure;
use meerkat_core::{ControllerModelClient, ControllerModelSelection};
use meerkat_runtime::meerkat_machine::RetainedControllerOwner;
use meerkat_runtime::{LogicalRuntimeId, RuntimeSessionRegistrationWitness};

/// The route report of a pinned controller client, defined in
/// [`crate::controller_route`] and re-exported here unchanged.
pub use crate::controller_route::{
    ControllerRouteReport, controller_route_key, controller_route_report,
};

pub struct StockControllerPin {
    client: ControllerModelClient,
}

impl StockControllerPin {
    pub fn new(client: ControllerModelClient) -> Arc<Self> {
        Arc::new(Self { client })
    }

    /// The pinned client, for the ingress pin of each admitted input.
    pub fn client(&self) -> &ControllerModelClient {
        &self.client
    }
}

impl RetainedControllerOwner for StockControllerPin {
    fn reacquire_controller(
        &self,
        selection: &ControllerModelSelection,
    ) -> Result<Option<ControllerModelClient>, ControllerReadinessFailure> {
        Ok((self.client.selection() == selection).then(|| self.client.clone()))
    }
}

/// The controller pins of one stock composition's sessions, for retained
/// resume only: per session runtime, the pin of the client the session's
/// newest governed binding (the highest destination generation) was made
/// with, kept for the exact registration that binding was made on.
///
/// A capability cache, not lifecycle state: it holds no authority, decides
/// nothing and records no fact of any owner. A governed binding replaces
/// its session's pin only with a newer destination generation, so a binding
/// that finishes after a later one never displaces it. A pin is served only
/// while the runtime entry of the registration it was made on exists
/// (`RuntimeSessionRegistrationWitness::has_ended`): once that entry is
/// gone it is never served again, and it is removed at the next binding or
/// resume check, so the cache does not grow with dead sessions in a
/// long-running host. Only the composition's retained-resume check reads it:
/// no authority decision does, the pin answers no selection but its own
/// client's, and a resume whose destination its session has since moved is
/// refused by the runtime before the check asks it.
#[derive(Default)]
pub(crate) struct StockResumePins {
    pins: std::sync::Mutex<HashMap<LogicalRuntimeId, RegistrationPin>>,
}

/// One session runtime's pin, the registration it was made on and the
/// destination generation that binding bound.
struct RegistrationPin {
    registration: RuntimeSessionRegistrationWitness,
    generation: u64,
    pin: Arc<StockControllerPin>,
}

impl StockResumePins {
    /// Pin `client` for the session `registration` registers, whose binding
    /// bound destination `generation`. It replaces that session's pin only
    /// when `generation` is newer than the pinned binding's. Every pin whose
    /// registration has ended is dropped.
    pub(crate) fn record(
        &self,
        registration: RuntimeSessionRegistrationWitness,
        generation: u64,
        client: ControllerModelClient,
    ) {
        let runtime = LogicalRuntimeId::for_session(registration.session_id());
        let mut pins = self.lock();
        pins.retain(|_, pinned| !pinned.registration.has_ended());
        if pins
            .get(&runtime)
            .is_some_and(|pinned| pinned.generation >= generation)
        {
            return;
        }
        pins.insert(
            runtime,
            RegistrationPin {
                registration,
                generation,
                pin: StockControllerPin::new(client),
            },
        );
    }

    /// The retained-resume controller owner of `runtime`: its session's pin
    /// while the runtime entry of the registration it was made on exists,
    /// otherwise one that has no client for any selection. The check then
    /// answers `Readiness(UnsupportedScope)`: a fresh resume stays owed, while
    /// a kept resume (its client did not survive the store, and a pin is
    /// never carried over to a later registration) ends as a refused resume
    /// at once. An ended pin is removed here.
    pub(crate) fn controller_owner(
        &self,
        runtime: &LogicalRuntimeId,
    ) -> Arc<dyn RetainedControllerOwner> {
        let mut pins = self.lock();
        if pins
            .get(runtime)
            .is_some_and(|pinned| pinned.registration.has_ended())
        {
            pins.remove(runtime);
        }
        let current = pins
            .get(runtime)
            .map(|pinned| Arc::clone(&pinned.pin) as Arc<dyn RetainedControllerOwner>);
        drop(pins);
        current.unwrap_or_else(|| Arc::new(Unpinned))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<LogicalRuntimeId, RegistrationPin>> {
        self.pins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A session runtime with no current pin: no client for any selection.
struct Unpinned;

impl RetainedControllerOwner for Unpinned {
    fn reacquire_controller(
        &self,
        _selection: &ControllerModelSelection,
    ) -> Result<Option<ControllerModelClient>, ControllerReadinessFailure> {
        Ok(None)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    struct Client(ControllerModelSelection);

    /// A client that reports route facts, for the selection it carries.
    struct Routed {
        reported: Option<ControllerModelSelection>,
        facts_for: ControllerModelSelection,
        endpoint: &'static str,
    }

    #[async_trait::async_trait]
    impl meerkat_core::AgentLlmClient for Routed {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            self.reported.clone()
        }
        fn controller_model_facts(
            &self,
        ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable>
        {
            Ok(meerkat_core::ControllerModelFacts::new(
                self.facts_for.clone(),
                Arc::from(self.endpoint),
                Arc::from("wire-model"),
            ))
        }
        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            Err(meerkat_core::AgentError::ConfigError(
                "route fixture".into(),
            ))
        }
        fn provider(&self) -> meerkat_core::Provider {
            self.facts_for.provider()
        }
        fn model(&self) -> &str {
            self.facts_for.model()
        }
    }

    #[test]
    fn the_route_key_comes_from_the_pinned_clients_own_facts() {
        const ENDPOINT: &str = "https://models.invalid/v1/messages";
        let pinned = selection("operator");
        let routed = |reported: Option<ControllerModelSelection>,
                      facts_for: ControllerModelSelection,
                      endpoint: &'static str| {
            ControllerModelClient::new(
                pinned.clone(),
                Arc::new(Routed {
                    reported,
                    facts_for,
                    endpoint,
                }),
            )
        };
        let actual = routed(Some(pinned.clone()), pinned.clone(), ENDPOINT);
        let expected = meerkat_core::ControllerModelFacts::new(
            pinned.clone(),
            Arc::from(ENDPOINT),
            Arc::from("wire-model"),
        )
        .route_key();
        assert_eq!(controller_route_key(&actual), Some(expected.clone()));
        let report = controller_route_report(&actual).expect("report");
        assert_eq!(report.provider, "anthropic");
        assert_eq!(report.model, "claude-sonnet-4-5");
        assert_eq!(report.wire_model, "wire-model");
        assert_eq!(report.endpoint, ENDPOINT);
        assert_eq!(report.credential_owner, "account stock/operator");
        let shown = report.to_string();
        assert!(shown.contains(expected.as_str()), "{shown}");

        // The pin's native read refuses, so there is no report: facts for
        // another selection, a client that independently reports another
        // selection or none, or a client without facts.
        let other = selection("someone-else");
        for refused in [
            routed(Some(pinned.clone()), other.clone(), ENDPOINT),
            routed(Some(other), pinned.clone(), ENDPOINT),
            routed(None, pinned.clone(), ENDPOINT),
            ControllerModelClient::new(pinned.clone(), Arc::new(Client(pinned.clone()))),
        ] {
            assert_eq!(controller_route_report(&refused), None);
        }

        // Userinfo is never shown; the key still names the actual endpoint.
        const WITH_USERINFO: &str =
            "https://user:secret-canary@models.invalid/v1/messages?key=query-canary";
        let with_userinfo = routed(Some(pinned.clone()), pinned.clone(), WITH_USERINFO);
        let report = controller_route_report(&with_userinfo).expect("report");
        assert_eq!(report.endpoint, ENDPOINT);
        let shown = report.to_string();
        assert!(
            !shown.contains("secret-canary") && !shown.contains("query-canary"),
            "{shown}"
        );
        assert_eq!(
            report.key,
            meerkat_core::ControllerModelFacts::new(
                pinned.clone(),
                Arc::from(WITH_USERINFO),
                Arc::from("wire-model"),
            )
            .route_key()
        );
        assert_ne!(report.key, expected);
    }

    #[async_trait::async_trait]
    impl meerkat_core::AgentLlmClient for Client {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            Some(self.0.clone())
        }
        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            Err(meerkat_core::AgentError::ConfigError("pin fixture".into()))
        }
        fn provider(&self) -> meerkat_core::Provider {
            self.0.provider()
        }
        fn model(&self) -> &str {
            self.0.model()
        }
    }

    fn selection(account: &str) -> ControllerModelSelection {
        ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: "claude-sonnet-4-5".into(),
                provider: meerkat_core::Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(serde_json::json!({"realm": "stock", "account": account}))
                .expect("credential identity"),
            "profile".into(),
            "transport".into(),
        )
    }

    #[test]
    fn the_pin_answers_only_its_own_selection_with_the_actual_client() {
        let pinned = selection("operator");
        let client = ControllerModelClient::new(pinned.clone(), Arc::new(Client(pinned.clone())));
        let pin = StockControllerPin::new(client.clone());
        let reacquired = pin
            .reacquire_controller(&pinned)
            .expect("answerable")
            .expect("the pinned selection");
        assert!(Arc::ptr_eq(reacquired.client(), client.client()));
        assert!(Arc::ptr_eq(pin.client().client(), client.client()));
        assert!(
            pin.reacquire_controller(&selection("someone-else"))
                .expect("answerable")
                .is_none(),
            "no client is built for another selection"
        );
    }

    /// The pins serve, per session runtime, only the client of its newest
    /// binding (a binding that finishes after a later one never displaces
    /// it, and a newer binding always replaces it), only for that runtime
    /// and that selection, and nothing once the registration's runtime entry
    /// is gone.
    #[tokio::test]
    async fn resume_pins_serve_only_the_newest_binding_of_a_live_registration() {
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let session = meerkat_core::SessionId::new();
        let other = meerkat_core::SessionId::new();
        for id in [&session, &other] {
            machine
                .register_session(id.clone())
                .await
                .expect("registered");
        }
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .expect("current registration");
        let pinned = selection("operator");
        let client =
            || ControllerModelClient::new(pinned.clone(), Arc::new(Client(pinned.clone())));
        let (newer, older) = (client(), client());
        let pins = StockResumePins::default();
        pins.record(registration.clone(), 9, newer.clone());
        pins.record(registration.clone(), 5, older);
        let runtime = LogicalRuntimeId::for_session(&session);
        let served = pins
            .controller_owner(&runtime)
            .reacquire_controller(&pinned)
            .expect("answerable")
            .expect("the newest binding's client");
        assert!(
            Arc::ptr_eq(served.client(), newer.client()),
            "the binding of generation 5 never displaces the one of generation 9"
        );
        let newest = client();
        pins.record(registration.clone(), 12, newest.clone());
        let served = pins
            .controller_owner(&runtime)
            .reacquire_controller(&pinned)
            .expect("answerable")
            .expect("the newest binding's client");
        assert!(
            Arc::ptr_eq(served.client(), newest.client()),
            "the binding of generation 12 replaces the pin of generation 9"
        );
        assert!(
            pins.controller_owner(&runtime)
                .reacquire_controller(&selection("someone-else"))
                .expect("answerable")
                .is_none(),
            "no client for another selection"
        );
        assert!(
            pins.controller_owner(&LogicalRuntimeId::for_session(&other))
                .reacquire_controller(&pinned)
                .expect("answerable")
                .is_none(),
            "no client for another session's runtime"
        );

        machine
            .unregister_session(&session)
            .await
            .expect("unregistered");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !registration.has_ended() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the registration's runtime entry is gone");
        assert!(
            pins.controller_owner(&runtime)
                .reacquire_controller(&pinned)
                .expect("answerable")
                .is_none(),
            "no client once the registration's runtime entry is gone"
        );
    }
}
