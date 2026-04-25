//! Typed route index consumed by [`super::CatalogCompositionDispatcher`].
//!
//! The table is built once from a
//! [`meerkat_machine_schema::CompositionSchema`] and then looked up by
//! `(producer_instance, effect_variant)`. It mirrors the
//! `route_to_input` function the codegen emits (B-4 + B-4b) but consumes
//! the schema directly so the runtime doesn't need to depend on a
//! generated file tree at wire-up time. Input-kind and signal-kind routes
//! are indexed separately and consumed by their matching typed dispatcher
//! surfaces.

use std::collections::HashMap;
use std::fmt;

use meerkat_machine_schema::identity::{
    EffectVariantId, FieldId, InputVariantId, MachineInstanceId, RouteId, SignalVariantId,
};
use meerkat_machine_schema::{
    CompositionSchema, Route, RouteBindingSource, RouteTargetKind, RouteVariantId,
};
use thiserror::Error;

/// Typed route descriptor returned by [`RouteTable::resolve`].
///
/// Always input-kind: `RouteTable::resolve` consults only the input
/// index, so signal-kind routes never surface here. `bindings` pairs
/// producer-side [`FieldId`]s with the consumer-side [`FieldId`]s they
/// must populate, in declaration order. Literal / owner-provided
/// bindings are filtered out at table build time — the dispatcher only
/// handles producer-field bindings (the other kinds are supplied by
/// the consumer surface or literal machinery upstream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedInputDescriptor {
    pub route_id: RouteId,
    pub instance_id: MachineInstanceId,
    pub input_variant: InputVariantId,
    pub bindings: Vec<(FieldId, FieldId)>,
}

/// Typed signal-route descriptor returned by [`RouteTable::resolve_signal`].
///
/// Always signal-kind: input routes are isolated on the input index and
/// never surface through signal resolution. `bindings` has the same
/// producer-field-to-consumer-field meaning as [`RoutedInputDescriptor`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedSignalDescriptor {
    pub route_id: RouteId,
    pub instance_id: MachineInstanceId,
    pub signal_variant: SignalVariantId,
    pub bindings: Vec<(FieldId, FieldId)>,
}

/// Errors surfaced when building a [`RouteTable`] from a schema.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouteTableError {
    /// Two routes declared for the same `(producer_instance, variant)` pair.
    /// Wave-b V2 requires unique routing per producer variant; multi-target
    /// fan-out is explicitly out of scope.
    #[error(
        "composition declares duplicate input route for producer {instance} variant {variant}: \
         existing={existing_route}, duplicate={duplicate_route}"
    )]
    DuplicateInputRoute {
        instance: MachineInstanceId,
        variant: EffectVariantId,
        existing_route: RouteId,
        duplicate_route: RouteId,
    },
    /// Two signal routes declared for the same `(producer_instance,
    /// variant)` pair. Signal routes are a typed dispatch surface too,
    /// so duplicate declarations are rejected instead of last-writer
    /// silently winning.
    #[error(
        "composition declares duplicate signal route for producer {instance} variant {variant}: \
         existing={existing_route}, duplicate={duplicate_route}"
    )]
    DuplicateSignalRoute {
        instance: MachineInstanceId,
        variant: EffectVariantId,
        existing_route: RouteId,
        duplicate_route: RouteId,
    },
    /// An Input-kind route carried a Signal-typed variant id. Schema
    /// validation normally rejects this at declaration time; the table
    /// builder surfaces it as a typed error rather than panicking so
    /// callers handling hand-assembled schemas see a deterministic
    /// failure instead of a crash.
    #[error("input-kind route {route} in composition has a signal-typed variant id `{variant}`")]
    InputRouteCarriesSignalVariant { route: RouteId, variant: String },
    /// A Signal-kind route carried an Input-typed variant id. Schema
    /// validation normally rejects this at declaration time; the table
    /// builder returns a typed error to keep malformed hand-assembled
    /// schemas deterministic.
    #[error("signal-kind route {route} in composition has an input-typed variant id `{variant}`")]
    SignalRouteCarriesInputVariant { route: RouteId, variant: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct InputKey {
    instance: MachineInstanceId,
    variant: EffectVariantId,
}

/// Typed route index.
///
/// Two indices: one for `Input`-kind routes (consulted by the dispatcher)
/// and one for `Signal`-kind routes (reserved for the signal surface; the
/// dispatcher refuses these with a typed error).
#[derive(Clone)]
pub struct RouteTable {
    inputs: HashMap<InputKey, RoutedInputDescriptor>,
    signals: HashMap<InputKey, RoutedSignalDescriptor>,
}

impl fmt::Debug for RouteTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteTable")
            .field("input_routes", &self.inputs.len())
            .field("signal_routes", &self.signals.len())
            .finish()
    }
}

impl RouteTable {
    /// Build a typed route table from a composition schema.
    ///
    /// Returns [`RouteTableError::DuplicateInputRoute`] if the schema
    /// declares two input-kind routes for the same
    /// `(producer_instance, effect_variant)` pair. Signal-kind routes are
    /// indexed but never trigger duplicate-input collisions (they live on
    /// a separate map).
    pub fn from_schema(schema: &CompositionSchema) -> Result<Self, RouteTableError> {
        let mut inputs: HashMap<InputKey, RoutedInputDescriptor> = HashMap::new();
        let mut signals: HashMap<InputKey, RoutedSignalDescriptor> = HashMap::new();

        for route in &schema.routes {
            let key = InputKey {
                instance: route.from_machine.clone(),
                variant: route.effect_variant.clone(),
            };

            match route.to.kind {
                RouteTargetKind::Input => {
                    let descriptor = Self::build_input_descriptor(route)?;
                    if let Some(existing) = inputs.get(&key) {
                        return Err(RouteTableError::DuplicateInputRoute {
                            instance: key.instance.clone(),
                            variant: key.variant.clone(),
                            existing_route: existing.route_id.clone(),
                            duplicate_route: route.name.clone(),
                        });
                    }
                    inputs.insert(key, descriptor);
                }
                RouteTargetKind::Signal => {
                    let descriptor = Self::build_signal_descriptor(route)?;
                    if let Some(existing) = signals.get(&key) {
                        return Err(RouteTableError::DuplicateSignalRoute {
                            instance: key.instance.clone(),
                            variant: key.variant.clone(),
                            existing_route: existing.route_id.clone(),
                            duplicate_route: route.name.clone(),
                        });
                    }
                    signals.insert(key, descriptor);
                }
            }
        }

        Ok(Self { inputs, signals })
    }

    fn build_input_descriptor(route: &Route) -> Result<RoutedInputDescriptor, RouteTableError> {
        let input_variant = match &route.to.input_variant {
            RouteVariantId::Input(id) => id.clone(),
            RouteVariantId::Signal(id) => {
                return Err(RouteTableError::InputRouteCarriesSignalVariant {
                    route: route.name.clone(),
                    variant: id.as_str().to_owned(),
                });
            }
        };

        Ok(RoutedInputDescriptor {
            route_id: route.name.clone(),
            instance_id: route.to.machine.clone(),
            input_variant,
            bindings: Self::field_bindings(route),
        })
    }

    fn build_signal_descriptor(route: &Route) -> Result<RoutedSignalDescriptor, RouteTableError> {
        let signal_variant = match &route.to.input_variant {
            RouteVariantId::Signal(id) => id.clone(),
            RouteVariantId::Input(id) => {
                return Err(RouteTableError::SignalRouteCarriesInputVariant {
                    route: route.name.clone(),
                    variant: id.as_str().to_owned(),
                });
            }
        };

        Ok(RoutedSignalDescriptor {
            route_id: route.name.clone(),
            instance_id: route.to.machine.clone(),
            signal_variant,
            bindings: Self::field_bindings(route),
        })
    }

    fn field_bindings(route: &Route) -> Vec<(FieldId, FieldId)> {
        route
            .bindings
            .iter()
            .filter_map(|binding| match &binding.source {
                RouteBindingSource::Field { from_field, .. } => {
                    Some((from_field.clone(), binding.to_field.clone()))
                }
                RouteBindingSource::Literal(_) | RouteBindingSource::OwnerProvided => None,
            })
            .collect()
    }

    /// Resolve a producer `(instance_id, effect_variant)` to the typed
    /// input descriptor. Returns `None` if no input-kind route exists for
    /// the pair — the caller then returns
    /// [`super::DispatchRefusal::UnresolvedRoute`].
    pub fn resolve(
        &self,
        instance_id: &MachineInstanceId,
        effect_variant: &EffectVariantId,
    ) -> Option<&RoutedInputDescriptor> {
        self.inputs.get(&InputKey {
            instance: instance_id.clone(),
            variant: effect_variant.clone(),
        })
    }

    /// Resolve a producer `(instance_id, effect_variant)` to the typed
    /// signal descriptor. Returns `None` if no signal-kind route exists
    /// for the pair.
    pub fn resolve_signal(
        &self,
        instance_id: &MachineInstanceId,
        effect_variant: &EffectVariantId,
    ) -> Option<&RoutedSignalDescriptor> {
        self.signals.get(&InputKey {
            instance: instance_id.clone(),
            variant: effect_variant.clone(),
        })
    }

    /// Count of input-kind routes. Primarily for diagnostics.
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// `true` when no input-kind routes are declared.
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// Number of signal-kind routes this table is aware of.
    pub fn signal_route_count(&self) -> usize {
        self.signals.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_machine_schema::catalog::meerkat_mob_seam_composition;

    #[test]
    fn builds_from_seam_schema_with_expected_routes() {
        let schema = meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).unwrap();

        // The live schema declares 4 Input-kind and 3 Signal-kind routes.
        assert_eq!(table.len(), 4);
        assert_eq!(table.signal_route_count(), 3);
    }

    #[test]
    fn resolves_request_runtime_binding_to_prepare_bindings() {
        let schema = meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).unwrap();

        let mob = MachineInstanceId::parse("mob").unwrap();
        let variant = EffectVariantId::parse("RequestRuntimeBinding").unwrap();
        let descriptor = table.resolve(&mob, &variant).expect("known route");

        assert_eq!(
            descriptor.route_id.as_str(),
            "binding_request_reaches_meerkat"
        );
        assert_eq!(descriptor.instance_id.as_str(), "meerkat");
        assert_eq!(descriptor.input_variant.as_str(), "PrepareBindings");
        let field_pairs: Vec<(&str, &str)> = descriptor
            .bindings
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect();
        assert_eq!(
            field_pairs,
            vec![
                ("agent_runtime_id", "agent_runtime_id"),
                ("fence_token", "fence_token"),
                ("generation", "generation"),
                ("session_id", "session_id"),
            ]
        );
    }

    #[test]
    fn signal_routes_are_isolated_from_input_lookup() {
        let schema = meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).unwrap();

        let meerkat = MachineInstanceId::parse("meerkat").unwrap();
        let runtime_bound = EffectVariantId::parse("RuntimeBound").unwrap();

        // `RuntimeBound` is a Signal-kind route; it must NOT surface in
        // the input-lookup path.
        assert!(table.resolve(&meerkat, &runtime_bound).is_none());
    }

    #[test]
    fn work_request_projects_agent_runtime_id_to_runtime_id() {
        let schema = meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).unwrap();

        let mob = MachineInstanceId::parse("mob").unwrap();
        let variant = EffectVariantId::parse("RequestRuntimeIngress").unwrap();
        let descriptor = table.resolve(&mob, &variant).expect("known route");

        assert_eq!(descriptor.route_id.as_str(), "work_request_reaches_meerkat");
        assert_eq!(descriptor.input_variant.as_str(), "Ingest");
        let field_pairs: Vec<(&str, &str)> = descriptor
            .bindings
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect();
        assert_eq!(
            field_pairs,
            vec![
                ("agent_runtime_id", "runtime_id"),
                ("work_id", "work_id"),
                ("origin", "origin"),
            ]
        );
    }

    #[test]
    fn resolves_runtime_bound_signal_binding_to_mob_signal() {
        let schema = meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).unwrap();

        let meerkat = MachineInstanceId::parse("meerkat").unwrap();
        let runtime_bound = EffectVariantId::parse("RuntimeBound").unwrap();
        let descriptor = table
            .resolve_signal(&meerkat, &runtime_bound)
            .expect("known signal route");

        assert_eq!(descriptor.route_id.as_str(), "runtime_bound_reaches_mob");
        assert_eq!(descriptor.instance_id.as_str(), "mob");
        assert_eq!(descriptor.signal_variant.as_str(), "ObserveRuntimeReady");
        let field_pairs: Vec<(&str, &str)> = descriptor
            .bindings
            .iter()
            .map(|(from, to)| (from.as_str(), to.as_str()))
            .collect();
        assert_eq!(
            field_pairs,
            vec![
                ("agent_runtime_id", "agent_runtime_id"),
                ("fence_token", "fence_token"),
            ]
        );
    }

    #[test]
    fn resolve_returns_none_for_unknown_variant() {
        let schema = meerkat_mob_seam_composition();
        let table = RouteTable::from_schema(&schema).unwrap();

        let mob = MachineInstanceId::parse("mob").unwrap();
        let variant = EffectVariantId::parse("NoSuchVariant").unwrap();
        assert!(table.resolve(&mob, &variant).is_none());
    }
}
