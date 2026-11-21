//! Idle WorkGraph pursuit composed into a persistent gateway launch, for one
//! gateway process test target only.
//!
//! This module exists only under the `workgraph-pursuit-test-vehicle`
//! feature, which no default, release, CI or packaging build names. With the
//! feature on, a gateway process that carries
//! `META_TEST_VEHICLE_WORKGRAPH_PURSUIT=all` in its environment makes one
//! [`meerkat_mob::WorkGraphPursuit`] over the WorkGraph service its launch
//! made, goes on with the pursuit's value of that service (the bootstrap spec
//! and the `meta/workgraph/*` methods then write through it, so their
//! committed writes are told), and attaches the launch's mob once its handle
//! exists. A process without that value composes nothing, feature or not.
//!
//! It is not a product surface: there is no configuration key, launch
//! option, command line flag, wire field or stored value, every item is
//! crate-private, and nothing here decides who may enable pursuit in a
//! product. The predicate states every binding as pursued, which only a test
//! host can want.
//!
//! What it does not cover: a member's own write through its WorkGraph tools
//! is not told to this pursuit, because the launch fills the member tool slot
//! before this module is asked; the embedding builder and the console gateway
//! compose nothing.

use std::sync::Arc;

use crate::mob_handle_runtime::{MobBootstrapSpec, MobRuntimeError};

/// The environment value a test host sets on the gateway process it spawns.
const ENABLE_ENV: &str = "META_TEST_VEHICLE_WORKGRAPH_PURSUIT";

/// The one accepted value: every binding is pursued.
const ENABLE_ALL: &str = "all";

/// The pursuit of one launch, or none.
#[derive(Clone, Default)]
pub(crate) struct Vehicle {
    pursuit: Option<meerkat_mob::WorkGraphPursuit>,
}

impl Vehicle {
    /// The service the launch goes on with: the pursuit's value of it when a
    /// pursuit was composed, the launch's own otherwise.
    pub(crate) fn service_or(
        &self,
        service: Option<meerkat::WorkGraphService>,
    ) -> Option<meerkat::WorkGraphService> {
        match self.pursuit.as_ref() {
            Some(pursuit) => Some(pursuit.workgraph_service().clone()),
            None => service,
        }
    }
}

/// Compose the pursuit over the launch's WorkGraph service when this process
/// was started with the enabling value and the launch has a WorkGraph.
pub(crate) fn compose(service: &Option<meerkat::WorkGraphService>) -> Vehicle {
    if !std::env::var(ENABLE_ENV).is_ok_and(|value| value == ENABLE_ALL) {
        return Vehicle::default();
    }
    let Some(service) = service else {
        eprintln!("workgraph pursuit test vehicle: this launch has no WorkGraph; nothing composed");
        return Vehicle::default();
    };
    let pursuit = meerkat_mob::WorkGraphPursuit::new(
        service.clone(),
        Arc::new(|_: &meerkat::WorkAttentionBinding| true),
    );
    eprintln!("workgraph pursuit test vehicle: composed; every binding is pursued");
    Vehicle {
        pursuit: Some(pursuit),
    }
}

/// Put the launch's pursuit on its bootstrap spec, where the bootstrap finds
/// it once the mob's handle exists.
pub(crate) fn carry(spec: &mut MobBootstrapSpec, vehicle: Vehicle) {
    spec.workgraph_pursuit = vehicle;
}

/// Attach the mob to the launch's pursuit. A launch that composed none
/// attaches nothing. A refused attach fails the bootstrap: a launch that was
/// asked for pursuit and runs without it would read as a member that was not
/// woken.
pub(crate) fn attach(
    vehicle: &Vehicle,
    mob: &meerkat_mob::MobHandle,
) -> Result<(), MobRuntimeError> {
    let Some(pursuit) = vehicle.pursuit.as_ref() else {
        return Ok(());
    };
    pursuit.attach(mob).map_err(MobRuntimeError::Mob)?;
    eprintln!(
        "workgraph pursuit test vehicle: attached to mob '{}'",
        mob.mob_id()
    );
    Ok(())
}
