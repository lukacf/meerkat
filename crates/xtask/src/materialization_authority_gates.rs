//! Member-materialization authority AST gates (run by `xtask rmat-audit`).
//!
//! `MobMachine` owns whether a member is materialized in this process
//! (`member_materialization`: Dormant, Waking, Live). Three rules keep the
//! shell from deciding that fact beside the machine:
//!
//! - `PeerNotFoundConstructionOutsideAllowlist`: `InprocSendError::PeerNotFound`
//!   and `SendError::PeerNotFound` mean "not a member". They are constructed
//!   only at the registered sites, each with its own reason: the inproc
//!   registry misses (reached only after a wake-route miss or for a
//!   non-member), the router's trust miss (unreachable for a seated, wired
//!   member) and the two mappings. A new construction site fails until it is
//!   registered with its row.
//! - `MobLiveSessionObservationOutsideAllowlist`: meerkat-mob reads
//!   `live_session_actor_registered` / `has_live_session` only at the
//!   registered sites: the drift observation, the wake path's own live
//!   observations, raw observations a machine input carries, and named
//!   mechanics. Everywhere else residency comes from the machine.
//! - `MobMemberSessionDiscardOutsideReleaseSeam`: meerkat-mob discards a
//!   member's live session (`discard_live_session*`) only inside the one
//!   mob-owned seam that applies `ReleaseMemberMaterialization`
//!   (`discard_member_session_with_release`), or inside the session-service
//!   adapter file that defines that API (keyed by its path: a fn's name never
//!   exempts it).
//!
//! Sites are keyed by file path suffix and enclosing fn name, never by line.
//! A `Pending` allowance is a known site that must move behind the machine;
//! it reports a warning until the site is rewritten and the entry removed. An
//! allowance that matches nothing reports a stale warning. Unregistered
//! sites are errors. Pattern positions (match arms, `if let`) and
//! `#[cfg(test)]` items are not constructions or reads and are ignored, as
//! are test source files.

use std::collections::BTreeSet;

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, ExprMethodCall, ExprPath, ExprStruct, ImplItemFn, ItemFn, TraitItemFn};

use crate::rmat_audit::{Finding, error_finding, warn_finding};

pub(crate) const PEER_NOT_FOUND_RULE: &str = "PeerNotFoundConstructionOutsideAllowlist";
pub(crate) const LIVE_SESSION_RULE: &str = "MobLiveSessionObservationOutsideAllowlist";
pub(crate) const DISCARD_RULE: &str = "MobMemberSessionDiscardOutsideReleaseSeam";

/// The one mob-owned seam allowed to discard a member's live session; it
/// applies `ReleaseMemberMaterialization` in the same step.
pub(crate) const RELEASE_SEAM_FN: &str = "discard_member_session_with_release";

/// Session-service API names whose read decides nothing but observes a live
/// session actor.
const LIVE_SESSION_OBSERVATIONS: &[&str] = &["live_session_actor_registered", "has_live_session"];

/// Prefix of every session-service API that discards a live session.
const DISCARD_PREFIX: &str = "discard_live_session";

/// The session-service adapter file: it defines and forwards the discard API
/// and rolls back its own create-time materialization, so its discards are
/// the API itself, never a residency decision.
const SESSION_SERVICE_ADAPTER_PATH: &str = "crates/meerkat-mob/src/runtime/session_service.rs";

/// The crate whose shell must read residency from `MobMachine`.
const MOB_SOURCE_PREFIX: &str = "crates/meerkat-mob/src/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllowanceStatus {
    /// A settled site: mechanics, a declared observation or a declared row.
    Settled,
    /// A known site that must move behind the machine (or the release seam);
    /// it warns until the site is rewritten and the entry is removed.
    Pending,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SiteAllowance {
    pub path_suffix: &'static str,
    pub enclosing_fn: &'static str,
    pub reason: &'static str,
    pub status: AllowanceStatus,
}

const fn settled(
    path_suffix: &'static str,
    enclosing_fn: &'static str,
    reason: &'static str,
) -> SiteAllowance {
    SiteAllowance {
        path_suffix,
        enclosing_fn,
        reason,
        status: AllowanceStatus::Settled,
    }
}

const fn pending(
    path_suffix: &'static str,
    enclosing_fn: &'static str,
    reason: &'static str,
) -> SiteAllowance {
    SiteAllowance {
        path_suffix,
        enclosing_fn,
        reason,
        status: AllowanceStatus::Pending,
    }
}

/// Every registered `PeerNotFound` construction site, each with the reason
/// it may construct one. An entry that matches nothing only warns.
pub(crate) const PEER_NOT_FOUND_CONSTRUCTIONS: &[SiteAllowance] = &[
    settled(
        "crates/meerkat-comms/src/inproc.rs",
        "send_resolving_wake",
        "neither a live nor a wake route: a non-member, or a member of another namespace",
    ),
    settled(
        "crates/meerkat-comms/src/inproc.rs",
        "await_wake",
        "a stale wake route whose owner denies membership (the NotAMember wake denial)",
    ),
    settled(
        "crates/meerkat-comms/src/router.rs",
        "map_inproc_send_error",
        "maps the inproc PeerNotFound above to the router error, nothing else",
    ),
    settled(
        "crates/meerkat-comms/src/router.rs",
        "send_with_id",
        "trust miss, unreachable for a seated, wired member with an endpoint. A seated member with no durable endpoint has no key and no trust row yet; the owed boot retry records its endpoint and publishes its wake route, and until then a peer cannot address it",
    ),
    settled(
        "crates/meerkat-comms/src/runtime/comms_runtime.rs",
        "send_peer_command_with_id",
        "maps the router PeerNotFound to the public SendError, nothing else",
    ),
    settled(
        "crates/meerkat-comms/src/mcp/tools.rs",
        "ensure_peer_id_is_trusted",
        "the tool-boundary trust miss; a seated member with no durable endpoint has no key and no trust row until the owed boot retry records its endpoint",
    ),
    settled(
        "crates/meerkat-comms/src/mcp/tools.rs",
        "dispatch",
        "maps the runtime PeerNotFound to the tool text, nothing else",
    ),
    settled(
        "crates/meerkat-comms/src/mcp/tools.rs",
        "format_router_send_error",
        "maps the router PeerNotFound to the tool text, nothing else",
    ),
];

/// Fns that construct `PeerNotFound` on behalf of their callers: a call of
/// one is itself a construction site and needs its own entry above. The
/// inproc resolution constructs it inline since the wake-route merge.
pub(crate) const PEER_NOT_FOUND_CONSTRUCTOR_HELPERS: &[(&str, &str)] = &[];

/// The tool-boundary text of a not-found peer. Comms source that writes it
/// constructs the not-found outcome without the enum.
const PEER_NOT_FOUND_TEXT_PREFIX: &str = "peer_not_found_or_not_trusted:";

/// The crate whose source decides comms reachability.
const COMMS_SOURCE_PREFIX: &str = "crates/meerkat-comms/src/";

/// Every registered live-session observation in meerkat-mob.
pub(crate) const MOB_LIVE_SESSION_OBSERVATIONS: &[SiteAllowance] = &[
    settled(
        "crates/meerkat-mob/src/runtime/session_service.rs",
        "live_session_actor_registered",
        "session-service adapter: defines and forwards the observation, decides nothing",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "run_local_turn_readiness",
        "drift observation (arm c): the host-turn readiness probe hands the raw observation to the machine",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor/reload_revival.rs",
        "begin_member_live_revival",
        "the wake path's own live observation",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor/reload_revival.rs",
        "commit_warm_revival",
        "the wake path's own live observation",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "spawn_explicit_resume_member_observation",
        "raw observation the machine classifies through ClassifyExplicitResumeMemberLive",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "autonomous_shutdown_member_action",
        "raw observation session_has_live_actor the machine decides through ResolveAutonomousShutdownMemberAction",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "member_session_present",
        "retirement disposal reads session presence (live or persisted), not residency",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "restore_member_local_cleanup_delivery",
        "boot cleanup-delivery restore observes live, durable and residue state of the named session",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "enqueue_spawn_with_origin",
        "spawn resume mechanics: refuse to reuse or race a live actor hosting the exact resume session",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/builder.rs",
        "reconcile_resume",
        "boot orphan-session archive observes sessions that machine bindings do not name",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/host_materialize.rs",
        "acquire_vacant_materialization_boundary",
        "member-host materialization mechanics: a placed member's liveness is host-owned",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/host_materialize.rs",
        "session_live",
        "member-host materialization mechanics: a placed member's liveness is host-owned",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/host_materialize.rs",
        "quiesce_nonserving_incarnation_until",
        "member-host materialization mechanics: a placed member's liveness is host-owned",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/host_materialize.rs",
        "materialize",
        "member-host materialization mechanics: a placed member's liveness is host-owned",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "retire_exact_attachment_for_explicit_resume_owned",
        "provisioner attachment mechanics at the provision boundary",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "finish_retained_resume_durable_convergence",
        "provisioner durable-convergence mechanics at the provision boundary",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/handle.rs",
        "wake_member",
        "the drift observation: the probe only feeds observed_missing to WakeMember (Operator or HostDelivery), whose verdict decides",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor/member_wake_turn.rs",
        "plan_member_wake",
        "the wake path's own live observation (as begin_member_live_revival): inside the authorized attempt, a session actor or successor still live in this process is kept, never built twice",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor/member_wake.rs",
        "member_materialization_served",
        "the drift observation, taken again on the actor: a member MobMachine holds Live is answered ready while its registration serves it (an executor and a live session actor); otherwise the miss goes to the drift arm (ClassifyMemberLiveMaterialization), whose verdict decides",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "serving_member_binding_registry",
        "explicit-resume lift mechanics: the operation registry of a registration that already serves the member's exact binding is reused only while its session actor is present, else the full restore runs; no residency is decided",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "session_has_retained_materialization",
        "explicit-resume seam mechanics: a member MobMachine holds Dormant is a candidate only while its session still holds something this process left behind (an executor attachment, or a live session actor from an earlier handle of the mob), which the seam retires before the durable Resume; it reads only in-memory state, never the store, so a lazy resume stays lazy, and decides no member state (the member stays Dormant for the lift's plan)",
    ),
    pending(
        "crates/meerkat-mob/src/runtime/actor.rs",
        "realize_restored_member_operation_binding",
        "the actor-start operation-binding pass reads the member phase (Live only)",
    ),
    pending(
        "crates/meerkat-mob/src/runtime/member_instruction.rs",
        "activate_member_instruction",
        "decide residency from member_materialization (or wake), not from a session probe",
    ),
];

/// Every registered member-session discard in meerkat-mob besides the release
/// seam and the session-service adapters.
pub(crate) const MOB_MEMBER_SESSION_DISCARDS: &[SiteAllowance] = &[
    settled(
        "crates/meerkat-mob/src/runtime/live_actor_binding.rs",
        "retire_conflicting_live_actor",
        "binding-conflict retire: a restore or a wake found the member's live actor (or live successor) on another auth binding than its spec, so the member is recorded broken (no phase to release, or its own wake attempt, where a release would be Stale); its exact runtime registration is unregistered to terminal first, then its live actor is discarded, so no surface reaches the old binding",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "retire_exact_attachment_for_explicit_resume_owned",
        "explicit-resume lift: the member is Waking under the resume's own attempt, so a release would be Stale; the attempt's resolution decides the phase",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "create_attached_session_actor_recovery_owned",
        "in-attempt rollback: the member is Waking (or not yet seated), so a release would be Stale; the attempt's Retryable, Abandon or Failed verdict decides the phase",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "discard_actor_then_roll_back_after_boundary",
        "in-attempt provision rollback: the member is Waking, so a release would be Stale; the attempt's verdict decides the phase",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "cleanup",
        "pending-provision cleanup inside an open attempt or for an unseated spawn: a release would be Stale; the attempt's verdict decides the phase",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "reload_degraded_runtime_registration",
        "durability reload keeps the registration: the reload's drift classification (ClassifyMemberLiveMaterialization, a counted fault path) moves the Live member to Waking under its own attempt",
    ),
    settled(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "create_dormant_member_session",
        "fresh-seat create: the empty session's agent is built standalone, with no runtime registration, and discarded at once; it never was a member materialization (the identity is not seated yet, or a rotation seats it on this session later), so a release has nothing to release; the seat or rotation commit seats it Dormant",
    ),
];

/// Which allowance list a rule reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GateAllowlists<'a> {
    pub peer_not_found: &'a [SiteAllowance],
    pub peer_not_found_helpers: &'a [(&'static str, &'static str)],
    pub live_session: &'a [SiteAllowance],
    pub discard: &'a [SiteAllowance],
}

impl GateAllowlists<'static> {
    pub(crate) fn canonical() -> Self {
        Self {
            peer_not_found: PEER_NOT_FOUND_CONSTRUCTIONS,
            peer_not_found_helpers: PEER_NOT_FOUND_CONSTRUCTOR_HELPERS,
            live_session: MOB_LIVE_SESSION_OBSERVATIONS,
            discard: MOB_MEMBER_SESSION_DISCARDS,
        }
    }
}

/// Runs the three rules file by file and remembers which allowances matched,
/// so the stale ones can be reported once every file was seen.
pub(crate) struct MaterializationAuthorityGates<'a> {
    allowlists: GateAllowlists<'a>,
    matched: BTreeSet<(&'static str, &'static str, &'static str)>,
}

impl<'a> MaterializationAuthorityGates<'a> {
    pub(crate) fn new(allowlists: GateAllowlists<'a>) -> Self {
        Self {
            allowlists,
            matched: BTreeSet::new(),
        }
    }

    /// Findings for one parsed workspace file (`relative` is repo-relative).
    pub(crate) fn file_findings(
        &mut self,
        relative: &str,
        source: &str,
        parsed: &syn::File,
    ) -> Vec<Finding> {
        if crate::rmat_audit::is_test_source_path(relative) {
            return Vec::new();
        }
        let helpers: Vec<&'static str> = self
            .allowlists
            .peer_not_found_helpers
            .iter()
            .filter(|(path_suffix, _)| relative.ends_with(path_suffix))
            .map(|(_, name)| *name)
            .collect();
        let wants_text = relative.starts_with(COMMS_SOURCE_PREFIX)
            && source.contains(PEER_NOT_FOUND_TEXT_PREFIX);
        let wants_peer_not_found =
            source.contains("PeerNotFound") || !helpers.is_empty() || wants_text;
        let in_mob = relative.starts_with(MOB_SOURCE_PREFIX);
        let wants_live_session = in_mob
            && LIVE_SESSION_OBSERVATIONS
                .iter()
                .any(|name| source.contains(name));
        let wants_discard = in_mob && source.contains(DISCARD_PREFIX);
        if !wants_peer_not_found && !wants_live_session && !wants_discard {
            return Vec::new();
        }
        let mut file = parsed.clone();
        crate::rmat_audit::strip_cfg_test_items(&mut file.items);
        let mut visitor = SiteVisitor {
            wants_peer_not_found,
            wants_text,
            helpers,
            wants_live_session,
            wants_discard,
            fn_stack: Vec::new(),
            sites: Vec::new(),
        };
        visitor.visit_file(&file);

        let mut findings = Vec::new();
        for site in visitor.sites {
            let (rule, list) = match site.kind {
                SiteKind::PeerNotFound => (PEER_NOT_FOUND_RULE, self.allowlists.peer_not_found),
                SiteKind::LiveSessionObservation => {
                    (LIVE_SESSION_RULE, self.allowlists.live_session)
                }
                SiteKind::MemberSessionDiscard => (DISCARD_RULE, self.allowlists.discard),
            };
            let enclosing = site.enclosing_fn.as_str();
            let symbol = format!("{enclosing}::{}", site.callee);
            if matches!(site.kind, SiteKind::MemberSessionDiscard)
                && (enclosing == RELEASE_SEAM_FN
                    || relative.ends_with(SESSION_SERVICE_ADAPTER_PATH))
            {
                // The release seam itself, or the session-service adapter
                // file that defines the discard API (by path: a fn named like
                // the API elsewhere is no adapter).
                continue;
            }
            let allowance = list.iter().find(|allowance| {
                relative.ends_with(allowance.path_suffix) && allowance.enclosing_fn == enclosing
            });
            match allowance {
                Some(allowance) => {
                    self.matched
                        .insert((rule, allowance.path_suffix, allowance.enclosing_fn));
                    if allowance.status == AllowanceStatus::Pending {
                        findings.push(warn_finding(
                            rule,
                            relative,
                            &symbol,
                            format!(
                                "pending site `{}` (line {}): {}",
                                site.callee, site.line, allowance.reason
                            ),
                            false,
                        ));
                    }
                }
                None => findings.push(error_finding(
                    rule,
                    relative,
                    &symbol,
                    site.kind
                        .unregistered_message(&site.callee, enclosing, site.line),
                    false,
                )),
            }
        }
        findings
    }

    /// One warning per allowance that matched no site in any file.
    pub(crate) fn stale_findings(&self) -> Vec<Finding> {
        let mut findings = Vec::new();
        for (rule, list) in [
            (PEER_NOT_FOUND_RULE, self.allowlists.peer_not_found),
            (LIVE_SESSION_RULE, self.allowlists.live_session),
            (DISCARD_RULE, self.allowlists.discard),
        ] {
            for allowance in list {
                if !self
                    .matched
                    .contains(&(rule, allowance.path_suffix, allowance.enclosing_fn))
                {
                    findings.push(warn_finding(
                        rule,
                        allowance.path_suffix,
                        allowance.enclosing_fn,
                        format!(
                            "stale allowance: `{}` in `{}` no longer has a matching site; remove the entry ({})",
                            allowance.enclosing_fn, allowance.path_suffix, allowance.reason
                        ),
                        false,
                    ));
                }
            }
        }
        findings
    }
}

#[derive(Debug, Clone, Copy)]
enum SiteKind {
    PeerNotFound,
    LiveSessionObservation,
    MemberSessionDiscard,
}

impl SiteKind {
    fn unregistered_message(self, callee: &str, enclosing: &str, line: usize) -> String {
        match self {
            Self::PeerNotFound => format!(
                "`{callee}` is constructed in `{enclosing}` (line {line}), which is not a registered PeerNotFound site. PeerNotFound means \"not a member\"; a seated member's miss takes the wake route. Register the site with its reason in materialization_authority_gates.rs, or return the typed wake refusal"
            ),
            Self::LiveSessionObservation => format!(
                "`{callee}` is read in `{enclosing}` (line {line}), which is not a registered live-session observation. Member residency is MobMachine state (member_materialization); read the phase or apply WakeMember instead"
            ),
            Self::MemberSessionDiscard => format!(
                "`{callee}` discards a member session in `{enclosing}` (line {line}), outside the release seam `{RELEASE_SEAM_FN}`. A discard that keeps the member seated must apply ReleaseMemberMaterialization in the same step"
            ),
        }
    }
}

struct Site {
    kind: SiteKind,
    callee: String,
    enclosing_fn: String,
    line: usize,
}

struct SiteVisitor {
    wants_peer_not_found: bool,
    /// Also report tool-boundary not-found text (comms source only).
    wants_text: bool,
    /// Constructor helpers of this file: a call of one is a construction.
    helpers: Vec<&'static str>,
    wants_live_session: bool,
    wants_discard: bool,
    fn_stack: Vec<String>,
    sites: Vec<Site>,
}

impl SiteVisitor {
    fn enclosing_fn(&self) -> String {
        self.fn_stack
            .last()
            .cloned()
            .unwrap_or_else(|| "<module>".to_string())
    }

    fn push(&mut self, kind: SiteKind, callee: String, span: proc_macro2::Span) {
        let enclosing_fn = self.enclosing_fn();
        self.sites.push(Site {
            kind,
            callee,
            enclosing_fn,
            line: span.start().line,
        });
    }

    /// `InprocSendError::PeerNotFound` or `SendError::PeerNotFound` in value
    /// position (a call, a constructor passed as a value, or a struct literal).
    fn peer_not_found_constructor(path: &syn::Path) -> Option<String> {
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        let [.., owner, variant] = segments.as_slice() else {
            return None;
        };
        if variant == "PeerNotFound" && (owner == "InprocSendError" || owner == "SendError") {
            Some(format!("{owner}::{variant}"))
        } else {
            None
        }
    }

    /// Whether a token stream holds a string literal that starts with the
    /// tool-boundary not-found text (macro bodies such as `format!`).
    fn tokens_write_not_found_text(tokens: proc_macro2::TokenStream) -> bool {
        tokens.into_iter().any(|token| match token {
            proc_macro2::TokenTree::Literal(literal) => literal
                .to_string()
                .starts_with(&format!("\"{PEER_NOT_FOUND_TEXT_PREFIX}")),
            proc_macro2::TokenTree::Group(group) => {
                Self::tokens_write_not_found_text(group.stream())
            }
            _ => false,
        })
    }

    fn classify_callee(&mut self, name: &str, span: proc_macro2::Span) {
        if self.wants_peer_not_found && self.helpers.contains(&name) {
            self.push(SiteKind::PeerNotFound, format!("{name}()"), span);
        }
        if self.wants_live_session && LIVE_SESSION_OBSERVATIONS.contains(&name) {
            self.push(SiteKind::LiveSessionObservation, name.to_string(), span);
        }
        if self.wants_discard && name.starts_with(DISCARD_PREFIX) {
            self.push(SiteKind::MemberSessionDiscard, name.to_string(), span);
        }
    }
}

impl<'ast> Visit<'ast> for SiteVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        visit::visit_trait_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_expr_path(&mut self, node: &'ast ExprPath) {
        if self.wants_peer_not_found
            && let Some(callee) = Self::peer_not_found_constructor(&node.path)
        {
            self.push(SiteKind::PeerNotFound, callee, node.span());
        }
        visit::visit_expr_path(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast ExprStruct) {
        if self.wants_peer_not_found
            && let Some(callee) = Self::peer_not_found_constructor(&node.path)
        {
            self.push(SiteKind::PeerNotFound, callee, node.span());
        }
        visit::visit_expr_struct(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if self.wants_text && Self::tokens_write_not_found_text(node.tokens.clone()) {
            self.push(
                SiteKind::PeerNotFound,
                PEER_NOT_FOUND_TEXT_PREFIX.to_string(),
                node.span(),
            );
        }
        visit::visit_macro(self, node);
    }

    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        if self.wants_text && node.value().starts_with(PEER_NOT_FOUND_TEXT_PREFIX) {
            self.push(
                SiteKind::PeerNotFound,
                PEER_NOT_FOUND_TEXT_PREFIX.to_string(),
                node.span(),
            );
        }
        visit::visit_lit_str(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let name = node.method.to_string();
        self.classify_callee(&name, node.method.span());
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        // Path-call form: `SessionService::has_live_session(service, id)`.
        if let Expr::Path(path) = node.func.as_ref()
            && let Some(last) = path.path.segments.last()
        {
            let name = last.ident.to_string();
            self.classify_callee(&name, last.ident.span());
        }
        visit::visit_expr_call(self, node);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;

    const FIXTURE_PEER_NOT_FOUND: &[SiteAllowance] = &[
        settled(
            "crates/meerkat-comms/src/router.rs",
            "registered_miss",
            "fixture row",
        ),
        settled(
            "crates/meerkat-comms/src/inproc.rs",
            "peer_not_found",
            "fixture constructor helper",
        ),
        settled(
            "crates/meerkat-comms/src/inproc.rs",
            "resolve",
            "fixture helper caller",
        ),
        settled(
            "crates/meerkat-comms/src/mcp/tools.rs",
            "trust_miss",
            "fixture tool-boundary miss",
        ),
    ];
    const FIXTURE_LIVE_SESSION: &[SiteAllowance] = &[
        settled(
            "crates/meerkat-mob/src/runtime/actor.rs",
            "drift_probe",
            "fixture drift observation",
        ),
        pending(
            "crates/meerkat-mob/src/runtime/handle.rs",
            "ensure_live",
            "fixture pending site",
        ),
        settled(
            "crates/meerkat-mob/src/runtime/actor.rs",
            "never_present",
            "fixture stale entry",
        ),
    ];
    const FIXTURE_DISCARD: &[SiteAllowance] = &[pending(
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        "rollback",
        "fixture pending discard",
    )];

    const FIXTURE_HELPERS: &[(&str, &str)] =
        &[("crates/meerkat-comms/src/inproc.rs", "peer_not_found")];

    fn fixture_gates() -> MaterializationAuthorityGates<'static> {
        MaterializationAuthorityGates::new(GateAllowlists {
            peer_not_found: FIXTURE_PEER_NOT_FOUND,
            peer_not_found_helpers: FIXTURE_HELPERS,
            live_session: FIXTURE_LIVE_SESSION,
            discard: FIXTURE_DISCARD,
        })
    }

    fn findings_for(
        gates: &mut MaterializationAuthorityGates<'_>,
        relative: &str,
        source: &str,
    ) -> Vec<Finding> {
        let parsed = syn::parse_file(source).expect("fixture parses");
        gates.file_findings(relative, source, &parsed)
    }

    fn errors(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|finding| finding.severity == "error")
            .collect()
    }

    fn warnings(findings: &[Finding]) -> Vec<&Finding> {
        findings
            .iter()
            .filter(|finding| finding.severity == "warn")
            .collect()
    }

    #[test]
    fn peer_not_found_construction_outside_the_allowlist_is_an_error() {
        let source = r"
            fn registered_miss(dest: PeerId) -> SendError {
                SendError::PeerNotFound(dest)
            }
            fn new_miss(dest: PeerId) -> Result<(), InprocSendError> {
                lookup().ok_or_else(|| InprocSendError::PeerNotFound(dest.to_string()))?;
                Ok(())
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, "crates/meerkat-comms/src/router.rs", source);
        let errors = errors(&findings);
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert_eq!(errors[0].key.rule, PEER_NOT_FOUND_RULE);
        assert_eq!(
            errors[0].key.symbol,
            "new_miss::InprocSendError::PeerNotFound"
        );
    }

    #[test]
    fn peer_not_found_patterns_and_test_items_are_not_constructions() {
        let source = r#"
            fn classify(err: SendError) -> bool {
                match err {
                    SendError::PeerNotFound(_) => true,
                    _ => false,
                }
            }
            fn passes(result: Result<(), SendError>) -> bool {
                if let Err(SendError::PeerNotFound(peer)) = result {
                    return peer.is_empty();
                }
                matches!(result, Err(SendError::PeerNotFound(_)))
            }
            #[cfg(test)]
            mod tests {
                fn fixture() -> SendError {
                    SendError::PeerNotFound("peer".to_string())
                }
            }
        "#;
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, "crates/meerkat-mob/src/runtime/x.rs", source);
        assert!(errors(&findings).is_empty(), "{findings:?}");
    }

    #[test]
    fn a_constructor_passed_as_a_value_is_a_construction() {
        let source = r"
            fn mapped(dest: PeerId) -> Result<(), SendError> {
                Err(dest).map_err(SendError::PeerNotFound)
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, "crates/meerkat-comms/src/router.rs", source);
        assert_eq!(errors(&findings).len(), 1, "{findings:?}");
    }

    #[test]
    fn live_session_reads_are_errors_outside_registered_sites() {
        let source = r"
            impl Actor {
                async fn drift_probe(&self) -> bool {
                    self.service.live_session_actor_registered(&self.session).await
                }
                async fn shadow_decision(&self) -> bool {
                    self.service.has_live_session(&self.session).await.unwrap_or(false)
                }
                async fn path_call(&self) -> bool {
                    SessionService::has_live_session(self.service.as_ref(), &self.session)
                        .await
                        .unwrap_or(false)
                }
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(
            &mut gates,
            "crates/meerkat-mob/src/runtime/actor.rs",
            source,
        );
        let symbols: BTreeSet<&str> = errors(&findings)
            .iter()
            .map(|finding| finding.key.symbol.as_str())
            .collect();
        assert_eq!(
            symbols,
            BTreeSet::from([
                "shadow_decision::has_live_session",
                "path_call::has_live_session"
            ]),
            "{findings:?}"
        );
    }

    #[test]
    fn a_pending_site_warns_and_a_stale_allowance_warns() {
        let source = r"
            impl Handle {
                async fn ensure_live(&self) -> bool {
                    self.service.live_session_actor_registered(&self.session).await
                }
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(
            &mut gates,
            "crates/meerkat-mob/src/runtime/handle.rs",
            source,
        );
        assert!(errors(&findings).is_empty(), "{findings:?}");
        assert_eq!(warnings(&findings).len(), 1, "{findings:?}");
        let stale: Vec<String> = gates
            .stale_findings()
            .into_iter()
            .map(|finding| finding.key.symbol)
            .collect();
        assert!(stale.contains(&"never_present".to_string()), "{stale:?}");
        assert!(stale.contains(&"drift_probe".to_string()), "{stale:?}");
        assert!(!stale.contains(&"ensure_live".to_string()), "{stale:?}");
    }

    #[test]
    fn live_session_rule_is_scoped_to_meerkat_mob() {
        let source = r"
            async fn elsewhere(service: &Service, id: &SessionId) -> bool {
                service.has_live_session(id).await.unwrap_or(false)
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(
            &mut gates,
            "crates/meerkat-session/src/persistent.rs",
            source,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn member_session_discards_go_through_the_release_seam() {
        let source = r"
            impl Actor {
                async fn discard_member_session_with_release(&mut self, id: &SessionId) {
                    let _ = self.service.discard_live_session(id).await;
                }
                async fn rollback(&self, id: &SessionId) {
                    let _ = self.service.discard_live_session_under_runtime_turn_boundary(id).await;
                }
                async fn reload(&self, id: &SessionId) {
                    let _ = self.service.discard_live_session(id).await;
                }
            }
            impl MobSessionService for Adapter {
                async fn discard_live_session_after_runtime_stop_terminalized(&self, id: &SessionId) {
                    let _ = self.discard_live_session(id).await;
                }
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(
            &mut gates,
            "crates/meerkat-mob/src/runtime/provisioner.rs",
            source,
        );
        let errors = errors(&findings);
        // A fn named like the discard API outside the adapter file is no
        // adapter: only the release seam and the adapter file are exempt.
        let symbols: BTreeSet<String> = errors
            .iter()
            .map(|finding| finding.key.symbol.clone())
            .collect();
        assert_eq!(
            symbols,
            BTreeSet::from([
                "reload::discard_live_session".to_string(),
                "discard_live_session_after_runtime_stop_terminalized::discard_live_session"
                    .to_string(),
            ]),
            "{findings:?}"
        );
        assert!(
            errors
                .iter()
                .all(|finding| finding.key.rule == DISCARD_RULE)
        );
        assert_eq!(warnings(&findings).len(), 1, "{findings:?}");
    }

    #[test]
    fn the_session_service_adapter_file_defines_the_discard_api() {
        let source = r"
            impl MobSessionService for Adapter {
                async fn discard_live_session_after_runtime_stop_terminalized(&self, id: &SessionId) {
                    let _ = self.discard_live_session(id).await;
                }
                async fn create_session_with_actor_witness_under_runtime_turn_boundary(&self, id: &SessionId) {
                    let _ = self.discard_live_session_actor(id).await;
                }
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, SESSION_SERVICE_ADAPTER_PATH, source);
        assert!(errors(&findings).is_empty(), "{findings:?}");
    }

    #[test]
    fn test_source_files_are_out_of_scope() {
        let source = r#"
            fn helper() -> SendError {
                SendError::PeerNotFound("peer".to_string())
            }
        "#;
        let mut gates = fixture_gates();
        let findings = findings_for(
            &mut gates,
            "crates/meerkat-mob/src/runtime/tests.rs",
            source,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_call_of_a_constructor_helper_is_a_construction_site() {
        let source = r"
            fn peer_not_found() -> InprocSendError {
                InprocSendError::PeerNotFound(String::new())
            }
            fn resolve() -> Result<(), InprocSendError> {
                Err(peer_not_found())
            }
            fn unregistered() -> Result<(), InprocSendError> {
                Err(peer_not_found())
            }
        ";
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, "crates/meerkat-comms/src/inproc.rs", source);
        let errors = errors(&findings);
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert_eq!(errors[0].key.symbol, "unregistered::peer_not_found()");
    }

    #[test]
    fn comms_source_writing_the_not_found_text_is_a_construction_site() {
        let source = r#"
            fn trust_miss(peer: &str) -> String {
                format!("peer_not_found_or_not_trusted: peer '{peer}' is not found or not trusted")
            }
            fn new_miss(peer: &str) -> String {
                format!("peer_not_found_or_not_trusted: peer '{peer}' is not found or not trusted")
            }
            fn classify(message: &str) -> bool {
                message.starts_with("peer_not_found_or_not_trusted")
            }
        "#;
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, "crates/meerkat-comms/src/mcp/tools.rs", source);
        let errors = errors(&findings);
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert_eq!(
            errors[0].key.symbol,
            format!("new_miss::{PEER_NOT_FOUND_TEXT_PREFIX}")
        );
        // Outside comms source the text is a surface mapping, not a decision.
        let mut gates = fixture_gates();
        let findings = findings_for(&mut gates, "crates/meerkat-rest/src/lib.rs", source);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn canonical_allowlists_are_unique_and_explained() {
        let lists = GateAllowlists::canonical();
        for list in [lists.peer_not_found, lists.live_session, lists.discard] {
            let mut keys = BTreeSet::new();
            for allowance in list {
                assert!(
                    keys.insert((allowance.path_suffix, allowance.enclosing_fn)),
                    "duplicate allowance {} {}",
                    allowance.path_suffix,
                    allowance.enclosing_fn
                );
                assert!(!allowance.reason.trim().is_empty());
                assert!(allowance.path_suffix.starts_with("crates/"));
            }
        }
        // The release seam is never listed: it is always allowed.
        assert!(
            lists
                .discard
                .iter()
                .all(|allowance| allowance.enclosing_fn != RELEASE_SEAM_FN)
        );
    }
}
