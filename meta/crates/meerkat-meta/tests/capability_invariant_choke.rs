//! Structural mutation gates for the declared-versus-resolved capability
//! invariant's first-party composition coverage.
//!
//! Behavioral fresh/resume and category-mutation tests live beside the
//! private wrapper implementation. These assertions make the production
//! gateway roots fail if either persistent or ephemeral composition bypasses
//! `MobBootstrapSpec::new`, or if the wrapper stops evaluating after the
//! inner service has materialized the live catalog.

#![allow(clippy::expect_used, clippy::panic)]

const RUNTIME_SOURCE: &str = include_str!("../src/mob_handle_runtime.rs");
const BUILDER_SOURCE: &str = include_str!("../src/unified_runtime/builder.rs");
const META_GATEWAY_SOURCE: &str = include_str!("../src/bin/meta_gateway.rs");
/// The stdin-RPC gateway lives in the library; its binary only delegates.
const RPC_GATEWAY_SOURCE: &str = include_str!("../src/rpc_gateway.rs");

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

/// `source` without its test modules, so a count sees production composition
/// roots only, not test fixtures that compose a spec the same way. A test
/// module is a `#[cfg(test)]` line, any further one-line attributes, and
/// `mod <name> {`, all at column 0 and whatever the name. A rustfmt-formatted
/// module indents its whole body, so its closing brace is the first later
/// line that is exactly `}`. Taking the FIRST such line can only end a cut
/// early (a column-0 `}` inside a raw string), which leaves test code in and
/// fails an exact count loudly; it can never swallow production code. An
/// out-of-line `mod name;`, or an attribute this does not recognise, keeps
/// the text, which can only over-count.
fn production_source(source: &str) -> String {
    const CFG_TEST: &str = "#[cfg(test)]\n";
    let mut production = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find(CFG_TEST) {
        let at_line_start = start == 0 || rest[..start].ends_with('\n');
        let mut item = &rest[start + CFG_TEST.len()..];
        while item.starts_with("#[") {
            match item.find('\n') {
                Some(end) if item[..end].ends_with(']') => item = &item[end + 1..],
                _ => break,
            }
        }
        let name_len = item.strip_prefix("mod ").map_or(0, |name| {
            name.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(name.len())
        });
        let header_len = "mod ".len() + name_len + " {".len();
        if !at_line_start || name_len == 0 || !item["mod ".len() + name_len..].starts_with(" {\n") {
            // Not an inline test module: keep it and look further.
            let kept = start + CFG_TEST.len();
            production.push_str(&rest[..kept]);
            rest = &rest[kept..];
            continue;
        }
        production.push_str(&rest[..start]);
        // From the newline that opens the body, so an empty body closes too.
        let body = &item[header_len..];
        let end = body.find("\n}\n").expect("test module closes") + "\n}\n".len();
        rest = &body[end..];
    }
    production.push_str(rest);
    production
}

#[test]
fn bootstrap_constructor_installs_the_post_materialization_wrapper() {
    let constructor = RUNTIME_SOURCE
        .split("impl MobBootstrapSpec {")
        .nth(1)
        .expect("MobBootstrapSpec impl")
        .split("pub fn dispatch_taint_slot")
        .next()
        .expect("constructor section");
    assert!(constructor.contains("PreBuildMobSessionService"));
    assert!(constructor.contains("inner: session_service"));

    let delegated_create = RUNTIME_SOURCE
        .split("macro_rules! delegate_mob_session_service")
        .nth(1)
        .expect("delegation macro")
        .split("async fn start_turn")
        .next()
        .expect("create_session implementation");
    let prepare = delegated_create
        .find("prepare_create_request")
        .expect("capture resolved declaration before materialization");
    let materialize = delegated_create
        .find("self.inner.create_session")
        .expect("inner materialization");
    let evaluate = delegated_create
        .find("complete_create")
        .expect("post-materialization evaluation");
    assert!(prepare < materialize && materialize < evaluate);
}

#[test]
fn both_gateways_funnel_persistent_and_ephemeral_roots_through_the_choke() {
    assert_eq!(
        occurrences(
            &production_source(META_GATEWAY_SOURCE),
            "MobBootstrapSpec::new("
        ),
        2,
        "meta_gateway persistent and ephemeral roots must both use the common wrapper"
    );
    assert_eq!(
        occurrences(
            &production_source(RPC_GATEWAY_SOURCE),
            "MobBootstrapSpec::new("
        ),
        2,
        "rpc_gateway persistent and ephemeral roots must both use the common wrapper"
    );
}

/// The production view strips exactly the test module: every production
/// root survives, and a fixture inside `mod tests` is not counted.
#[test]
fn production_source_strips_only_the_test_module() {
    let source = "fn root() { MobBootstrapSpec::new(a) }\n#[cfg(test)]\nmod tests {\n    fn fixture() {\n        MobBootstrapSpec::new(b);\n    }\n}\nfn later_root() { MobBootstrapSpec::new(c) }\n";
    let production = production_source(source);
    assert_eq!(occurrences(&production, "MobBootstrapSpec::new("), 2);
    assert!(production.contains("new(a)") && production.contains("new(c)"));
    assert!(!production.contains("fixture"));
}

/// Every test module is stripped whatever its name, also behind further
/// attributes, while the production roots before, between and after them
/// survive. An out-of-line `mod name;` has no body and strips nothing.
#[test]
fn production_source_strips_every_test_module() {
    let source = concat!(
        "fn root() { MobBootstrapSpec::new(a) }\n",
        "#[cfg(test)]\nmod tests {\n    fn fixture() {\n        MobBootstrapSpec::new(b);\n    }\n}\n",
        "fn middle_root() { MobBootstrapSpec::new(c) }\n",
        "#[cfg(test)]\nmod callback_tests {\n    fn other_fixture() {\n        MobBootstrapSpec::new(d);\n    }\n}\n",
        "#[cfg(test)]\n#[allow(clippy::expect_used)]\nmod allowed_tests {\n    fn allowed_fixture() {\n        MobBootstrapSpec::new(e);\n    }\n}\n",
        "#[cfg(test)]\nmod declared_elsewhere;\n",
        "fn later_root() { MobBootstrapSpec::new(f) }\n",
    );
    let production = production_source(source);
    assert_eq!(occurrences(&production, "MobBootstrapSpec::new("), 3);
    for root in ["new(a)", "new(c)", "new(f)"] {
        assert!(production.contains(root), "production root {root} survives");
    }
    assert!(!production.contains("fixture"));
    assert!(production.contains("mod declared_elsewhere;"));
}

#[test]
fn unified_builder_roots_funnel_through_wrapped_stock_constructors() {
    assert!(
        BUILDER_SOURCE.contains("MobBootstrapSpec::persistent_inner_with_provider_stores("),
        "builder root must retain the persistent stock constructor"
    );
    // Both ephemeral roots (definition-based and scratch) use the provider-
    // store constructor, which threads the host's native grant configuration.
    assert_eq!(
        occurrences(
            BUILDER_SOURCE,
            "MobBootstrapSpec::ephemeral_runtime_backed_with_provider_stores("
        ),
        2,
        "builder roots must retain both ephemeral stock constructors"
    );
    // The persistent root chains its mob storage provenance onto the
    // wrapped spec, so the call is matched without its statement end.
    assert_eq!(
        occurrences(
            RUNTIME_SOURCE,
            "Self::with_base_layer(definition, storage, session_service, no_op_pre_build_hook())"
        ),
        3,
        "every stock constructor used by the builder must install the common wrapper"
    );
    // Each wraps the concrete service first, with its own hook followed by
    // the refusal of an unresolved document MCP marker.
    assert_eq!(
        occurrences(
            RUNTIME_SOURCE,
            "let hook = refusing_unresolved_document_markers(hook);"
        ),
        3,
        "every stock constructor must refuse an unresolved marker after its hook"
    );
}

#[test]
fn wrapper_does_not_skip_resume_materializations() {
    let delegation = RUNTIME_SOURCE
        .split("macro_rules! delegate_mob_session_service")
        .nth(1)
        .expect("delegation macro")
        .split("delegate_mob_session_service!(PreBuildMobSessionService)")
        .next()
        .expect("delegation implementation");
    assert!(
        !delegation.contains("resume_session.is_some"),
        "capability evaluation must not be gated out for either fresh or resumed requests"
    );
    assert_eq!(
        occurrences(delegation, "prepare_create_request(req).await?"),
        6,
        "ordinary, runtime-boundary, exact-witness, and archived-resume creation paths must all capture declared intent"
    );
    assert_eq!(
        occurrences(
            delegation,
            "complete_create(result, context, capability_context)"
        ),
        6,
        "every creation path must compare after successful materialization"
    );
}

/// Both production `MobSessionService` wrappers forward the cold-predecessor
/// settlement to the service they wrap. Its trait default refuses typed and
/// touches nothing, so a wrapper without the forward compiles and silently
/// refuses every settlement. The behavioral row is
/// `mob_session_wrappers_forward_the_cold_predecessor_settlement`.
#[test]
fn both_wrappers_forward_the_cold_predecessor_settlement() {
    const FORWARD: &str = ".settle_cold_predecessor_with_mob_lifecycle_authority_before(";
    let production = production_source(RUNTIME_SOURCE);
    let delegation = production
        .split("macro_rules! delegate_mob_session_service")
        .nth(1)
        .expect("delegation macro")
        .split("delegate_mob_session_service!(PreBuildMobSessionService)")
        .next()
        .expect("delegation implementation");
    assert_eq!(
        occurrences(delegation, FORWARD),
        1,
        "the delegation macro forwards the settlement to its inner service"
    );
    let after_create = production
        .split("impl MobSessionService for AfterCreateMobSessionService {")
        .nth(1)
        .expect("after-create wrapper")
        .split("\n}\n")
        .next()
        .expect("after-create implementation");
    assert_eq!(
        occurrences(after_create, FORWARD),
        1,
        "the after-create wrapper forwards the settlement to its inner service"
    );
}
