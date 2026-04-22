//! Composition driver execution framework.
//!
//! Compositions that need cross-machine orchestration declare a driver in
//! their `CompositionSchema` (see
//! [`meerkat_machine_schema::CompositionDriver`]). The driver watches
//! specific effect variants on one or more producer machines and, given an
//! observed effect, returns a set of typed inputs the dispatcher routes to
//! target machines.
//!
//! This module is the runtime counterpart to the declarative descriptor.
//! It is deliberately generic: it does not know about MobMachine,
//! MeerkatMachine, or any specific effect variant. Concrete drivers live
//! elsewhere and implement [`CompositionDriverTrait`]; the dispatcher
//! holds them behind `Arc<dyn CompositionDriverTrait>`.
//!
//! Commit 1 of the Track-B peer-graph/wiring-graph unification ships this
//! framework plus a trivial no-op driver proving the end-to-end path:
//! schema → declarative descriptor → runtime registration → dispatch →
//! routed inputs. Real drivers (e.g. `RecomputeMobPeerOverlay`) land on
//! this foundation in subsequent commits.
//!
//! Design notes:
//!
//! - Drivers are **pure decision functions over observed effects**. They
//!   must not mutate shared runtime state; they may only return dispatched
//!   inputs. The caller is responsible for routing the dispatched inputs
//!   to the target machines (typically via each machine's authority-apply
//!   seam).
//! - `ObservedEffect` and `DispatchedInput` carry typed descriptors plus a
//!   `serde_json::Value` payload. The payload is the wire-shape the
//!   producer emitted / the target consumes. Concrete drivers parse
//!   payloads via `serde` at the point of use — the dispatcher itself
//!   stays schema-agnostic.
//! - Driver errors are typed through [`DriverError`] and surfaced up to
//!   the caller unchanged — the dispatcher does not swallow failures.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use meerkat_machine_schema::{
    CompositionDriver, CompositionSchema, DriverDispatchRoute, RouteTargetKind, WatchedEffect,
};
use serde_json::Value;

/// An effect observation the dispatcher passes to a driver.
///
/// `producer_instance` and `effect_variant` match the declarative
/// [`WatchedEffect`] entries on the composition driver descriptor.
/// `payload` is the serialized effect body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedEffect {
    pub producer_instance: String,
    pub effect_variant: String,
    pub payload: Value,
}

impl ObservedEffect {
    pub fn new(
        producer_instance: impl Into<String>,
        effect_variant: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            producer_instance: producer_instance.into(),
            effect_variant: effect_variant.into(),
            payload,
        }
    }
}

/// An input the driver dispatches, to be routed to a target machine.
///
/// `route_name` matches a declared [`DriverDispatchRoute`] on the driver
/// descriptor. `payload` is the serialized input body the target machine
/// will consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchedInput {
    pub route_name: String,
    pub payload: Value,
}

impl DispatchedInput {
    pub fn new(route_name: impl Into<String>, payload: Value) -> Self {
        Self {
            route_name: route_name.into(),
            payload,
        }
    }
}

/// Typed driver error. Drivers return `Result<_, DriverError>` so the
/// dispatcher can surface structured failures without panicking.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DriverError {
    /// The driver received an effect it did not declare as a watched
    /// effect. Usually indicates a routing bug in the dispatcher or a
    /// mismatch between declarative schema and implementation.
    #[error(
        "driver `{driver}` received unexpected effect `{producer}::{effect}`: not in watched set"
    )]
    UnexpectedEffect {
        driver: String,
        producer: String,
        effect: String,
    },
    /// The driver wanted to emit a dispatched input whose route name is
    /// not declared in the composition descriptor. Always a driver bug.
    #[error("driver `{driver}` emitted input on undeclared dispatch route `{route}`")]
    UnknownDispatchRoute { driver: String, route: String },
    /// The driver failed for a domain-specific reason. Opaque message;
    /// callers may log this but should not attempt recovery generically.
    #[error("driver `{driver}` decision failed: {reason}")]
    DecisionFailed { driver: String, reason: String },
    /// The payload JSON on an observed effect could not be parsed into
    /// the shape the driver expected.
    #[error("driver `{driver}` could not parse effect payload: {reason}")]
    PayloadParseError { driver: String, reason: String },
}

/// Contract a concrete composition driver implements.
///
/// Drivers are expected to be pure: given an observed effect (and a
/// read-only snapshot of any state they need from the calling context),
/// return the set of dispatched inputs the dispatcher should route.
pub trait CompositionDriverTrait: Send + Sync + std::fmt::Debug {
    /// Logical name of the driver. Must match the `name` field on the
    /// descriptor.
    fn name(&self) -> &str;

    /// Invoked by the dispatcher for each observed watched effect.
    ///
    /// Implementations return `Ok(Vec<DispatchedInput>)` on success; the
    /// vector may be empty if the observation does not trigger any
    /// dispatch. Any error is surfaced to the caller.
    fn on_effect(&self, effect: &ObservedEffect) -> Result<Vec<DispatchedInput>, DriverError>;
}

/// Thin wrapper over the declarative descriptor + concrete driver.
#[derive(Debug)]
pub struct RegisteredDriver {
    descriptor: CompositionDriver,
    driver: Arc<dyn CompositionDriverTrait>,
    watched_index: HashSet<(String, String)>,
    dispatch_index: HashMap<String, DriverDispatchRoute>,
}

impl RegisteredDriver {
    /// Returns the declared watched effects for this driver.
    pub fn watched_effects(&self) -> &[WatchedEffect] {
        &self.descriptor.watched_effects
    }

    /// Returns the declared dispatch routes for this driver.
    pub fn dispatch_routes(&self) -> &[DriverDispatchRoute] {
        &self.descriptor.dispatch_routes
    }

    /// Returns the driver's logical name.
    pub fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn watches(&self, producer: &str, variant: &str) -> bool {
        self.watched_index
            .contains(&(producer.to_string(), variant.to_string()))
    }

    fn route(&self, route_name: &str) -> Option<&DriverDispatchRoute> {
        self.dispatch_index.get(route_name)
    }
}

/// Failure modes that surface while registering or dispatching.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DispatcherError {
    /// The composition has no driver descriptor to register.
    #[error("composition `{composition}` has no driver declared in its schema")]
    CompositionHasNoDriver { composition: String },
    /// The driver name does not match the descriptor name.
    #[error(
        "driver name mismatch: descriptor declares `{declared}`, implementation reports `{impl_name}`"
    )]
    DriverNameMismatch { declared: String, impl_name: String },
    /// Two drivers have been registered under the same logical name.
    #[error("driver `{driver}` already registered")]
    DuplicateDriver { driver: String },
    /// Propagated driver failure.
    #[error(transparent)]
    Driver(#[from] DriverError),
}

/// Routes observed effects to the drivers that declared interest.
///
/// A dispatcher holds zero or more registered drivers. When an observed
/// effect arrives via [`dispatch_effect`], each driver whose
/// `watched_effects` include this `(producer, variant)` pair is invoked,
/// and the union of their dispatched inputs is returned to the caller.
///
/// The dispatcher performs shape validation on driver output: every
/// dispatched input's `route_name` must match a declared route on the
/// driver descriptor. Unknown route names surface as
/// [`DriverError::UnknownDispatchRoute`] failures through
/// [`DispatcherError::Driver`].
#[derive(Debug, Default)]
pub struct CompositionDispatcher {
    drivers: HashMap<String, RegisteredDriver>,
}

/// One complete dispatch — the target route plus the input payload.
/// Returned to the caller for routing to the target machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedDispatch {
    /// Name of the driver that produced this dispatch.
    pub driver_name: String,
    /// Target machine instance id.
    pub target_instance: String,
    /// Whether the dispatch lands on an input or a signal.
    pub target_kind: RouteTargetKind,
    /// Target variant name on the instance's input/signal enum.
    pub target_variant: String,
    /// Payload the driver emitted.
    pub payload: Value,
}

impl CompositionDispatcher {
    /// Construct an empty dispatcher. Drivers are registered explicitly
    /// via [`register_driver`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a concrete driver implementation against the declarative
    /// descriptor pulled from `composition.driver`. The descriptor is
    /// cloned into the dispatcher so future schema mutations do not
    /// affect the registration.
    pub fn register_driver(
        &mut self,
        composition: &CompositionSchema,
        driver: Arc<dyn CompositionDriverTrait>,
    ) -> Result<(), DispatcherError> {
        let descriptor =
            composition
                .driver
                .clone()
                .ok_or_else(|| DispatcherError::CompositionHasNoDriver {
                    composition: composition.name.clone(),
                })?;
        if descriptor.name != driver.name() {
            return Err(DispatcherError::DriverNameMismatch {
                declared: descriptor.name,
                impl_name: driver.name().to_string(),
            });
        }
        if self.drivers.contains_key(&descriptor.name) {
            return Err(DispatcherError::DuplicateDriver {
                driver: descriptor.name,
            });
        }

        let mut watched_index = HashSet::new();
        for watched in &descriptor.watched_effects {
            watched_index.insert((
                watched.producer_instance.clone(),
                watched.effect_variant.clone(),
            ));
        }
        let mut dispatch_index = HashMap::new();
        for dispatch in &descriptor.dispatch_routes {
            dispatch_index.insert(dispatch.name.clone(), dispatch.clone());
        }

        let name = descriptor.name.clone();
        let registered = RegisteredDriver {
            descriptor,
            driver,
            watched_index,
            dispatch_index,
        };
        self.drivers.insert(name, registered);
        Ok(())
    }

    /// Returns the logical names of all registered drivers.
    pub fn registered_driver_names(&self) -> Vec<&str> {
        self.drivers.keys().map(String::as_str).collect()
    }

    /// Returns the registered driver by name, if any.
    pub fn driver(&self, name: &str) -> Option<&RegisteredDriver> {
        self.drivers.get(name)
    }

    /// Dispatch one observed effect. Returns the routed dispatches
    /// emitted by all interested drivers.
    ///
    /// Any driver returning [`DriverError`] short-circuits dispatch and
    /// surfaces through [`DispatcherError::Driver`] — the caller sees the
    /// first error, other drivers in the same invocation are not run.
    pub fn dispatch_effect(
        &self,
        effect: &ObservedEffect,
    ) -> Result<Vec<RoutedDispatch>, DispatcherError> {
        let mut out = Vec::new();
        for driver in self.drivers.values() {
            if !driver.watches(&effect.producer_instance, &effect.effect_variant) {
                continue;
            }

            let dispatched = driver.driver.on_effect(effect)?;
            for dispatched_input in dispatched {
                let Some(route) = driver.route(&dispatched_input.route_name) else {
                    return Err(DispatcherError::Driver(DriverError::UnknownDispatchRoute {
                        driver: driver.name().to_string(),
                        route: dispatched_input.route_name,
                    }));
                };
                out.push(RoutedDispatch {
                    driver_name: driver.name().to_string(),
                    target_instance: route.target_instance.clone(),
                    target_kind: route.target_kind,
                    target_variant: route.input_variant.clone(),
                    payload: dispatched_input.payload,
                });
            }
        }
        Ok(out)
    }
}

// --------------------------------------------------------------------------
// No-op driver — the framework's first built-in consumer.
//
// The no-op driver is installed in tests to prove the declarative-schema →
// registration → dispatch path works end-to-end without pulling in a real
// domain-specific driver. It also serves as a living reference for authors
// implementing `CompositionDriverTrait`.
//
// It records every effect it observes (so tests can assert dispatch
// actually reached the driver) and, optionally, emits a single dispatched
// input on a caller-specified route.
// --------------------------------------------------------------------------

/// Trivial no-op driver. Records observations; optionally emits a single
/// dispatched input on `emit_on_route` with `emit_payload` for each
/// observed effect.
///
/// Used as the framework's first consumer in tests and as a reference
/// implementation for [`CompositionDriverTrait`].
#[derive(Debug)]
pub struct NoopCompositionDriver {
    name: String,
    emit_on_route: Option<String>,
    emit_payload: Value,
    observations: std::sync::Mutex<Vec<ObservedEffect>>,
}

impl NoopCompositionDriver {
    /// Construct a pure recorder — observes effects, emits nothing.
    pub fn recorder(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            emit_on_route: None,
            emit_payload: Value::Null,
            observations: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Construct a recorder that also emits one dispatched input per
    /// observed effect on the given route.
    pub fn emitting(
        name: impl Into<String>,
        emit_on_route: impl Into<String>,
        emit_payload: Value,
    ) -> Self {
        Self {
            name: name.into(),
            emit_on_route: Some(emit_on_route.into()),
            emit_payload,
            observations: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of every effect observed so far.
    pub fn observations(&self) -> Vec<ObservedEffect> {
        self.observations
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

impl CompositionDriverTrait for NoopCompositionDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_effect(&self, effect: &ObservedEffect) -> Result<Vec<DispatchedInput>, DriverError> {
        if let Ok(mut guard) = self.observations.lock() {
            guard.push(effect.clone());
        }
        match &self.emit_on_route {
            Some(route) => Ok(vec![DispatchedInput::new(
                route.clone(),
                self.emit_payload.clone(),
            )]),
            None => Ok(vec![]),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_machine_schema::{
        CompositionDriverRustBinding, RouteTargetKind, meerkat_mob_seam_composition,
    };
    use serde_json::json;

    fn noop_descriptor() -> CompositionDriver {
        CompositionDriver {
            name: "noop_driver".into(),
            rust: CompositionDriverRustBinding {
                module_path: "meerkat-runtime/src/generated/noop_driver.rs".into(),
                driver_type: "NoopDriver".into(),
                store_plan_type: "NoopStorePlan".into(),
                work_type: "NoopWork".into(),
                decision_type: "NoopDecision".into(),
                required_imports: vec![],
            },
            watched_effects: vec![WatchedEffect {
                producer_instance: "mob".into(),
                effect_variant: "RequestRuntimeBinding".into(),
            }],
            dispatch_routes: vec![DriverDispatchRoute {
                name: "noop_dispatch".into(),
                target_instance: "meerkat".into(),
                target_kind: RouteTargetKind::Input,
                input_variant: "PrepareBindings".into(),
            }],
        }
    }

    fn noop_composition() -> CompositionSchema {
        let mut composition = meerkat_mob_seam_composition();
        composition.driver = Some(noop_descriptor());
        composition
    }

    #[test]
    fn dispatcher_registers_and_lists_driver_names() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        let driver: Arc<dyn CompositionDriverTrait> =
            Arc::new(NoopCompositionDriver::recorder("noop_driver"));

        dispatcher
            .register_driver(&composition, driver.clone())
            .expect("register");

        let names = dispatcher.registered_driver_names();
        assert_eq!(names, vec!["noop_driver"]);
    }

    #[test]
    fn dispatcher_rejects_composition_without_driver() {
        let mut composition = meerkat_mob_seam_composition();
        composition.driver = None;
        let mut dispatcher = CompositionDispatcher::new();
        let driver: Arc<dyn CompositionDriverTrait> =
            Arc::new(NoopCompositionDriver::recorder("noop_driver"));

        let err = dispatcher
            .register_driver(&composition, driver)
            .expect_err("should reject driverless composition");
        assert!(matches!(
            err,
            DispatcherError::CompositionHasNoDriver { .. }
        ));
    }

    #[test]
    fn dispatcher_rejects_name_mismatch_between_descriptor_and_implementation() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        let driver: Arc<dyn CompositionDriverTrait> =
            Arc::new(NoopCompositionDriver::recorder("other_name"));

        let err = dispatcher
            .register_driver(&composition, driver)
            .expect_err("name mismatch must be rejected");
        assert!(matches!(err, DispatcherError::DriverNameMismatch { .. }));
    }

    #[test]
    fn dispatcher_rejects_duplicate_driver_registration() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        let driver_1: Arc<dyn CompositionDriverTrait> =
            Arc::new(NoopCompositionDriver::recorder("noop_driver"));
        let driver_2: Arc<dyn CompositionDriverTrait> =
            Arc::new(NoopCompositionDriver::recorder("noop_driver"));

        dispatcher
            .register_driver(&composition, driver_1)
            .expect("first registration");
        let err = dispatcher
            .register_driver(&composition, driver_2)
            .expect_err("duplicate must be rejected");
        assert!(matches!(err, DispatcherError::DuplicateDriver { .. }));
    }

    #[test]
    fn dispatcher_routes_observed_effect_to_watching_driver() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        let driver = Arc::new(NoopCompositionDriver::recorder("noop_driver"));
        dispatcher
            .register_driver(&composition, driver.clone())
            .expect("register");

        let effect = ObservedEffect::new(
            "mob",
            "RequestRuntimeBinding",
            json!({ "agent_runtime_id": "arn-1" }),
        );
        let dispatches = dispatcher
            .dispatch_effect(&effect)
            .expect("dispatch succeeds");
        assert!(
            dispatches.is_empty(),
            "recorder driver must not emit dispatches"
        );

        let observations = driver.observations();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0], effect);
    }

    #[test]
    fn dispatcher_skips_drivers_not_watching_the_effect() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        let driver = Arc::new(NoopCompositionDriver::recorder("noop_driver"));
        dispatcher
            .register_driver(&composition, driver.clone())
            .expect("register");

        let effect = ObservedEffect::new("mob", "SomeOtherEffect", json!({ "field": "value" }));
        let dispatches = dispatcher
            .dispatch_effect(&effect)
            .expect("dispatch succeeds");
        assert!(dispatches.is_empty());
        assert!(
            driver.observations().is_empty(),
            "driver must not observe effects outside its watched set"
        );
    }

    #[test]
    fn dispatcher_routes_emitting_driver_output_through_declared_dispatch_route() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        let driver = Arc::new(NoopCompositionDriver::emitting(
            "noop_driver",
            "noop_dispatch",
            json!({ "agent_runtime_id": "arn-1" }),
        ));
        dispatcher
            .register_driver(&composition, driver)
            .expect("register");

        let effect = ObservedEffect::new(
            "mob",
            "RequestRuntimeBinding",
            json!({ "agent_runtime_id": "arn-1" }),
        );
        let dispatches = dispatcher
            .dispatch_effect(&effect)
            .expect("dispatch succeeds");

        assert_eq!(dispatches.len(), 1);
        let dispatch = &dispatches[0];
        assert_eq!(dispatch.driver_name, "noop_driver");
        assert_eq!(dispatch.target_instance, "meerkat");
        assert_eq!(dispatch.target_kind, RouteTargetKind::Input);
        assert_eq!(dispatch.target_variant, "PrepareBindings");
        assert_eq!(dispatch.payload, json!({ "agent_runtime_id": "arn-1" }));
    }

    #[derive(Debug)]
    struct RogueDriver;

    impl CompositionDriverTrait for RogueDriver {
        fn name(&self) -> &'static str {
            "noop_driver"
        }

        fn on_effect(&self, _effect: &ObservedEffect) -> Result<Vec<DispatchedInput>, DriverError> {
            Ok(vec![DispatchedInput::new("undeclared_dispatch", json!({}))])
        }
    }

    #[test]
    fn dispatcher_surfaces_unknown_dispatch_route_as_driver_error() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        dispatcher
            .register_driver(&composition, Arc::new(RogueDriver))
            .expect("register");

        let effect = ObservedEffect::new("mob", "RequestRuntimeBinding", json!({}));
        let err = dispatcher
            .dispatch_effect(&effect)
            .expect_err("rogue dispatch must fail");
        assert!(matches!(
            err,
            DispatcherError::Driver(DriverError::UnknownDispatchRoute { .. })
        ));
    }

    #[derive(Debug)]
    struct FailingDriver;

    impl CompositionDriverTrait for FailingDriver {
        fn name(&self) -> &'static str {
            "noop_driver"
        }

        fn on_effect(&self, _effect: &ObservedEffect) -> Result<Vec<DispatchedInput>, DriverError> {
            Err(DriverError::DecisionFailed {
                driver: "noop_driver".into(),
                reason: "synthetic failure".into(),
            })
        }
    }

    #[test]
    fn dispatcher_propagates_decision_failure_as_typed_error() {
        let composition = noop_composition();
        let mut dispatcher = CompositionDispatcher::new();
        dispatcher
            .register_driver(&composition, Arc::new(FailingDriver))
            .expect("register");

        let effect = ObservedEffect::new("mob", "RequestRuntimeBinding", json!({}));
        let err = dispatcher
            .dispatch_effect(&effect)
            .expect_err("driver failure must surface");
        let DispatcherError::Driver(DriverError::DecisionFailed { driver, reason }) = err else {
            panic!("expected DecisionFailed, got {err:?}");
        };
        assert_eq!(driver, "noop_driver");
        assert_eq!(reason, "synthetic failure");
    }
}
