//! #1450: an ephemeral session service owns the runtime machine it hands a
//! mob. The machine lives and dies with the service instance; there is no
//! process-global registry keyed by the service's address, so a service can
//! never be handed another service's machine.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use meerkat::{AgentFactory, Config};
use meerkat_mob::MobSessionService;

fn service() -> Arc<meerkat::EphemeralSessionService<meerkat::FactoryAgentBuilder>> {
    Arc::new(meerkat::build_ephemeral_service(
        AgentFactory::minimal(),
        Config::default(),
        4,
    ))
}

fn machine_of(
    service: &meerkat::EphemeralSessionService<meerkat::FactoryAgentBuilder>,
) -> Arc<meerkat_runtime::MeerkatMachine> {
    MobSessionService::runtime_adapter(service).expect("an ephemeral service serves a machine")
}

#[test]
fn one_service_always_serves_the_same_machine() {
    let service = service();
    assert!(Arc::ptr_eq(&machine_of(&service), &machine_of(&service)));
}

#[test]
fn distinct_services_own_distinct_machines() {
    let first = service();
    let second = service();
    assert!(!Arc::ptr_eq(&machine_of(&first), &machine_of(&second)));
}

#[test]
fn a_dropped_service_takes_its_machine_with_it() {
    let survivor = service();
    let survivor_machine = machine_of(&survivor);
    let dropped = service();
    let machine = Arc::downgrade(&machine_of(&dropped));
    assert!(
        machine.upgrade().is_some(),
        "the service keeps its machine alive"
    );
    drop(dropped);
    assert!(
        machine.upgrade().is_none(),
        "the machine is owned by its service, not by a process-global registry"
    );
    // A service created after the drop gets a machine of its own, never the
    // dropped one, whatever address it lands at.
    let successor = service();
    assert!(!Arc::ptr_eq(&machine_of(&successor), &survivor_machine));
    assert!(Arc::ptr_eq(&machine_of(&survivor), &survivor_machine));
}
