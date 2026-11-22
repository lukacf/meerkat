//! Questions a member asks a person (`ask_user`).
//!
//! Core owns the tool and its vocabulary (`meerkat_core::ask_user`) and the
//! decision kind that holds a pending question set
//! (`meerkat_runtime::ask_user`, kind `ask_user.questions`): the decision's
//! machine owns the whole lifecycle of a set, and this module keeps no state
//! of its own. Meta is the host:
//!
//! - **Mount.** Every member build passes the base session layer's hook
//!   (`AskUserSlot::after_hook`), which puts the tool, with a
//!   [`MemberAskUserHost`] for that member, into the build's host tools
//!   (`SessionBuildOptions::host_tools`): Core's factory composes them as
//!   one more part of the build's tool surface, beside whatever its base
//!   is, so the build's own tools and its tool catalog are what they are
//!   without the tool. A build whose tools already name
//!   `ask_user` (a host's own tool of that name) keeps its own, and Meta
//!   mounts none: the questions that tool asks are that tool's, so no
//!   decision is held for them, the read lists none of them and no notice
//!   is sent for them. A member build that runs through its registration's
//!   tool-surface gate (a host with a member registration hook decides the
//!   build's prepared surface whole) gets none either: it carries only the
//!   tools it was asked for. Nothing is opened, read or started for the
//!   mount.
//! - **Deny.** A member's tool restriction may deny the stock tool by name
//!   (a profile's `tools.deny`) on every member build, whether or not the
//!   build carries it: the same hook declares the stock names
//!   (`STOCK_MEMBER_TOOLS`) to the build's declared tool restriction as
//!   names of its host tools, the vocabulary Core's deny check reads. On a
//!   build without the stock tool the entry is inert, or hides and refuses
//!   the build's own tool of that name, as a deny does to every tool.
//! - **Surface.** The slot is late-bound: the runtime fills it
//!   (`AskUserSurface`) once it keeps durable decisions and serves its
//!   long-term agents. A call answers `no_human_surface` at once, holding
//!   nothing, while the slot is empty, when the runtime keeps no durable
//!   decisions, on a candidate launch (which acts nowhere), when no person
//!   can decide on the runtime's Console
//!   (`crate::console_schedules::ConsoleServed`: it serves none, or one
//!   that can identify no person or takes no changes), and for
//!   an asker no answer can be routed back to (a member of another mob than
//!   the runtime's own, or a name that is no agent name). Otherwise the set
//!   is held and the call answers `asked`.
//! - **Read.** `open_question_sets` lists the open sets, soonest deadline
//!   first, for `meta/gating/questions`.
//! - **Answer.** `answer_question_set` is the one hand-off of a person's
//!   answer to the decision owner, for `meta/gating/decide` on the Console
//!   and on stdio. Its match on the decision's kind is closed: a decision of
//!   any other kind is `QuestionSetRefusal::NotAQuestionSet`, which each
//!   route answers as it answers an unknown pending id.
//! - **Notice.** The kind's executor is driven for the runtime from its first
//!   set on (an executor claim creates files, so none is made before)
//!   (`spawn_ask_user_driver`); the end of a set reaches its asker as a
//!   keyed send (`MemberKeyedNotices`, key [`ask_user_notice_key`]) whose
//!   body is the typed result ([`ask_user_result_input`]). The notice is
//!   its asking member's: it is sent only while that member holds its name
//!   still.
//! - **End of the asker.** When a member private to a person ends, the
//!   open sets it asked are withdrawn through the decision's own lifecycle
//!   (`EndedPrivateMemberQuestionSets`); a set such a member left open
//!   while nothing acted on its end is withdrawn when the runtime serves
//!   its members' questions again.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::consent::{AuthenticatedApprover, ConsentEligibility};
use meerkat_core::approval::decision::{
    AttestedDecision, DECISION_PAGE_MAX, DecisionId, DecisionIngress, DecisionListFilter,
    DecisionPhase, DecisionPrincipalKey, DecisionReference, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::ask_user::{
    ASK_USER_RESULT_INPUT, ASK_USER_TOOL, AskUserAnswers, AskUserDispatcher, AskUserHost,
    AskUserQuestionAnswer, AskUserQuestionSet, AskUserRefusal, AskUserRequest, AskUserResult,
};
use meerkat_core::service::CreateSessionRequest;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_runtime::ask_user::{
    ASK_USER_QUESTIONS_KIND, AskUserApproverCurrent, AskUserBody, AskUserError, AskUserExecutor,
    AskUserHold, AskUserNotice, AskUserNotices, ask_user_questions_kind, ask_user_result_of,
    attest_ask_user_answers, hold_ask_user,
};
use meerkat_runtime::decision::{DecisionClock, DecisionError, DecisionExecutor, DecisionRefusal};
use serde::Serialize;
use serde_json::{Value, json};

use crate::console_schedules::ConsoleServed;
use crate::durable_decisions::{DecisionExecutorDriver, RuntimeDecisions};
use crate::mob_handle_runtime::{MobRuntime, PreBuildHook};

/// The `kind` of a decided question set in a `meta/gating/decide` result.
pub const QUESTION_SET_RESULT_KIND: &str = "question_set";

/// The stock tools Meta puts on member builds as host tools, by name. Every
/// member build is told these names (`declare_stock_tools`), whether or
/// not `AskUserSlot::mount` puts the tool on it, so a deny list may name a
/// stock tool in a profile used for any member.
pub(crate) const STOCK_MEMBER_TOOLS: [&str; 1] = [ASK_USER_TOOL];

/// Declare `STOCK_MEMBER_TOOLS` to a member build's declared tool
/// restriction as names of its host tools
/// ([`meerkat_core::ToolVocabularySource::HostTools`]): names its deny list
/// may name on this build, mounted here or not. A build without a declared
/// restriction denies nothing, so it has nothing to tell.
fn declare_stock_tools(restriction: Option<&mut meerkat_core::ops::DeclaredToolRestriction>) {
    let Some(restriction) = restriction else {
        return;
    };
    let names = restriction
        .vocabulary
        .entry(meerkat_core::ToolVocabularySource::HostTools)
        .or_default();
    for name in STOCK_MEMBER_TOOLS {
        names.insert(name);
    }
}

/// The ingress and channel of an answer given on the Console.
pub(crate) const CONSOLE_CHANNEL: &str = "console";

/// The ingress and channel of an answer the stdio host carries.
pub(crate) const HOST_CHANNEL: &str = "host";

/// The key the notice of the decision `decision_id` is sent under: a notice
/// asked for twice is delivered once.
pub fn ask_user_notice_key(decision_id: &str) -> String {
    format!("ask-user:{decision_id}")
}

/// The input that carries `result` to the asking member: the label
/// [`ASK_USER_RESULT_INPUT`] on a line of its own, then the typed result as
/// one JSON document. The one encoding of a result as an input.
///
/// # Errors
/// The encoder's, which a typed result does not raise.
pub fn ask_user_result_input(result: &AskUserResult) -> Result<String, serde_json::Error> {
    Ok(format!(
        "{ASK_USER_RESULT_INPUT}\n{}",
        serde_json::to_string(result)?
    ))
}

/// What a runtime serves of the questions its members ask: its durable
/// decisions, whether it serves a Console, and the mob whose members it can
/// route an answer back to.
pub(crate) struct AskUserSurface {
    decisions: RuntimeDecisions,
    console: ConsoleServed,
    mob_id: String,
    clock: Arc<dyn DecisionClock>,
    /// Signalled by each set the runtime holds: what starts the executor's
    /// driver at the first one ([`AskUserDriver`]).
    held: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for AskUserSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskUserSurface")
            .field("decisions", &self.decisions)
            .field("a_person_decides", &self.console.takes_a_persons_decision())
            .field("mob_id", &self.mob_id)
            .finish_non_exhaustive()
    }
}

impl AskUserSurface {
    /// The surface of the runtime of the mob `mob_id` over `decisions`,
    /// which serves a Console while `console` says so.
    pub(crate) fn new(
        decisions: RuntimeDecisions,
        console: ConsoleServed,
        mob_id: impl Into<String>,
    ) -> Self {
        Self {
            decisions,
            console,
            mob_id: mob_id.into(),
            // The clock the runtime's decisions are stamped by.
            clock: Arc::new(crate::durable_decisions::EntryTimeClock),
            held: Arc::default(),
        }
    }

    /// The signal each set the runtime holds raises.
    pub(crate) fn held_signal(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.held)
    }

    /// This surface reading `clock` when it holds a set.
    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Arc<dyn DecisionClock>) -> Self {
        self.clock = clock;
        self
    }

    /// Whether a person can be shown a question now, and answer it: the
    /// runtime keeps durable decisions, it speaks for its composition, and
    /// it serves a Console on which a person can decide (one that can
    /// identify a person and takes changes).
    fn shows_questions(&self) -> bool {
        if !self.decisions.is_available() {
            return false;
        }
        if self.decisions.is_candidate() {
            return false;
        }
        self.console.takes_a_persons_decision()
    }

    /// Whether the answer to a question of `asker`, a member of the mob
    /// `mob_id`, can be routed back to it: it is a member of the runtime's
    /// own mob, under an agent name.
    fn routes_answers_to(&self, mob_id: &str, asker: &str) -> bool {
        if mob_id != self.mob_id {
            return false;
        }
        crate::identity_first::AgentIdentity::parse(asker).is_ok()
    }
}

/// The late-bound surface every member's `ask_user` reads at each call;
/// empty until the runtime fills it, and again once it shut down.
#[derive(Clone, Default)]
pub struct AskUserSlot {
    surface: SurfaceCell,
    /// Who the runtime's members are private to, once the runtime follows
    /// the ends of its private members with it
    /// ([`Self::follow_private_member_ends`]).
    owners: Arc<std::sync::OnceLock<crate::private_owners::PrivateOwners>>,
}

/// Where a slot keeps its surface.
type SurfaceCell = Arc<RwLock<Option<Arc<AskUserSurface>>>>;

impl std::fmt::Debug for AskUserSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskUserSlot")
            .field("filled", &self.surface().is_some())
            .finish()
    }
}

impl AskUserSlot {
    /// Fill the slot; it replaces the surface before. A runtime that
    /// follows the ends of its private members
    /// ([`Self::follow_private_member_ends`]) then withdraws, in the
    /// background, the open sets of the private members that ended while
    /// nothing acted on their end
    /// ([`EndedPrivateMemberQuestionSets::withdraw_sets_of_ended_members`]).
    pub(crate) fn fill(&self, surface: AskUserSurface) {
        *self
            .surface
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(surface));
        if let (Some(owners), Ok(runtime)) =
            (self.owners.get(), tokio::runtime::Handle::try_current())
        {
            let sets = EndedPrivateMemberQuestionSets::of(self);
            let owners = owners.clone();
            runtime.spawn(async move { sets.withdraw_sets_of_ended_members(&owners).await });
        }
    }

    /// Follow the ends of the members private to a person `owners` records:
    /// from here on the open question sets such a member asked are
    /// withdrawn when it ends ([`EndedPrivateMemberQuestionSets`]), whatever
    /// else the launch composes. The follower reads this slot at each end,
    /// so a runtime that serves no questions follows the ends and withdraws
    /// nothing.
    pub(crate) async fn follow_private_member_ends(
        &self,
        owners: &crate::private_owners::PrivateOwners,
    ) {
        let _ = self.owners.set(owners.clone());
        owners
            .follow_member_ends_as(
                crate::private_owners::MemberEndSeat::QuestionSets,
                Arc::new(EndedPrivateMemberQuestionSets::of(self)),
            )
            .await;
    }

    /// Empty the slot: every later call answers `no_human_surface`.
    pub(crate) fn clear(&self) {
        *self
            .surface
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn surface(&self) -> Option<Arc<AskUserSurface>> {
        self.surface
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The signal each set held through the slot's surface raises, when the
    /// slot is filled.
    #[cfg(test)]
    pub(crate) fn held_signal(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.surface().map(|surface| surface.held_signal())
    }

    /// The durable decisions the question sets are held in, when the slot is
    /// filled.
    pub(crate) fn decisions(&self) -> Option<RuntimeDecisions> {
        self.surface().map(|surface| surface.decisions.clone())
    }

    /// `hook`, then [`Self::mount`]: the pre-build hook of the base session
    /// layer, which every member create and rebuild passes last, after the
    /// hooks of every layer around it.
    pub(crate) fn after_hook(&self, hook: PreBuildHook) -> PreBuildHook {
        let slot = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let slot = slot.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                slot.mount(req);
                Ok(())
            })
        })
    }

    /// Put the `ask_user` tool, with a host for that member, into the host
    /// tools of a member build (`SessionBuildOptions::host_tools`), beside a
    /// dispatcher a host layer already put there. Core's factory composes
    /// the build's host tools as one more part of its tool surface, beside
    /// whatever base it builds for the build, so the mount adds the one tool
    /// and changes nothing else: the build's external tools are not touched
    /// (the factory binds and judges them as it does without the tool), and
    /// the tool's own catalog is exact and inline, so the build's catalog
    /// reads exact or not, with or without the catalog's control tools, as
    /// it does without the tool.
    ///
    /// Nothing changes for a request that is not a member build, and for a
    /// build whose external tools or host tools already name `ask_user`, as
    /// a tool they list or an entry of their catalog (the host's own tool of
    /// that name stays reachable: a name two parts of a build's surface list
    /// is offered by neither). Nothing changes either for a member build
    /// that runs through its registration's tool-surface gate
    /// (`built_through_surface_gate` of `crate::native_realm`): the host that
    /// installed the gate decides the build's prepared surface whole, it can
    /// accept or refuse it but take no tool out of it, so the build carries
    /// only the tools it was asked for and Meta adds none of its own.
    ///
    /// Every member build, with the tool or without it, is first told the
    /// stock names (`declare_stock_tools`): a deny of `ask_user` is a deny
    /// of a known name on each of them.
    pub(crate) fn mount(&self, req: &mut CreateSessionRequest) {
        let Some(build) = req.build.as_mut() else {
            return;
        };
        let Some(binding) = build.mob_member_binding.as_ref() else {
            return;
        };
        declare_stock_tools(build.declared_tool_restriction.as_mut());
        if crate::native_realm::built_through_surface_gate(build) {
            tracing::debug!(
                member = %binding.member,
                "the member's build runs through its registration's tool-surface gate: \
                 Meta mounts no ask_user"
            );
            return;
        }
        if [build.external_tools.as_ref(), build.host_tools.as_ref()]
            .into_iter()
            .flatten()
            .any(|tools| names_ask_user(tools.as_ref()))
        {
            tracing::debug!(
                member = %binding.member,
                "the member's build already has a tool named ask_user: Meta mounts none"
            );
            return;
        }
        let host = MemberAskUserHost::new(
            self.clone(),
            binding.mob_id.clone(),
            crate::member_comms_id::logical_memory_identity(&binding.member),
        );
        let tool: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(AskUserDispatcher::new(Arc::new(host)));
        build.host_tools = Some(match build.host_tools.take() {
            Some(already) => Arc::new(meerkat_core::DynamicToolComposite::new(vec![already, tool])),
            None => tool,
        });
    }
}

/// Whether `tools` names `ask_user`: as a tool it lists now, or as an entry
/// of its catalog (one that is deferred or not callable now is still its
/// name).
fn names_ask_user(tools: &dyn meerkat_core::AgentToolDispatcher) -> bool {
    tools
        .tools()
        .iter()
        .any(|tool| tool.name.as_ref() == ASK_USER_TOOL)
        || tools
            .tool_catalog()
            .iter()
            .any(|entry| entry.tool.name.as_ref() == ASK_USER_TOOL)
}

/// The principal a question set names as its requester: the asking agent,
/// in its mob's own domain, as a change of an agent names an agent
/// requester.
fn asker_requester(mob_id: &str, asker: &str) -> Option<DecisionPrincipalKey> {
    let domain = TrustDomainId::new(format!(
        "{}{mob_id}",
        crate::identity_creation::REQUESTER_TRUST_DOMAIN_PREFIX
    ))
    .ok()?;
    let principal = PrincipalRef::in_domain(PrincipalKind::SharedAgent, asker, domain).ok()?;
    DecisionPrincipalKey::of(&principal).ok()
}

/// Where one member's `ask_user` calls go: see the module documentation.
pub struct MemberAskUserHost {
    slot: AskUserSlot,
    /// The mob the member is a member of.
    mob_id: String,
    /// The member's identity: what a gating origin's `identity` carries, and
    /// the name its notice is sent to.
    asker: String,
}

impl std::fmt::Debug for MemberAskUserHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MemberAskUserHost")
            .field("mob_id", &self.mob_id)
            .field("asker", &self.asker)
            .finish_non_exhaustive()
    }
}

impl MemberAskUserHost {
    /// The host of the member `asker` of the mob `mob_id`, reading `slot`
    /// at each call.
    pub(crate) fn new(slot: AskUserSlot, mob_id: String, asker: String) -> Self {
        Self {
            slot,
            mob_id,
            asker,
        }
    }
}

/// The refusal of a hold the decision owner or the kind's code refused.
fn hold_refusal(error: AskUserError) -> AskUserRefusal {
    match error {
        AskUserError::Invalid(reason) => AskUserRefusal::Invalid(reason),
        AskUserError::Decision(DecisionError::Refused {
            refusal: DecisionRefusal::HoldConflict,
            ..
        }) => AskUserRefusal::Invalid(
            "this call asked another set of questions before: ask a new set in a new call"
                .to_owned(),
        ),
        other => AskUserRefusal::Failed(other.to_string()),
    }
}

#[async_trait]
impl AskUserHost for MemberAskUserHost {
    async fn ask(&self, request: AskUserRequest<'_>) -> Result<AskUserResult, AskUserRefusal> {
        let Some(surface) = self.slot.surface() else {
            return Ok(AskUserResult::no_human_surface());
        };
        if !surface.shows_questions() {
            return Ok(AskUserResult::no_human_surface());
        }
        if !surface.routes_answers_to(&self.mob_id, &self.asker) {
            return Ok(AskUserResult::no_human_surface());
        }
        let Some(requester) = asker_requester(&self.mob_id, &self.asker) else {
            return Err(AskUserRefusal::Failed(
                "the asking agent cannot be named as a requester".to_owned(),
            ));
        };
        let now_ms = surface
            .clock
            .now_ms()
            .map_err(|error| AskUserRefusal::Failed(error.to_string()))?;
        let held = hold_ask_user(
            surface.decisions.owner(),
            now_ms,
            AskUserHold {
                asker: self.asker.clone(),
                requester,
                questions: request.questions.clone(),
                timeout: request.timeout,
                run_id: request.run_id.clone(),
                call_id: request.call_id.to_owned(),
            },
        )
        .map_err(hold_refusal)?;
        surface.held.notify_one();
        surface.decisions.notify_work();
        ask_user_result_of(&held).map_err(|error| AskUserRefusal::Failed(error.to_string()))
    }
}

/// Notices of ended question sets, as a keyed send to the asking member's
/// own name through the mob's identity runtime: the key is the decision, so
/// a notice asked for twice is delivered once, and the send starts a turn
/// of an idle agent. A member that is no longer an agent of the mob is a
/// terminal failed receipt; one that cannot take the input now is asked
/// again at a later pass.
///
/// A notice is its asking member's, never a later holder's of its name: it
/// is sent only while the member that held the name when the set was asked
/// holds it still ([`crate::private_owners::PrivateOwners::still_held_by`]:
/// no member under the name ended since, and the name's newest seating is
/// private to the same person, or like it to no one). For a set whose asker
/// ended or was replaced the receipt is a terminal failure, and nothing is
/// sent. While the record of who held the name is still being read, the
/// notice is asked again at a later pass.
pub(crate) struct MemberKeyedNotices {
    mob: MobRuntime,
}

impl MemberKeyedNotices {
    /// Notices sent through the identity runtime `mob` has installed when a
    /// notice is delivered. `mob` is the composition's own handle of its
    /// mob, never its public owner.
    pub(crate) fn new(mob: MobRuntime) -> Self {
        Self { mob }
    }

    /// Whether the member that asked the set of `notice` holds its name
    /// still: `None` while that cannot be told (the record of who held the
    /// name is still being read, or the set cannot be read now).
    fn asker_holds_its_name(&self, notice: &AskUserNotice) -> Option<bool> {
        let owners = self.mob.private_owners();
        if owners.first_read_pending() {
            return None;
        }
        let decisions = self.mob.ask_user_slot().decisions()?;
        // A candidate's read records nothing: the notice's own pass settles
        // what the decision owes.
        let asked_at_ms = decisions
            .owner()
            .for_candidate_launch()
            .get(&notice.decision_id)
            .ok()?
            .descriptor
            .created_at_ms;
        // A name the record knows no seating of is a member of a mob whose
        // journal it does not see: the send decides, as before.
        Some(
            owners
                .still_held_by(
                    self.mob.handle().mob_id().as_str(),
                    &notice.asker,
                    asked_at_ms,
                )
                .unwrap_or(true),
        )
    }
}

#[async_trait]
impl AskUserNotices for MemberKeyedNotices {
    async fn deliver(&self, notice: &AskUserNotice) -> Option<ApprovalConsentDeliveryState> {
        let failed = |reason: &str| {
            Some(ApprovalConsentDeliveryState::Failed {
                reason: reason.to_owned(),
            })
        };
        let Ok(identity) = crate::identity_first::AgentIdentity::parse(&notice.asker) else {
            return failed("the asker is not an agent name");
        };
        if !self.asker_holds_its_name(notice)? {
            return failed("the asker ended, or another member holds its name");
        }
        let Ok(text) = ask_user_result_input(&notice.result) else {
            return failed("the result could not be encoded");
        };
        // No identity runtime now: asked again at a later pass.
        let runtime = self.mob.installed_identity_runtime()?;
        let content = meerkat_core::ContentInput::Text(text);
        let key = ask_user_notice_key(notice.decision_id.as_str());
        match runtime
            .send_keyed_tracked(
                &self.mob,
                &identity,
                None,
                &content,
                meerkat_core::types::HandlingMode::Queue,
                &key,
            )
            .await
        {
            Ok(receipt) => match receipt.status {
                meerkat_contracts::WireContinuationStatus::Admitted {
                    session_id,
                    input_id,
                } => match meerkat_core::SessionId::parse(&session_id) {
                    Ok(session_id) => Some(ApprovalConsentDeliveryState::Admitted {
                        session_id,
                        input_id,
                    }),
                    Err(_) => failed("the notice was admitted to a session with no readable id"),
                },
                meerkat_contracts::WireContinuationStatus::Pending => None,
                meerkat_contracts::WireContinuationStatus::Stranded => {
                    failed("the asker was replaced before the notice was admitted")
                }
                meerkat_contracts::WireContinuationStatus::Refused { reason } => failed(&reason),
            },
            Err(crate::identity_first::IdentityRuntimeError::UnknownIdentity(_)) => {
                failed("the asker is no longer an agent of the mob")
            }
            Err(error) => {
                tracing::debug!(%error, "the notice of a question set could not be sent now");
                None
            }
        }
    }
}

/// The question sets of a runtime's members, as they follow the end of a
/// member private to a person
/// ([`crate::private_owners::PrivateMemberEndFollower`]): every open set the
/// ended member asked is withdrawn, by the decision's own lifecycle (the
/// host withdraws it), and its asker is owed the notice every withdrawn set
/// owes. Nobody is left to route an answer to. It keeps no state: it reads
/// the open sets from the decision store at each end.
///
/// An open set asked under the ended member's name is withdrawn when either
/// holds:
///
/// - it was asked at or before the end's time. Its time can be earlier than
///   the member's seating only because the clock that stamps both was set
///   back in between, so the seatings are not asked whose it was then; a
///   set an earlier holder of the name left open is withdrawn with it (that
///   holder ended before the name was taken, and nothing can be routed to
///   it);
/// - the seatings the mob's journal recorded under the name say it is the
///   member's ([`PrivateOwners::ends_with_a_private_member`]): it was asked
///   while the member last seated under the name held it, where that
///   member was seated no later than the end's time (a member seated after
///   the end is a later one, and its sets stay), whatever time the journal
///   gave the end against the set's; or while a private member held the
///   name whose end the journal holds.
///
/// A set asked under the name after the end's time, while another member
/// holds the name or none that the record knows of, stays as it is; so do a
/// set of any other member, a set that ended some other way, and every set
/// of a launch that does not speak for its composition.
///
/// [`PrivateOwners::ends_with_a_private_member`]: crate::private_owners::PrivateOwners::ends_with_a_private_member
pub(crate) struct EndedPrivateMemberQuestionSets {
    /// The slot's surface, held weakly: the record of who is private to
    /// whom keeps this follower, and the slot keeps that record.
    surface: std::sync::Weak<RwLock<Option<Arc<AskUserSurface>>>>,
    /// The record of who is private to whom the slot follows, held weakly
    /// for the same reason.
    owners: std::sync::Weak<std::sync::OnceLock<crate::private_owners::PrivateOwners>>,
}

impl EndedPrivateMemberQuestionSets {
    /// How often the withdrawals of one pass are tried.
    const ATTEMPTS: u32 = 3;

    /// The follower over the surface `slot` holds when a member ends.
    fn of(slot: &AskUserSlot) -> Self {
        Self {
            surface: Arc::downgrade(&slot.surface),
            owners: Arc::downgrade(&slot.owners),
        }
    }

    /// The surface the slot holds now, when it holds one whose decisions
    /// this launch may change.
    fn surface(&self) -> Option<Arc<AskUserSurface>> {
        let surface = self
            .surface
            .upgrade()?
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        (surface.decisions.is_available() && !surface.decisions.is_candidate()).then_some(surface)
    }

    /// Withdraw every open set of the runtime's own mob that `theirs` names,
    /// each through the decision's own lifecycle, and tell the executors
    /// that decisions changed. A set that ended meanwhile is left as it
    /// ended. A withdrawal that fails does not end the pass: the sets after
    /// it are visited all the same, and the pass answers its first failure.
    fn withdraw_open_sets(
        surface: &AskUserSurface,
        why: &'static str,
        theirs: impl Fn(&HeldQuestionSet) -> bool,
    ) -> Result<(), DecisionError> {
        let decisions = &surface.decisions;
        let mut withdrawn = false;
        let mut first_failure = None;
        for set in open_question_sets(decisions)? {
            if !theirs(&set) {
                continue;
            }
            match decisions.owner().withdraw(
                set.decision_id(),
                &meerkat_runtime::decision::DecisionWithdrawer::Host,
            ) {
                Ok(_) => {
                    withdrawn = true;
                    tracing::info!(
                        mob_id = %surface.mob_id,
                        decision_id = %set.decision_id(),
                        asker = %set.asker(),
                        why,
                        "a question set was withdrawn: the member that asked it, private to a \
                         person, ended"
                    );
                }
                Err(DecisionError::Refused {
                    refusal: DecisionRefusal::NotOpen | DecisionRefusal::Expired,
                    ..
                }) => {}
                Err(error) => {
                    tracing::debug!(
                        %error,
                        mob_id = %surface.mob_id,
                        decision_id = %set.decision_id(),
                        "a question set of an ended member private to a person could not be \
                         withdrawn"
                    );
                    if first_failure.is_none() {
                        first_failure = Some(error);
                    }
                }
            }
        }
        if withdrawn {
            decisions.notify_work();
        }
        first_failure.map_or(Ok(()), Err)
    }

    /// `pass`, tried again after a wait while it fails, [`Self::ATTEMPTS`]
    /// times in all; the sets a last failure leaves stay as they are, until
    /// their deadline.
    async fn withdrawing(
        &self,
        mut pass: impl FnMut(&AskUserSurface) -> Result<(), DecisionError> + Send,
    ) {
        let mut wait = Duration::from_millis(250);
        for attempt in 1..=Self::ATTEMPTS {
            let Some(surface) = self.surface() else {
                return;
            };
            let Err(error) = pass(&surface) else {
                return;
            };
            if attempt == Self::ATTEMPTS {
                tracing::warn!(
                    %error,
                    mob_id = %surface.mob_id,
                    "the question sets of an ended member private to a person could not all be \
                     withdrawn: the ones left stay open until their deadline"
                );
                return;
            }
            tokio::time::sleep(wait).await;
            wait *= 4;
        }
    }

    /// Withdraw the open sets of the private members that ended while
    /// nothing acted on their end: the runtime served no questions then, or
    /// an earlier process, or an earlier release, left the set open. Once
    /// the journals are read ([`PrivateOwners::journals_read`]), an open set
    /// is withdrawn when the member that held its asker's name when it was
    /// asked was an ephemeral member private to a person and has ended. The
    /// set of a long-term agent that is not seated now is never one: such
    /// an agent is woken by its answer. The decision store is read only
    /// when the journals hold such an end.
    ///
    /// [`PrivateOwners::journals_read`]: crate::private_owners::PrivateOwners::journals_read
    pub(crate) async fn withdraw_sets_of_ended_members(
        &self,
        owners: &crate::private_owners::PrivateOwners,
    ) {
        owners.journals_read().await;
        if !owners.knows_an_ended_private_member() {
            return;
        }
        self.withdrawing(|surface| {
            Self::withdraw_open_sets(surface, "its end was found in the journal", |set| {
                owners
                    .private_holder_ended(&surface.mob_id, set.asker(), set.created_at_ms())
                    .is_some()
            })
        })
        .await;
    }
}

#[async_trait]
impl crate::private_owners::PrivateMemberEndFollower for EndedPrivateMemberQuestionSets {
    async fn member_ended(&self, ended: &crate::private_owners::EndedPrivateMember) {
        let asker = crate::member_comms_id::logical_memory_identity(&ended.member);
        let ended_at_ms = u64::try_from(ended.ended_at.timestamp_millis()).unwrap_or(0);
        self.withdrawing(|surface| {
            // The sets are of members of the runtime's own mob.
            if surface.mob_id != ended.mob_id {
                return Ok(());
            }
            let owners = self.owners.upgrade();
            let owners = owners.as_ref().and_then(|owners| owners.get());
            Self::withdraw_open_sets(surface, "its end was followed", |set| {
                set.asker() == asker
                    && (set.created_at_ms() <= ended_at_ms
                        || owners
                            .and_then(|owners| {
                                owners.ends_with_a_private_member(
                                    &surface.mob_id,
                                    &asker,
                                    set.created_at_ms(),
                                    ended_at_ms,
                                )
                            })
                            .unwrap_or(false))
            })
        })
        .await;
    }
}

/// The driver of the executor of a runtime's `ask_user.questions`
/// decisions, started when it has work: at once when the decision store
/// already holds a set (an earlier process's, which its first pass
/// settles), otherwise at the first set the runtime holds. Until then it
/// claims nothing, creates no file and makes no pass. Dropping it stops the
/// passes.
pub(crate) struct AskUserDriver {
    starter: tokio::task::JoinHandle<()>,
    started: Arc<std::sync::Mutex<Option<DecisionExecutorDriver>>>,
}

impl std::fmt::Debug for AskUserDriver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AskUserDriver")
            .field("passes", &self.passes())
            .finish()
    }
}

impl AskUserDriver {
    /// How many passes the executor's driver has finished; `None` while it
    /// has not started (no set was ever held).
    pub(crate) fn passes(&self) -> Option<u64> {
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(DecisionExecutorDriver::passes)
    }

    /// Stop the passes and wait until the driver's task has ended.
    #[cfg(test)]
    pub(crate) async fn shutdown(self) {
        self.starter.abort();
        let driver = self
            .started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(driver) = driver {
            driver.shutdown().await;
        }
    }
}

impl Drop for AskUserDriver {
    fn drop(&mut self) {
        self.starter.abort();
        // The started driver stops its passes when it is dropped.
        self.started
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// Whether `decisions` hold a decision of the kind, open or ended. A count
/// the store cannot give counts as one that does.
fn holds_a_set(decisions: &RuntimeDecisions) -> bool {
    let Ok(kind) = ask_user_questions_kind() else {
        return true;
    };
    decisions
        .owner()
        .count(&DecisionListFilter::default().with_kind(kind))
        .map_or(true, |count| count > 0)
}

/// Drive the executor of `ask_user.questions` decisions for a runtime, from
/// the moment it has work ([`AskUserDriver`]): `held` is signalled by each
/// set the runtime holds. Once started: a pass at start (which settles
/// what an earlier process left), after each change of a decision, and per
/// `tick` while a set waits for its deadline. `None`, starting nothing,
/// for a runtime that keeps no durable decisions, for a candidate launch,
/// and outside an async runtime.
pub(crate) fn spawn_ask_user_driver(
    decisions: &RuntimeDecisions,
    notices: Arc<dyn AskUserNotices>,
    tick: Duration,
    held: Arc<tokio::sync::Notify>,
) -> Option<AskUserDriver> {
    if !decisions.is_available() {
        return None;
    }
    if decisions.is_candidate() {
        return None;
    }
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::warn!(
            "ended question sets are not delivered: their driver was asked for outside an \
             async runtime"
        );
        return None;
    }
    let executor: Arc<dyn DecisionExecutor> = match AskUserExecutor::new(notices) {
        Ok(executor) => Arc::new(executor),
        Err(error) => {
            tracing::warn!(%error, "ended question sets are not delivered: no executor");
            return None;
        }
    };
    let started = Arc::new(std::sync::Mutex::new(None));
    let starter = tokio::spawn({
        let started = Arc::clone(&started);
        let decisions = decisions.clone();
        async move {
            // The executor's claim creates the store's lock files: it waits
            // for the first set, unless an earlier process left one.
            if !holds_a_set(&decisions) {
                held.notified().await;
            }
            match DecisionExecutorDriver::spawn(
                &decisions,
                vec![executor],
                Arc::new(AskUserApproverCurrent),
                tick,
            ) {
                Ok(driver) => {
                    *started
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(driver);
                }
                Err(error) => {
                    tracing::warn!(
                        error = ?error,
                        "ended question sets are not delivered: their driver did not start"
                    );
                }
            }
        }
    });
    Some(AskUserDriver { starter, started })
}

/// Whether `pending_id` has the shape of a decision id: `decision:` and 64
/// lowercase hex digits. A gating pending id never has it.
pub(crate) fn is_decision_id(pending_id: &str) -> bool {
    pending_id.strip_prefix("decision:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// One held question set: its decision and the body it holds.
#[derive(Debug, Clone)]
pub(crate) struct HeldQuestionSet {
    row: StoredDecision,
    body: AskUserBody,
}

/// A question set as `meta/gating/questions` lists it.
#[derive(Serialize)]
struct WireQuestionSet<'a> {
    pending_id: &'a str,
    origin: WireOrigin<'a>,
    #[serde(flatten)]
    questions: &'a AskUserQuestionSet,
    created_at_ms: u64,
    deadline_at_ms: u64,
    subject_digest: String,
}

#[derive(Serialize)]
struct WireOrigin<'a> {
    identity: &'a str,
}

impl HeldQuestionSet {
    /// The question set `row` holds. The match on the decision's kind is
    /// closed: only `ask_user.questions` is one, and a body of that kind
    /// this version cannot read is none either.
    fn of(row: StoredDecision) -> Result<Self, QuestionSetRefusal> {
        match row.descriptor.kind.as_str() {
            ASK_USER_QUESTIONS_KIND => {}
            _ => return Err(QuestionSetRefusal::NotAQuestionSet),
        }
        let body = AskUserBody::of(&row).map_err(|_| QuestionSetRefusal::NotAQuestionSet)?;
        Ok(Self { row, body })
    }

    /// The asking member's identity.
    pub(crate) fn asker(&self) -> &str {
        &self.body.asker
    }

    /// The decision that holds the set.
    pub(crate) fn decision_id(&self) -> &DecisionId {
        &self.row.decision_id
    }

    /// When the set was asked: the clock reading of its hold, in UNIX
    /// milliseconds.
    pub(crate) fn created_at_ms(&self) -> u64 {
        self.row.descriptor.created_at_ms
    }

    /// Why the set, as this read found it, would not take `verdict` on the
    /// digest `presented`: the refusal the decision itself gives a decide
    /// of a set that is no longer open (`not_open`: a read that finds a set
    /// past its deadline records its expiry) or on another digest than the
    /// set's (`subject_changed`), or answers that are not valid for the
    /// set. A route asks before it records anything for a request; the
    /// hand-off decides again.
    pub(crate) fn refuses(
        &self,
        verdict: &QuestionSetVerdict,
        presented: &Sha256Digest,
    ) -> Option<QuestionSetRefusal> {
        if self.row.state.phase != DecisionPhase::Open {
            return Some(QuestionSetRefusal::Refused(DecisionRefusal::NotOpen));
        }
        if *presented != self.row.descriptor.subject_digest {
            return Some(QuestionSetRefusal::Refused(DecisionRefusal::SubjectChanged));
        }
        match verdict {
            QuestionSetVerdict::Answer(answers) => {
                AskUserAnswers::for_set(&self.body.questions, answers.clone())
                    .err()
                    .map(|error| QuestionSetRefusal::Invalid(error.to_string()))
            }
            QuestionSetVerdict::Dismiss => None,
        }
    }

    /// The set as `meta/gating/questions` lists it.
    pub(crate) fn wire(&self) -> Value {
        serde_json::to_value(WireQuestionSet {
            pending_id: self.row.decision_id.as_str(),
            origin: WireOrigin {
                identity: &self.body.asker,
            },
            questions: &self.body.questions,
            created_at_ms: self.row.descriptor.created_at_ms,
            deadline_at_ms: self.row.descriptor.deadline_at_ms,
            subject_digest: self.row.descriptor.subject_digest.to_string(),
        })
        .unwrap_or(Value::Null)
    }
}

/// The result of `meta/gating/questions` over `sets`.
pub(crate) fn question_sets_result<'a>(
    sets: impl IntoIterator<Item = &'a HeldQuestionSet>,
) -> Value {
    json!({
        "question_sets": sets.into_iter().map(HeldQuestionSet::wire).collect::<Vec<_>>(),
    })
}

/// The open question sets of `decisions`, soonest deadline first. A row
/// this version cannot read is left out, and so is a set whose deadline
/// passed. A runtime that keeps no durable decisions has none.
///
/// # Errors
/// The decision store's read failures.
pub(crate) fn open_question_sets(
    decisions: &RuntimeDecisions,
) -> Result<Vec<HeldQuestionSet>, DecisionError> {
    if !decisions.is_available() {
        return Ok(Vec::new());
    }
    let filter = DecisionListFilter::default()
        .with_kind(ask_user_questions_kind()?)
        .with_phase(DecisionPhase::Open);
    let owner = decisions.owner();
    let mut sets = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let listing = owner.list(&filter, after.as_deref(), DECISION_PAGE_MAX)?;
        for row in listing.decisions {
            // A listing reads each row at its own clock reading: a set past
            // its deadline reads expired, and is no longer open.
            if row.state.phase != DecisionPhase::Open {
                continue;
            }
            if let Ok(set) = HeldQuestionSet::of(row) {
                sets.push(set);
            }
        }
        match listing.next_after {
            Some(next) if after.as_deref() != Some(next.as_str()) => after = Some(next),
            _ => break,
        }
    }
    sets.sort_by(|left, right| {
        left.row
            .descriptor
            .deadline_at_ms
            .cmp(&right.row.descriptor.deadline_at_ms)
            .then_with(|| {
                left.row
                    .decision_id
                    .as_str()
                    .cmp(right.row.decision_id.as_str())
            })
    });
    Ok(sets)
}

/// The asking member of every question set `decisions` hold, open or ended,
/// each as its set names it. A row this version cannot read names none. A
/// runtime that keeps no durable decisions has none. A read, with no rule
/// in it.
///
/// # Errors
/// The decision store's read failures.
// Read through `UnifiedRuntime::names_with_gating_records`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn question_set_askers(
    decisions: &RuntimeDecisions,
) -> Result<std::collections::BTreeSet<String>, DecisionError> {
    let mut askers = std::collections::BTreeSet::new();
    if !decisions.is_available() {
        return Ok(askers);
    }
    let filter = DecisionListFilter::default().with_kind(ask_user_questions_kind()?);
    // A candidate's read records nothing.
    let owner = decisions.owner().for_candidate_launch();
    let mut after: Option<String> = None;
    loop {
        let listing = owner.list(&filter, after.as_deref(), DECISION_PAGE_MAX)?;
        for row in listing.decisions {
            if let Ok(set) = HeldQuestionSet::of(row) {
                askers.insert(set.body.asker);
            }
        }
        match listing.next_after {
            Some(next) if after.as_deref() != Some(next.as_str()) => after = Some(next),
            _ => break,
        }
    }
    Ok(askers)
}

/// The question set the pending id `pending_id` names in `decisions`, open
/// or ended; `None` for an id that names none (not a decision id, no such
/// decision, a decision of another kind, a body this version cannot read),
/// which a decide route then answers as any unknown pending id.
pub(crate) fn held_question_set(
    decisions: &RuntimeDecisions,
    pending_id: &str,
) -> Option<HeldQuestionSet> {
    if !is_decision_id(pending_id) || !decisions.is_available() {
        return None;
    }
    let decision_id = DecisionId::new(pending_id.to_owned()).ok()?;
    let row = decisions.owner().get(&decision_id).ok()?;
    HeldQuestionSet::of(row).ok()
}

/// What a person decided of a question set.
#[derive(Debug, Clone)]
pub(crate) enum QuestionSetVerdict {
    /// The person answered: one entry per question they answered.
    Answer(Vec<AskUserQuestionAnswer>),
    /// The person closed the questions without answering.
    Dismiss,
}

/// What a decide route read of its request's `decision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionSetChoice {
    /// `approve`: the request carries the answers.
    Answer,
    /// `reject` or `deny`: a dismissal.
    Dismiss,
}

/// The verdict and the presented digest a `meta/gating/decide` request
/// carries for a question set.
///
/// # Errors
/// The sentence that says what the request lacks or gets wrong, for an
/// invalid params answer: a missing or malformed `presented_digest`, an
/// answer without `answers`, `answers` that do not parse, and a dismissal
/// that carries answers.
pub(crate) fn question_set_request(
    choice: QuestionSetChoice,
    params: &Value,
) -> Result<(QuestionSetVerdict, Sha256Digest), String> {
    let Some(digest) = params.get("presented_digest").and_then(Value::as_str) else {
        return Err(
            "presented_digest required: the subject_digest of the question set as it was shown"
                .to_owned(),
        );
    };
    let digest =
        Sha256Digest::new(digest).map_err(|error| format!("invalid presented_digest: {error}"))?;
    let answers = params.get("answers").filter(|answers| !answers.is_null());
    let verdict = match (choice, answers) {
        (QuestionSetChoice::Answer, Some(answers)) => QuestionSetVerdict::Answer(
            serde_json::from_value(answers.clone())
                .map_err(|error| format!("invalid answers: {error}"))?,
        ),
        (QuestionSetChoice::Answer, None) => {
            return Err(
                "answers required: an approving decision of a question set carries its answers"
                    .to_owned(),
            );
        }
        (QuestionSetChoice::Dismiss, None) => QuestionSetVerdict::Dismiss,
        (QuestionSetChoice::Dismiss, Some(_)) => {
            return Err("a dismissal carries no answers".to_owned());
        }
    };
    Ok((verdict, digest))
}

/// The authenticated event a person's decision of a question set arrived
/// as: who decided, on which ingress and channel, and when. A route builds
/// it only after it authenticated the caller; nothing of it is read from
/// the request.
#[derive(Debug, Clone)]
pub(crate) struct QuestionSetEvent {
    /// The person who decided, as the route authenticated them.
    pub(crate) approver: AuthenticatedApprover,
    /// The ingress and channel the decision arrived on: [`CONSOLE_CHANNEL`]
    /// or [`HOST_CHANNEL`].
    pub(crate) channel: &'static str,
    /// The reference of this one decide event.
    pub(crate) event_ref: DecisionReference,
    /// When the route observed the event, in UNIX milliseconds.
    pub(crate) observed_at_ms: u64,
    /// What the evidence keeps as its reason, when the route has something
    /// to record there.
    pub(crate) reason: Option<String>,
}

/// One person's decision of one question set, as its route hands it over.
#[derive(Debug, Clone)]
pub(crate) struct QuestionSetDecide {
    pub(crate) decision_id: DecisionId,
    pub(crate) verdict: QuestionSetVerdict,
    /// The digest of the set as the person was shown it.
    pub(crate) presented_digest: Sha256Digest,
    pub(crate) attested: QuestionSetEvent,
}

/// How a decided question set ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionSetOutcome {
    Answered,
    Dismissed,
}

impl QuestionSetOutcome {
    /// The outcome's token in a `meta/gating/decide` result.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::Dismissed => "dismissed",
        }
    }
}

/// A question set a person decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionSetDecided {
    pub(crate) pending_id: String,
    pub(crate) outcome: QuestionSetOutcome,
    pub(crate) decided_at_ms: u64,
}

impl QuestionSetDecided {
    /// The `meta/gating/decide` result of a decided question set.
    pub(crate) fn wire(&self) -> Value {
        json!({
            "pending_id": self.pending_id,
            "kind": QUESTION_SET_RESULT_KIND,
            "outcome": self.outcome.as_str(),
            "decided_at_ms": self.decided_at_ms,
        })
    }
}

/// Why a decision of a question set was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionSetRefusal {
    /// The id names no question set: a route answers it as it answers an
    /// unknown pending id, and the decision it names, if any, is untouched.
    NotAQuestionSet,
    /// The request is at fault: an invalid params answer with this
    /// sentence.
    Invalid(String),
    /// The decision itself refused the answer (it is no longer open, the
    /// digest is not the set's, ...): a route answers it typed, in the form
    /// the decision methods give a refusal ([`refused_answer`]).
    Refused(DecisionRefusal),
    /// The decider was not eligible when the decision committed: the route
    /// that built the eligibility says why.
    NotEligible,
    /// The decision owner failed: nothing is known to be decided.
    Failed(String),
}

/// The JSON-RPC answer `id` gets when the decision `decision_id` refused a
/// decide with `refusal`: the typed form of the decision methods
/// (`data.kind = "decision_refused"` with the refusal), never a sentence of
/// this module's own.
pub(crate) fn refused_answer(
    id: &Value,
    decision_id: &DecisionId,
    refusal: DecisionRefusal,
) -> Value {
    crate::durable_decisions::decision_error(
        id,
        DecisionError::Refused {
            decision_id: decision_id.clone(),
            refusal,
        },
    )
}

/// Whose question set a decision is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QuestionSetAsker {
    /// The decision is of another kind.
    NotAQuestionSet,
    /// The asking member's identity.
    Asker(String),
    /// A question set whose body this version cannot read: its asker is
    /// not known.
    Unreadable,
}

/// Whose question set `row` is: what a read of decisions asks before it
/// shows one, since a question set is seen only by who may see its asking
/// member.
pub(crate) fn question_set_asker(row: &StoredDecision) -> QuestionSetAsker {
    match row.descriptor.kind.as_str() {
        ASK_USER_QUESTIONS_KIND => match AskUserBody::of(row) {
            Ok(body) => QuestionSetAsker::Asker(body.asker),
            Err(_) => QuestionSetAsker::Unreadable,
        },
        _ => QuestionSetAsker::NotAQuestionSet,
    }
}

/// Hand one person's decision of a question set to the decision owner.
///
/// The match on the decision's kind is closed: only a decision of kind
/// `ask_user.questions` proceeds, and every other kind is
/// [`QuestionSetRefusal::NotAQuestionSet`] with the decision untouched. For
/// a question set, the answers are validated against the held set, carried
/// as typed data in the human evidence, and the decision is decided for the
/// set's approver id with a presentation that shows every canonical part of
/// its rendering at the event's observed time. `eligibility` is checked by
/// the owner when the decision commits.
///
/// # Errors
/// [`QuestionSetRefusal`].
pub(crate) fn answer_question_set(
    decisions: &RuntimeDecisions,
    decide: QuestionSetDecide,
    eligibility: &dyn ConsentEligibility,
) -> Result<QuestionSetDecided, QuestionSetRefusal> {
    let invalid = |error: &dyn std::fmt::Display| QuestionSetRefusal::Invalid(error.to_string());
    let owner = decisions.owner();
    let row = match owner.get(&decide.decision_id) {
        Ok(row) => row,
        Err(DecisionError::NotFound(_) | DecisionError::Unreadable { .. }) => {
            return Err(QuestionSetRefusal::NotAQuestionSet);
        }
        Err(error) => return Err(QuestionSetRefusal::Failed(error.to_string())),
    };
    let set = HeldQuestionSet::of(row)?;
    let answers = match decide.verdict {
        QuestionSetVerdict::Answer(answers) => Some(
            AskUserAnswers::for_set(&set.body.questions, answers)
                .map_err(|error| invalid(&error))?,
        ),
        QuestionSetVerdict::Dismiss => None,
    };
    let QuestionSetEvent {
        approver,
        channel,
        event_ref,
        observed_at_ms,
        reason,
    } = decide.attested;
    let descriptor = &set.row.descriptor;
    let presentation = PresentationRecord::complete(
        PresentationAudience::Approver {
            approver_id: descriptor.approver_id.clone(),
        },
        &descriptor.rendering,
        None,
        DecisionReference::new(channel).map_err(|error| invalid(&error))?,
        event_ref.clone(),
        observed_at_ms,
    );
    let mut decision = AttestedDecision::new(
        approver,
        DecisionIngress::new(channel).map_err(|error| invalid(&error))?,
        event_ref,
        observed_at_ms,
        descriptor.approver_id.clone(),
        decide.presented_digest,
        answers.is_some(),
    )
    .map_err(|error| invalid(&error))?
    .with_presentation(presentation);
    if let Some(reason) = reason {
        decision = decision
            .with_reason(reason)
            .map_err(|error| invalid(&error))?;
    }
    if let Some(answers) = &answers {
        decision = attest_ask_user_answers(decision, answers).map_err(|error| invalid(&error))?;
    }
    match owner.decide(&decide.decision_id, &decision, eligibility) {
        Ok(decided) => {
            decisions.notify_work();
            Ok(QuestionSetDecided {
                pending_id: decided.decision_id.to_string(),
                outcome: if answers.is_some() {
                    QuestionSetOutcome::Answered
                } else {
                    QuestionSetOutcome::Dismissed
                },
                decided_at_ms: decided.state.decided_at_ms.unwrap_or(observed_at_ms),
            })
        }
        Err(DecisionError::Refused { refusal, .. }) => Err(match refusal {
            DecisionRefusal::NotEligible => QuestionSetRefusal::NotEligible,
            refusal => QuestionSetRefusal::Refused(refusal),
        }),
        Err(error) => Err(QuestionSetRefusal::Failed(error.to_string())),
    }
}
