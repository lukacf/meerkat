//! Trusted selection/commissioning guards, not native grants or collection runs.
use super::*;
use crate::service_probe::{
    ServiceProbeSelection, ServiceSelection,
    tests::{binding, selected},
};
use serde_json::json;
struct Never;
impl CollectionProgram for Never {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("no native result is supplied by these guards")
    }
}
fn source(id: &str, profile: &str, agent: &str, ops: &[&str]) -> RequiredServiceCollection {
    RequiredServiceCollection::new(
        id.into(),
        "github".into(),
        profile.into(),
        agent.into(),
        vec![
            RequiredCollectionOperation::new(
                "required".into(),
                json!({"repository":id}),
                ops.iter().map(|op| (*op).into()).collect(),
                Arc::new(Never),
            )
            .unwrap(),
        ],
        CollectionLimits::new(4, 4096, 4096).unwrap(),
    )
    .unwrap()
}
fn config(ops: &[&str]) -> McpServerConfig {
    serde_json::from_value(json!({"name":"selected","command":"/bin/false","args":[],
        "access":{"allowed_operations":ops}}))
    .unwrap()
}
fn ops(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|v| (*v).into()).collect()
}
#[test]
fn collection_host_preserves_distinct_sources_without_a_dummy_fixed_probe() {
    let root = tempfile::tempdir().unwrap();
    let selected = ServiceCollectionSelection::new(
        binding(root.path()),
        vec![
            source("one", "enterprise", "agent", &["read"]),
            source("two", "enterprise", "agent", &["read"]),
        ],
    )
    .unwrap();
    assert_eq!(selected.source_ids().collect::<Vec<_>>(), ["one", "two"]);
    assert_eq!(
        selected.collections[0].requirements[0].resource,
        json!({"repository":"one"})
    );
    assert_eq!(
        selected.collections[1].requirements[0].resource,
        json!({"repository":"two"})
    );
    assert!(
        selected
            .host
            .lifetime
            .current(selected.host.binding.expires_at_ms)
    );
    let composition = ServiceSelection::Collections(Arc::new(selected));
    let same = composition.clone();
    assert!(Arc::ptr_eq(composition.host(), same.host()));
    assert_eq!(composition.version(), 7);
    assert!(ServiceProbeSelection::new(binding(root.path()), vec![]).is_err());
}
#[test]
fn collections_require_one_installed_tuple_unique_sources_and_finite_union() {
    let root = tempfile::tempdir().unwrap();
    let make = || source("one", "enterprise", "agent", &["read"]);
    assert!(ServiceCollectionSelection::new(binding(root.path()), vec![]).is_err());
    assert!(ServiceCollectionSelection::new(binding(root.path()), vec![make(), make()]).is_err());
    for (profile, agent) in [("other", "agent"), ("enterprise", "other")] {
        assert!(
            ServiceCollectionSelection::new(
                binding(root.path()),
                vec![make(), source("two", profile, agent, &["read"])]
            )
            .is_err()
        );
    }
    let many = (0..17)
        .map(|id| source(&format!("s{id}"), "enterprise", "agent", &["read"]))
        .collect();
    assert!(ServiceCollectionSelection::new(binding(root.path()), many).is_err());
    let many_ops = (0..9)
        .map(|i| {
            let values = (0..32).map(|j| format!("read_{i}_{j}")).collect::<Vec<_>>();
            source(
                &format!("source{i}"),
                "enterprise",
                "agent",
                &values.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert!(ServiceCollectionSelection::new(binding(root.path()), many_ops).is_err());
}
#[test]
fn v7_commissioning_requires_exact_reviewed_union_and_native_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let selection = ServiceSelection::Collections(Arc::new(
        ServiceCollectionSelection::new(
            binding(root.path()),
            vec![
                source("one", "enterprise", "agent", &["list"]),
                source("two", "enterprise", "agent", &["read", "list"]),
            ],
        )
        .unwrap(),
    ));
    let connection = config(&["read", "list"]);
    assert!(selection.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops(&["list", "read"]),
        &connection
    ));
    for reviewed in [
        ops(&["read"]),
        ops(&["list", "read", "write"]),
        BTreeSet::new(),
    ] {
        assert!(!selection.permits_review("github", "enterprise", "agent", &reviewed, &connection));
    }
    for (family, profile, agent) in [
        ("foreign", "enterprise", "agent"),
        ("github", "other", "agent"),
        ("github", "enterprise", "other"),
    ] {
        assert!(!selection.permits_review(
            family,
            profile,
            agent,
            &ops(&["read", "list"]),
            &connection
        ));
    }
    assert!(!selection.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops(&["read", "list"]),
        &config(&["read"])
    ));
    // V6 deliberately retains its prior matched-profile semantics, including a
    // reviewed superset. V7's stricter equality must not silently change v6.
    let old = ServiceSelection::Probes(Arc::new(
        ServiceProbeSelection::new(binding(root.path()), vec![selected("one")]).unwrap(),
    ));
    assert!(old.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops(&["repository_read", "another"]),
        &config(&["repository_read"])
    ));
}
#[tokio::test]
async fn collection_control_retires_the_same_host_publication_owner() {
    let root = tempfile::tempdir().unwrap();
    let selection = ServiceCollectionSelection::new(
        binding(root.path()),
        vec![source("one", "enterprise", "agent", &["read"])],
    )
    .unwrap();
    let host = selection.host.clone();
    let control = selection.control();
    host.lifetime
        .join(meerkat_authorization::publication::LocalAuthorizationPublication::new())
        .unwrap();
    let waiting = host.lifetime.closed(host.binding.expires_at_ms);
    tokio::pin!(waiting);
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());
    control.retire();
    tokio::time::timeout(std::time::Duration::from_secs(2), waiting)
        .await
        .unwrap();
    assert!(!host.lifetime.current(host.binding.expires_at_ms));
    assert!(
        host.lifetime
            .join(meerkat_authorization::publication::LocalAuthorizationPublication::new())
            .is_err()
    );
}

#[test]
fn multiple_services_retain_one_host_and_never_union_their_commissioning_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let selection = ServiceCollectionSelection::for_services(
        binding(root.path()),
        vec![
            (
                "official".into(),
                vec![source("identity", "enterprise", "agent", &["get_me"])],
            ),
            (
                "python".into(),
                vec![source(
                    "repository",
                    "python",
                    "reader",
                    &["read_repository"],
                )],
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        selection.service_ids().collect::<Vec<_>>(),
        ["official", "python"]
    );
    assert_eq!(
        selection.source_ids().collect::<Vec<_>>(),
        ["identity", "repository"]
    );
    let first = selection.service_selection(0).unwrap();
    let second = selection.service_selection(1).unwrap();
    assert!(Arc::ptr_eq(&selection.host, &first.host) && Arc::ptr_eq(&first.host, &second.host));
    assert!(first.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops(&["get_me"]),
        &config(&["get_me"])
    ));
    assert!(second.permits_review(
        "github",
        "python",
        "reader",
        &ops(&["read_repository"]),
        &config(&["read_repository"])
    ));
    assert!(!first.permits_review(
        "github",
        "python",
        "reader",
        &ops(&["read_repository"]),
        &config(&["read_repository"])
    ));
    assert!(!first.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops(&["get_me", "read_repository"]),
        &config(&["get_me", "read_repository"])
    ));
    assert!(!selection.permits_review(
        "github",
        "enterprise",
        "agent",
        &ops(&["get_me", "read_repository"]),
        &config(&["get_me", "read_repository"])
    ));
    assert!(selection.service_selection(2).is_none());
    selection.control().retire();
    assert!(
        !first
            .host
            .lifetime
            .current(first.host.binding.expires_at_ms)
    );
    assert!(
        !second
            .host
            .lifetime
            .current(second.host.binding.expires_at_ms)
    );
}
#[test]
fn multiple_services_keep_global_source_uniqueness_and_finite_total_budget() {
    let root = tempfile::tempdir().unwrap();
    let make = |id: &str| source(id, "enterprise", "agent", &["read"]);
    assert!(ServiceCollectionSelection::for_services(binding(root.path()), vec![]).is_err());
    assert!(
        ServiceCollectionSelection::for_services(
            binding(root.path()),
            vec![("one".into(), vec![])]
        )
        .is_err()
    );
    assert!(
        ServiceCollectionSelection::for_services(
            binding(root.path()),
            vec![
                ("one".into(), vec![make("a")]),
                ("one".into(), vec![make("b")])
            ]
        )
        .is_err()
    );
    assert!(
        ServiceCollectionSelection::for_services(
            binding(root.path()),
            vec![
                ("one".into(), vec![make("a")]),
                ("two".into(), vec![make("a")])
            ]
        )
        .is_err()
    );
    assert!(
        ServiceCollectionSelection::for_services(
            binding(root.path()),
            vec![(
                "one".into(),
                vec![make("a"), source("b", "foreign", "agent", &["read"])]
            )]
        )
        .is_err()
    );
    let too_many = (0..17)
        .map(|id| (format!("service-{id}"), vec![make(&format!("source-{id}"))]))
        .collect();
    assert!(ServiceCollectionSelection::for_services(binding(root.path()), too_many).is_err());
    let many_sources = vec![
        (
            "one".into(),
            (0..9).map(|i| make(&format!("a{i}"))).collect(),
        ),
        (
            "two".into(),
            (0..8).map(|i| make(&format!("b{i}"))).collect(),
        ),
    ];
    assert!(ServiceCollectionSelection::for_services(binding(root.path()), many_sources).is_err());
}

#[test]
fn equal_binding_labels_do_not_make_another_invocation_observation_match() {
    let root = tempfile::tempdir().unwrap();
    let first = ServiceCollectionSelection::new(
        binding(root.path()),
        vec![source("one", "enterprise", "agent", &["read"])],
    )
    .unwrap();
    let other = ServiceCollectionSelection::new(
        binding(root.path()),
        vec![source("one", "enterprise", "agent", &["read"])],
    )
    .unwrap();
    let observation = CollectionInvocationObservation(first.host.collection_identity.clone());
    let same = CollectionInvocationObservation(
        first
            .service_selection(0)
            .unwrap()
            .host
            .collection_identity
            .clone(),
    );
    let foreign = CollectionInvocationObservation(other.host.collection_identity.clone());
    assert!(observation.same_invocation(&same));
    assert!(observation.same_invocation(&observation.clone()));
    assert!(!observation.same_invocation(&foreign));
    first.control().retire();
    assert!(observation.same_invocation(&same));
    // Historical pointer correlation deliberately does not claim currentness.
    assert!(
        !first
            .host
            .lifetime
            .current(first.host.binding.expires_at_ms)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn selected_input_follows_only_its_original_first_or_next_service_slot() {
    let root = tempfile::tempdir().unwrap();
    for (id, index) in [("official", 0), ("python", 1)] {
        let mut selection = ServiceCollectionSelection::for_services(
            binding(root.path()),
            vec![
                (
                    "official".into(),
                    vec![source("identity", "enterprise", "agent", &["get_me"])],
                ),
                (
                    "python".into(),
                    vec![source(
                        "repository",
                        "python",
                        "reader",
                        &["read_repository"],
                    )],
                ),
            ],
        )
        .unwrap();
        let original = crate::protected_parent::selected_input::inert_test_input(id);
        selection.selected_input = Some(original.clone());
        let selected = ServiceSelection::Collections(selection.service_selection(index).unwrap());
        let other = ServiceSelection::Collections(selection.service_selection(1 - index).unwrap());
        assert!(Arc::ptr_eq(&original, &selected.selected_input().unwrap()));
        assert!(other.selected_input().is_none());
        assert!(
            ServiceSelection::Collections(Arc::new(selection))
                .selected_input()
                .is_none()
        );
    }
}
