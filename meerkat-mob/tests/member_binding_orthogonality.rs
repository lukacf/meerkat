//! Plan §5.6: mob member `connection_ref` orthogonality.
//!
//! Two mob members bound to different realms must resolve their
//! credentials independently. Cross-contamination (member B seeing
//! member A's lease, or vice-versa) would violate the design §3c
//! guarantee that `AgentIdentity` and `connection_ref` are orthogonal
//! axes of identity.
//!
//! The canonical choke point for this guarantee is
//! `ProviderRuntimeRegistry::resolve(&realm, &binding_id, &env)` —
//! the only path through which `AgentBuildConfig.connection_ref`
//! resolves to a live credential lease (see
//! `meerkat/src/factory.rs::build_agent`). The registry is stateless
//! across resolve calls; each call reads only the `(realm, binding_id,
//! env)` inputs it is given. This test exercises the two-member case
//! concurrently to prove the property at the choke point, rather than
//! re-implementing a full mob construction pipeline (which would add
//! scaffolding without adding signal for this specific invariant).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_client::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_core::auth::ResolvedAuthKind;
use meerkat_core::{RealmConfigSection, RealmConnectionSet};

fn realm_with_anthropic_key(realm_id: &str, key: &str) -> RealmConnectionSet {
    let section = RealmConfigSection::from_inline_api_keys(&[("anthropic", key)]);
    RealmConnectionSet::from_config(realm_id, &section)
        .expect("inline anthropic realm is always well-formed")
}

fn inline_secret(kind: &ResolvedAuthKind) -> Arc<String> {
    match kind {
        ResolvedAuthKind::InlineSecret(s) => Arc::clone(s),
        other => panic!("expected InlineSecret lease kind, got {other:?}"),
    }
}

/// Two concurrent resolves against two distinct realms produce two
/// distinct leases. Each lease surfaces the secret that belongs to
/// its own realm — no cross-contamination.
#[tokio::test]
async fn two_member_bindings_resolve_to_distinct_leases() {
    let realm_a = realm_with_anthropic_key("mob_member_a", "sk-a-0000000000000000000000000000");
    let realm_b = realm_with_anthropic_key("mob_member_b", "sk-b-1111111111111111111111111111");

    let registry = ProviderRuntimeRegistry::default();
    let env = ResolverEnvironment::with_process_env();

    let (a, b) = tokio::join!(
        registry.resolve(&realm_a, "default_anthropic", &env),
        registry.resolve(&realm_b, "default_anthropic", &env),
    );
    let a = a.expect("realm A resolves");
    let b = b.expect("realm B resolves");

    let secret_a = inline_secret(a.auth_lease.kind());
    let secret_b = inline_secret(b.auth_lease.kind());
    assert_eq!(
        secret_a.as_str(),
        "sk-a-0000000000000000000000000000",
        "member A lease must carry realm A's secret"
    );
    assert_eq!(
        secret_b.as_str(),
        "sk-b-1111111111111111111111111111",
        "member B lease must carry realm B's secret"
    );
    assert_ne!(
        secret_a, secret_b,
        "orthogonality: two distinct realms must not share a lease"
    );
}

/// Resolving the same realm twice is independent: each resolve
/// returns a fresh lease (no caching that would let member A's
/// resolution shortcut member B's). Covers the case where two members
/// happen to share a realm but each goes through its own resolve.
#[tokio::test]
async fn same_realm_two_resolves_are_independent_invocations() {
    let realm = realm_with_anthropic_key("mob_shared_realm", "sk-shared-abcdef0123456789abcdef01");

    let registry = ProviderRuntimeRegistry::default();
    let env = ResolverEnvironment::with_process_env();

    let first = registry
        .resolve(&realm, "default_anthropic", &env)
        .await
        .expect("first resolve");
    let second = registry
        .resolve(&realm, "default_anthropic", &env)
        .await
        .expect("second resolve");

    let s1 = inline_secret(first.auth_lease.kind());
    let s2 = inline_secret(second.auth_lease.kind());
    assert_eq!(s1.as_str(), "sk-shared-abcdef0123456789abcdef01");
    assert_eq!(s2.as_str(), "sk-shared-abcdef0123456789abcdef01");
    // Distinct Arc instances — each resolve produces its own lease
    // object, so there is no registry-level lease reuse that could
    // leak state between members.
    assert!(
        !Arc::ptr_eq(&s1, &s2),
        "each resolve must produce an independent lease Arc"
    );
}

/// AgentIdentity (here: provider/binding choice) is orthogonal to
/// connection_ref — mixing providers across realms does not cause
/// member A's anthropic lease to collide with member B's openai lease.
#[tokio::test]
async fn cross_provider_members_resolve_orthogonally() {
    let section_a = RealmConfigSection::from_inline_api_keys(&[(
        "anthropic",
        "sk-ant-aaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )]);
    let realm_a = RealmConnectionSet::from_config("mob_ant", &section_a).unwrap();

    let section_b = RealmConfigSection::from_inline_api_keys(&[(
        "openai",
        "sk-oai-bbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )]);
    let realm_b = RealmConnectionSet::from_config("mob_oai", &section_b).unwrap();

    let registry = ProviderRuntimeRegistry::default();
    let env = ResolverEnvironment::with_process_env();

    let (a, b) = tokio::join!(
        registry.resolve(&realm_a, "default_anthropic", &env),
        registry.resolve(&realm_b, "default_openai", &env),
    );
    let a = a.expect("anthropic realm resolves");
    let b = b.expect("openai realm resolves");

    assert_ne!(
        a.provider, b.provider,
        "members must route to their own provider"
    );
    let sa = inline_secret(a.auth_lease.kind());
    let sb = inline_secret(b.auth_lease.kind());
    assert!(sa.starts_with("sk-ant-"), "A lease carries anthropic key");
    assert!(sb.starts_with("sk-oai-"), "B lease carries openai key");
}
