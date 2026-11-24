//! Structural mutation gates for the declared-versus-resolved capability
//! invariant's first-party composition coverage.
//!
//! Behavioral fresh/resume and category-mutation tests live beside the
//! private wrapper implementation. These assertions make the production
//! gateway roots fail if either persistent or ephemeral composition bypasses
//! `MobBootstrapSpec::new` (a root calls it, or `MobBootstrapSpec::declared`,
//! which calls it), or if the wrapper stops evaluating after the inner
//! service has materialized the live catalog.
//!
//! A launch over a declared store hands its wrapped service to the launch's
//! composition, and the mob set builds the launch mob through that
//! composition: both lines of that hand-over are pinned here. Not read by
//! this guard: what the mob set's own build of a mob does with the builder
//! the composition returns. Not seen by it either: a root that builds the
//! spec by a struct literal, and a root built in another file than the two
//! gateways' sources.

#![allow(clippy::expect_used, clippy::panic)]

const RUNTIME_SOURCE: &str = include_str!("../src/mob_handle_runtime.rs");
const BUILDER_SOURCE: &str = include_str!("../src/unified_runtime/builder.rs");
const META_GATEWAY_SOURCE: &str = include_str!("../src/bin/meta_gateway.rs");
/// The stdin-RPC gateway lives in the library; its binary only delegates.
const RPC_GATEWAY_SOURCE: &str = include_str!("../src/rpc_gateway.rs");

fn occurrences(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

/// Every call of a `MobBootstrapSpec` constructor in `source`, by the
/// constructor's name: each `MobBootstrapSpec::<name>(`. A mention without
/// the opening parenthesis (a comment that names a constructor) is no call.
fn spec_constructor_calls(source: &str) -> std::collections::BTreeMap<&str, usize> {
    const PREFIX: &str = "MobBootstrapSpec::";
    let mut calls = std::collections::BTreeMap::new();
    for (at, _) in source.match_indices(PREFIX) {
        let rest = &source[at + PREFIX.len()..];
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if name_len > 0 && rest[name_len..].starts_with('(') {
            *calls.entry(&rest[..name_len]).or_insert(0) += 1;
        }
    }
    calls
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

/// Both gateways build their persistent and their ephemeral root through
/// the constructor that installs the wrapper: `MobBootstrapSpec::new`, or
/// `MobBootstrapSpec::declared`, which is `new` over a store that is
/// declared and not opened. The counts are exact and name every constructor
/// the production text calls, so a root built by another constructor, and a
/// root added or removed, fails here.
#[test]
fn both_gateways_funnel_persistent_and_ephemeral_roots_through_the_choke() {
    let declared = RUNTIME_SOURCE
        .split("pub fn declared(")
        .nth(1)
        .expect("the declared constructor")
        .split("\n    }\n")
        .next()
        .expect("the declared constructor's body");
    assert!(
        declared.contains("Self::new("),
        "the declared constructor must build its spec through the common wrapper: {declared}"
    );
    assert!(
        !declared.contains("session_service =") && !declared.contains("with_session_service"),
        "the declared constructor must keep the session service the common wrapper built: \
         {declared}"
    );

    let meta_gateway = production_source(META_GATEWAY_SOURCE);
    assert_eq!(
        spec_constructor_calls(&meta_gateway),
        std::collections::BTreeMap::from([("declared", 2)]),
        "meta_gateway persistent and ephemeral roots must both use the common wrapper: each \
         declares its mob's store"
    );
    assert_eq!(
        occurrences(&meta_gateway, "MobBootstrapSpec::"),
        3,
        "meta_gateway names the spec's paths three times: its two roots, and one comment \
         that says what `MobBootstrapSpec::ephemeral` does not accept; any other path is a \
         root or a hand-over this guard has not judged"
    );
    let rpc_gateway = production_source(RPC_GATEWAY_SOURCE);
    assert_eq!(
        spec_constructor_calls(&rpc_gateway),
        std::collections::BTreeMap::from([("declared", 2), ("new", 1)]),
        "rpc_gateway persistent and ephemeral roots must both use the common wrapper: the \
         persistent root declares its mob's store or opens it itself, the ephemeral root \
         declares a store in memory"
    );
    assert_eq!(
        occurrences(&rpc_gateway, "MobBootstrapSpec::"),
        3,
        "rpc_gateway names the spec's paths three times, each a root's call; any other path \
         is a root or a hand-over this guard has not judged"
    );
}

/// A launch hands the service the common wrapper built to its composition,
/// and the composition gives it to every mob the launch's mob set builds:
/// the launch mob of a declared store and each mob beside it.
#[test]
fn the_launchs_composition_takes_and_hands_on_the_wrapped_service() {
    let made = RUNTIME_SOURCE
        .split("fn launch_mob_composition(&self) -> LaunchMobComposition {")
        .nth(1)
        .expect("the launch's composition is made from the spec")
        .split("\n    }\n")
        .next()
        .expect("the function's body");
    assert!(
        made.contains("session_service: self.session_service.clone(),"),
        "the composition must take the spec's own session service: {made}"
    );
    let configure = RUNTIME_SOURCE
        .split("impl LaunchMobComposition {")
        .nth(1)
        .expect("the composition's own functions")
        .split("\n    fn configure(")
        .nth(1)
        .expect("the composition configures a mob's builder")
        .split("\n    }\n")
        .next()
        .expect("the function's body");
    assert!(
        configure.contains(".with_session_service(self.session_service.clone())"),
        "every mob the composition builds must take the composition's session service"
    );
}

/// The count of constructor calls is by name and skips a mention that is no
/// call.
#[test]
fn spec_constructor_calls_are_counted_by_name() {
    let source = "MobBootstrapSpec::declared(a); MobBootstrapSpec::new(b);\n// \
                  MobBootstrapSpec::ephemeral is not called\nMobBootstrapSpec::declared(c)";
    assert_eq!(
        spec_constructor_calls(source),
        std::collections::BTreeMap::from([("declared", 2), ("new", 1)])
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

/// Both production `MobSessionService` wrappers forward a mob member's
/// operator-instruction delivery, its release, its read and a dormant resume
/// seat's member binding to the service they wrap. Their trait defaults
/// refuse typed (`durability_unavailable`, `Unsupported`), so a wrapper
/// without a forward compiles and blocks every member's operator
/// instructions behind it, or refuses its dormant resume seats over a host's
/// session.
#[test]
fn both_wrappers_forward_the_member_instruction_methods() {
    const FORWARDS: [&str; 4] = [
        ".record_member_instruction_target(",
        ".release_member_instruction_target(",
        ".read_member_instructions(",
        ".bind_seated_member_session(",
    ];
    let production = production_source(RUNTIME_SOURCE);
    let delegation = production
        .split("macro_rules! delegate_mob_session_service")
        .nth(1)
        .expect("delegation macro")
        .split("delegate_mob_session_service!(PreBuildMobSessionService)")
        .next()
        .expect("delegation implementation");
    let after_create = production
        .split("impl MobSessionService for AfterCreateMobSessionService {")
        .nth(1)
        .expect("after-create wrapper")
        .split("\n}\n")
        .next()
        .expect("after-create implementation");
    for forward in FORWARDS {
        assert_eq!(
            occurrences(delegation, forward),
            1,
            "the delegation macro forwards {forward} to its inner service"
        );
        assert_eq!(
            occurrences(after_create, forward),
            1,
            "the after-create wrapper forwards {forward} to its inner service"
        );
    }
}

/// Both production `MobSessionService` wrappers forward the cold settle of a
/// member session's owed compaction projections, and the stdin-RPC gateway's
/// agent builder forwards the builder seam that settle finalizes through.
/// The trait defaults refuse both typed for a service that keeps durable
/// sessions, so a wrapper without the forward compiles and leaves the
/// compaction a dormant member owes unsettled when its session ends.
#[test]
fn every_wrapper_forwards_the_owed_compaction_settle() {
    const SETTLE: &str = ".settle_owed_compaction_projections(";
    const FINALIZE: &str = ".finalize_absent_session_compaction_stages(";
    let production = production_source(RUNTIME_SOURCE);
    let delegation = production
        .split("macro_rules! delegate_mob_session_service")
        .nth(1)
        .expect("delegation macro")
        .split("delegate_mob_session_service!(PreBuildMobSessionService)")
        .next()
        .expect("delegation implementation");
    let after_create = production
        .split("impl MobSessionService for AfterCreateMobSessionService {")
        .nth(1)
        .expect("after-create wrapper")
        .split("\n}\n")
        .next()
        .expect("after-create implementation");
    assert_eq!(
        occurrences(delegation, SETTLE),
        1,
        "the delegation macro forwards the owed compaction settle to its inner service"
    );
    assert_eq!(
        occurrences(after_create, SETTLE),
        1,
        "the after-create wrapper forwards the owed compaction settle to its inner service"
    );
    assert_eq!(
        occurrences(&production_source(RPC_GATEWAY_SOURCE), FINALIZE),
        1,
        "the stdin-RPC gateway's agent builder forwards the compaction stage finalization"
    );
}
