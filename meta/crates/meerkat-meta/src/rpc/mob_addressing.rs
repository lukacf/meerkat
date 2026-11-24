//! The addressing id of a request: `mob_id`.
//!
//! A request names the mob it is for with a top-level `mob_id` in its
//! params; without one it addresses the default mob. Every surface reads
//! the id here, once. What follows is the surface's: the stdio dispatch
//! judges that the mob is one the host serves before any arm runs; an arm
//! that serves one mob then resolves it, an arm whose service is keyed by
//! the mob reads the judged id, and an arm of the host, or of a method that
//! serves the launch mob alone, resolves none. Below an arm a mob is never
//! optional.
//!
//! The id is taken off the params, so a method whose parameter type refuses
//! members it does not know never sees it. The methods whose own parameter
//! `mob_id` is keep it ([`MobIdPlace`]).

use meerkat_mob::MobId;
use serde_json::Value;

use super::JsonRpcError;
use crate::access::AccessView;
use crate::unified_runtime::{MobResolveError, MobResolver};

/// `data.kind` of the refusal of a mob the host does not have.
pub(crate) const FOREIGN_MOB_TARGET_KIND: &str = "foreign_mob_target";

/// What is wrong with a `mob_id` that is neither a string nor `null`.
pub(crate) const MOB_ID_NOT_A_STRING: &str = "mob_id must be a string";

/// `meta/capabilities.feature_capabilities` name of a gateway that judges
/// the `mob_id` of a request: it serves the mob the id names and refuses an
/// id it does not serve. A gateway without it ignores the id on most methods
/// and refuses it on the methods whose params refuse members they do not
/// know, so a client sends `mob_id` only to a gateway that lists this name.
pub(crate) const MOB_ADDRESSING_FEATURE: &str = "mob_addressing";

/// The announcement methods that carry a mob of their own. The family
/// resolves it, as before: it also reaches a mob a member created, and it
/// answers a mob the host does not have with its own refusal. On the
/// console the door judges that id, and every other mob a request of the
/// family names inside its params (a member, a role selector, a set of
/// mobs, an audience grant, a declaration), with the request's caller
/// before the family reads the request, like the id of every other method:
/// a mob other than the launch mob that the caller may not address is
/// refused there like an id no mob has, and the family's listings are
/// shown to that caller without the rows that name such a mob.
pub(crate) const ANNOUNCEMENT_METHODS_NAMING_A_MOB: &[&str] = &[
    "meta/mob_announcements/announce",
    "meta/mob_announcements/get",
    "meta/mob_announcements/audit/list",
    "meta/mob_announcements/sources/put",
    "meta/mob_announcements/sources/pause",
    "meta/mob_announcements/sources/resume",
    "meta/mob_announcements/sources/end",
    "meta/mob_announcements/sources/get",
    "meta/mob_announcements/sources/list",
    "meta/mob_announcements/sources/advance",
    "meta/mob_announcements/sources/record_failure",
];

/// The methods whose own parameter `mob_id` is the addressing id: the event
/// query's mob, and the mob Core's declaration params require.
const METHODS_KEEPING_THE_MOB: &[&str] = &[
    "meta/mob_events/query",
    "meta/mob_events/subscribe",
    super::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION,
    super::mob_methods::APPLY_MEMBER_TOOL_DECLARATION,
    super::mob_methods::MEMBER_TOOL_DECLARATION,
];

/// The methods of the job door: the stdio gateway answers them on a door of
/// their own, before this seam and before the dispatch. They address a job,
/// and a job's mob is a fact of the job's record: none of them takes a
/// `mob_id`. Six of them are `meta/*` methods, the callbacks of a job's
/// runner (`heartbeat`, `progress`, `checkpoint`, `complete`, `fail`,
/// `cancel_ack`); they name their job by its `authority`, and a `mob_id`
/// beside it is a member their params do not know and ignore. The job door
/// reads this list and no other.
pub(crate) const JOB_DOOR_METHODS: &[&str] = &[
    "jobs/get",
    "jobs/list",
    "jobs/cancel",
    "jobs/progress",
    "jobs/result",
    "jobs/artifacts",
    "jobs/retry",
    "jobs/health",
    "jobs/subscribe",
    "jobs/unsubscribe",
    "monitors/start",
    "meta/jobs/heartbeat",
    "meta/jobs/progress",
    "meta/jobs/checkpoint",
    "meta/jobs/complete",
    "meta/jobs/fail",
    "meta/jobs/cancel_ack",
];

/// What the seam does with a method's top-level `mob_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobIdPlace {
    /// Read and removed: the method's own parse never sees it.
    Taken,
    /// Read and left: it is the method's own parameter too.
    Kept,
    /// Left unread: the method's family resolves it.
    FamilyResolved,
    /// The method addresses no mob, and its params are not this seam's.
    NotAddressed,
}

fn mob_id_place(method: &str) -> MobIdPlace {
    if ANNOUNCEMENT_METHODS_NAMING_A_MOB.contains(&method)
        || super::mob_hosting::MOB_HOSTING_METHODS_NAMING_A_MOB.contains(&method)
    {
        // The announcement family, and the methods over the mobs of a host
        // that act on one mob: each reads the mob it names itself, and
        // answers for mobs this seam serves no request for (a mob a member
        // created, a mob whose store did not load).
        MobIdPlace::FamilyResolved
    } else if METHODS_KEEPING_THE_MOB.contains(&method) {
        MobIdPlace::Kept
    } else if matches!(method, "meta/init" | "meta/shutdown") || JOB_DOOR_METHODS.contains(&method)
    {
        // The launch, its end, and the job door's methods (six of them are
        // `meta/*` methods): none is a request to a mob.
        MobIdPlace::NotAddressed
    } else if method.starts_with("meta/") || method == "live/webrtc/answer" {
        MobIdPlace::Taken
    } else {
        // A module's own method, a callback, a job: not a request to a mob.
        MobIdPlace::NotAddressed
    }
}

/// The console methods that are the host's: a method on this list reads no
/// value of any mob. The console's door does not resolve a mob for such a
/// method (it checks the id the request names, which loads nothing) and
/// resolves one for every other method its runtime dispatch serves.
/// Sorted, one name per line.
pub(crate) const HOST_SCOPED_CONSOLE_METHODS: &[&str] = &[
    "meta/access/enable",
    "meta/access/get",
    "meta/access/groups/delete",
    "meta/access/groups/set",
    "meta/access/preview",
    "meta/access/rules/delete",
    "meta/access/rules/upsert",
    "meta/access/set",
    "meta/access/status",
    "meta/blob/get",
    "meta/blob/upload",
    "meta/console/attention",
    "meta/console/attention/dismiss",
    "meta/console/audit",
    "meta/console/revoke_sessions",
    "meta/console/sign_out",
    "meta/console/whoami",
    "meta/cross_mob/directory",
    "meta/decisions/decide",
    "meta/decisions/get",
    "meta/decisions/list",
    "meta/decisions/notice",
    "meta/decisions/present",
    "meta/decisions/rendering",
    "meta/decisions/withdraw",
    "meta/mob_announcements/declarations/apply",
    "meta/mob_announcements/declarations/get",
    "meta/mob_announcements/deliveries/dismiss",
    "meta/mob_announcements/deliveries/list",
    "meta/mob_announcements/deliveries/observe",
    "meta/mob_announcements/deliveries/reconcile",
    "meta/mob_announcements/deliveries/redeliver",
    "meta/mob_announcements/escalations/list",
    "meta/mob_announcements/members/get",
    "meta/mob_announcements/members/list",
    "meta/mob_announcements/subscriptions/cancel",
    "meta/mob_announcements/subscriptions/get",
    "meta/mob_announcements/subscriptions/list",
    "meta/mob_announcements/subscriptions/pause",
    "meta/mob_announcements/subscriptions/put",
    "meta/mob_announcements/subscriptions/resume",
    "meta/mobs/create",
    "meta/mobs/destroy",
    "meta/mobs/get",
    "meta/mobs/lifecycle",
    "meta/mobs/list",
    "meta/mobs/redefine",
    "meta/peer_pubkey",
    "meta/schedule/tools",
    "meta/storage/doctor",
];

/// Whether the console method `method` is the host's
/// ([`HOST_SCOPED_CONSOLE_METHODS`]).
pub(crate) fn console_method_is_host_scoped(method: &str) -> bool {
    HOST_SCOPED_CONSOLE_METHODS.binary_search(&method).is_ok()
}

/// The stdio methods whose arms act on the launch mob's services without
/// resolving the request's mob. The stdio dispatch serves such a method for
/// the launch mob alone: for another mob the host serves it is refused,
/// with the refusal of a mob id that says the host has the mob. A method
/// leaves this list in the change that makes its arm serve the mob the
/// request names. None is left: the delivery methods were the last, and
/// they now act for the mob the request names (the host's delivery service
/// keeps each mob's deliveries apart).
/// Sorted, one name per line: the source rows read the list in that form.
#[rustfmt::skip]
pub(crate) const STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY: &[&str] = &[
];

/// Whether the stdio method `method` serves the launch mob alone: a listed
/// method ([`STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY`]), or a method of
/// the live family, by the one test the dispatch's live arm is guarded by
/// (`is_stdio_live_method`; a source row keeps the two one:
/// `the_live_family_is_the_same_for_its_arm_and_for_the_rule_that_refuses_it`).
/// The host has one live transport, composed for the
/// launch mob: it is bound to that mob's sessions and engages its one voice
/// path under an identity's name. The dispatch refuses such a method for
/// any other mob before its arm, and `meta/capabilities` does not list it
/// for one.
pub(crate) fn stdio_method_serves_the_launch_mob_only(method: &str) -> bool {
    STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY
        .binary_search(&method)
        .is_ok()
        || super::is_stdio_live_method(method)
}

/// The mob a request names, read from the wire: the one place where a mob
/// is optional. Whether the host serves it is judged before the method's
/// arm runs; an arm that serves one mob resolves it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct NamedMob(Option<MobId>);

impl NamedMob {
    /// The id the request names; `None` when it names no mob.
    pub(crate) fn named(&self) -> Option<&MobId> {
        self.0.as_ref()
    }

    /// The mob a query parameter names (an event stream route).
    pub(crate) fn from_query(mob_id: Option<String>) -> Self {
        Self(mob_id.map(MobId::from))
    }

    /// The mob a member of a request's body names (a console route that
    /// takes a JSON body): absent or `null` names no mob, a string is an id
    /// like any other, and any other value is refused with what is wrong
    /// with it.
    pub(crate) fn from_member(mob_id: Option<Value>) -> Result<Self, &'static str> {
        match mob_id {
            None | Some(Value::Null) => Ok(Self(None)),
            Some(Value::String(mob_id)) => Ok(Self(Some(MobId::from(mob_id)))),
            Some(_) => Err(MOB_ID_NOT_A_STRING),
        }
    }
}

/// Read the mob `method`'s request names from `params`, and take the id off
/// the params unless it is the method's own parameter. An absent or `null`
/// `mob_id` names no mob; a blank one is an id like any other. A request
/// that names no mob leaves `params` untouched.
pub(crate) fn take_named_mob(method: &str, params: &mut Value) -> Result<NamedMob, JsonRpcError> {
    let place = mob_id_place(method);
    if matches!(place, MobIdPlace::NotAddressed | MobIdPlace::FamilyResolved) {
        return Ok(NamedMob::default());
    }
    let Some(object) = params.as_object_mut() else {
        return Ok(NamedMob::default());
    };
    let named = match object.get("mob_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(mob_id)) => Some(MobId::from(mob_id.as_str())),
        Some(_) => {
            return Err(JsonRpcError {
                code: -32602,
                message: format!("Invalid params: {MOB_ID_NOT_A_STRING}"),
                data: None,
            });
        }
    };
    // A `null` id is taken off as well: it names no mob, and a method that
    // refuses members it does not know must not meet it.
    if place == MobIdPlace::Taken && object.contains_key("mob_id") {
        object.remove("mob_id");
    }
    Ok(NamedMob(named))
}

/// The mob a request by position (params that are an array) names on a
/// method whose own parameter `mob_id` is: the id as that method's own
/// parameter type reads it. `None` when the params are no array, when they
/// name no mob, and when the method's own parse refuses them: the method's
/// handler answers that.
fn mob_named_by_position(method: &str, params: &Value) -> Option<MobId> {
    use meerkat_contracts::wire;

    if !params.is_array() {
        return None;
    }
    let params = params.clone();
    let mob_id = if method == super::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION {
        serde_json::from_value::<wire::MobAdoptMemberIdentityDeclarationParams>(params)
            .ok()
            .map(|params| params.mob_id)
    } else if method == super::mob_methods::APPLY_MEMBER_TOOL_DECLARATION {
        serde_json::from_value::<wire::MobApplyMemberToolDeclarationParams>(params)
            .ok()
            .map(|params| params.mob_id)
    } else if method == super::mob_methods::MEMBER_TOOL_DECLARATION {
        serde_json::from_value::<wire::MobMemberToolDeclarationParams>(params)
            .ok()
            .map(|params| params.mob_id)
    } else {
        serde_json::from_value::<crate::unified_runtime::EventQuery>(params)
            .ok()
            .and_then(|query| query.mob_id)
    };
    mob_id.map(MobId::from)
}

/// The stdio dispatch's reading of the mob a request names:
/// [`take_named_mob`], and, on a method whose own parameter `mob_id` is,
/// also the id a request names by position. Those methods' parameter types
/// read params that are an array as well as params that are an object, and
/// the stdio gateway keeps both forms: a request names the same mob in
/// either. The params are left as they came.
///
/// The console's request doors and the decision door read the id with
/// [`take_named_mob`] alone.
pub(crate) fn take_named_mob_on_stdio(
    method: &str,
    params: &mut Value,
) -> Result<NamedMob, JsonRpcError> {
    let named = take_named_mob(method, params)?;
    if named.named().is_none()
        && mob_id_place(method) == MobIdPlace::Kept
        && let Some(mob_id) = mob_named_by_position(method, params)
    {
        return Ok(NamedMob(Some(mob_id)));
    }
    Ok(named)
}

/// The stdio dispatch's refusal of a request that names a mob this host
/// does not have: `-32602`, `data.kind = "foreign_mob_target"`, with the id
/// the request named and the host's default mob. The one caller of that
/// dispatch is the host itself. A door that has callers answers
/// [`mob_not_served`] instead.
pub(crate) fn foreign_mob_target(requested: &str, default_mob: &str) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: format!(
            "request targets mob '{requested}' but this gateway serves '{default_mob}'"
        ),
        data: Some(serde_json::json!({
            "kind": FOREIGN_MOB_TARGET_KIND,
            "requested_mob_id": requested,
            "gateway_mob_id": default_mob,
        })),
    }
}

/// The stdio dispatch's answer of a mob that did not resolve: the refusal
/// of an id the host does not have, or, for a mob the host has and its mob
/// set could not serve, the mob set's own error as this gateway answers
/// every mob error (a store that did not load answers `mob_quarantined`).
pub(crate) fn mob_resolve_refusal(error: &MobResolveError) -> JsonRpcError {
    match error {
        MobResolveError::NotHosted {
            requested,
            default_mob,
        } => foreign_mob_target(requested.as_str(), default_mob.as_str()),
        MobResolveError::Unavailable { error, .. } => mob_set_error(error.error()),
    }
}

/// A mob set's error as this gateway answers Meerkat's mob errors: its
/// typed code and structured data.
fn mob_set_error(error: &meerkat_mob::MobError) -> JsonRpcError {
    match crate::identity_first::runtime::mob_error_wire(error) {
        Ok((code, data)) => JsonRpcError {
            code,
            message: error.to_string(),
            data,
        },
        Err(serialize_error) => JsonRpcError {
            code: -32603,
            message: format!("failed to serialize mob error detail: {serialize_error}"),
            data: None,
        },
    }
}

/// The mob a member's session is bound to, when a live request names its
/// target by a raw `session_id` and that mob is not the launch mob.
///
/// The host has one live transport, composed for the launch mob. A request
/// that names a mob is judged before the live arm runs; a raw session id
/// names none, and every mob of the host keeps its members' sessions in the
/// launch's one session service. So the session's own stored member binding
/// is asked whose member it is: `Ok(Some(mob))` when it names a mob other
/// than `launch_mob`. This is the judgement's first half only: the stdio
/// dispatch then asks the mob set's records whether that mob is in the
/// reach of a mob the host hosts beside its launch mob, and builds the
/// refusal itself.
///
/// `Ok(None)`, served as before: the request names no session by id; the
/// session has no stored record or no member binding (a session that is no
/// member's); the binding names `launch_mob`; or the runtime holds no
/// session service to ask (a runtime made from a bare handle, which has no
/// mob set and so hosts no second mob). A stored record that cannot be read
/// is an error: the request is not served on a judgement that could not be
/// made.
pub(crate) async fn mob_of_a_raw_live_session(
    sessions: Option<&std::sync::Arc<dyn meerkat_mob::MobSessionService>>,
    launch_mob: &MobId,
    params: &Value,
) -> Result<Option<String>, String> {
    let Some(session) = params
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|raw| meerkat_core::types::SessionId::parse(raw).ok())
    else {
        return Ok(None);
    };
    let Some(sessions) = sessions else {
        return Ok(None);
    };
    let binding = sessions
        .load_persisted_session_metadata(&session)
        .await
        .map_err(|error| format!("the session's stored member binding cannot be read: {error}"))?
        .and_then(|view| view.session_metadata)
        .and_then(|metadata| metadata.mob_member_binding);
    Ok(binding
        .map(|binding| binding.mob_id)
        .filter(|mob| mob != launch_mob.as_str()))
}

/// The refusal of a mob id on a door that has callers (the console's
/// request doors, the event streams): `-32602`, `data.kind =
/// "foreign_mob_target"`, with the id the request named and nothing else.
///
/// It is such a door's one answer for an id it does not serve to its
/// caller, whatever the reason and whoever asks. It is built from the
/// requested id alone, so it cannot name another mob and cannot tell an id
/// the host does not have from one the caller may not address.
pub(crate) fn mob_not_served(requested: &str) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: format!(
            "request targets mob '{requested}', which this gateway does not serve to the caller"
        ),
        data: Some(serde_json::json!({
            "kind": FOREIGN_MOB_TARGET_KIND,
            "requested_mob_id": requested,
        })),
    }
}

/// The answer of a door that has callers for a mob that did not resolve:
/// [`mob_not_served`], for every reason a mob does not resolve.
pub(crate) fn mob_refusal_for_a_caller(error: &MobResolveError) -> JsonRpcError {
    match error {
        MobResolveError::NotHosted { requested, .. }
        | MobResolveError::Unavailable { requested, .. } => mob_not_served(requested.as_str()),
    }
}

/// The mob a request on a door that has callers is for, once the door has
/// judged the id the request names with its caller in hand
/// ([`judge_named_mob`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AddressedMob {
    /// The launch mob: the request names no mob, or names the launch mob's
    /// id as the door knows it. Every caller the door let past its sign-in
    /// check addresses it.
    Launch,
    /// Another mob, by its id, which the caller may address
    /// ([`AccessView::may_address_other_mob`]).
    Other(MobId),
}

/// The caller of a door, as the judgement of a request's mob takes it and
/// hands it back: a door goes on with the caller the judgement returned and
/// with no other.
pub(crate) trait DoorCaller: Sized {
    /// The caller's access view. `None` for a caller the door did not
    /// identify: such a caller addresses the launch mob only.
    fn access_view(&self) -> Option<&AccessView>;

    /// This caller with its access view taken for the mob `mob_id`
    /// ([`AccessView::in_mob`]): its agent checks from here on read a name
    /// as an agent of that mob.
    #[must_use]
    fn seeing_mob(self, mob_id: &MobId) -> Self;
}

impl DoorCaller for AccessView {
    fn access_view(&self) -> Option<&AccessView> {
        Some(self)
    }

    fn seeing_mob(self, mob_id: &MobId) -> Self {
        self.in_mob(mob_id)
    }
}

impl<C: DoorCaller> DoorCaller for Option<C> {
    fn access_view(&self) -> Option<&AccessView> {
        self.as_ref().and_then(DoorCaller::access_view)
    }

    fn seeing_mob(self, mob_id: &MobId) -> Self {
        self.map(|caller| caller.seeing_mob(mob_id))
    }
}

/// A caller on a door that serves the launch mob only: what the door
/// answers with is the launch mob's own (values keyed by an identity alone,
/// or captured when its router was built), so no caller addresses another
/// mob there, an administrator included. The judgement answers such a
/// caller's request of any other mob [`mob_not_served`], before anything is
/// asked about that mob, and hands the caller back as it came.
///
/// A door stops wrapping its caller in the change that makes everything it
/// reads the resolved mob's.
pub(crate) struct LaunchMobOnly<C>(pub(crate) C);

impl<C: DoorCaller> DoorCaller for LaunchMobOnly<C> {
    fn access_view(&self) -> Option<&AccessView> {
        None
    }

    fn seeing_mob(self, _mob_id: &MobId) -> Self {
        // Never asked: this caller addresses no other mob.
        self
    }
}

/// The judgement of the mob a request names on a door that has callers,
/// made with the caller in hand, right after the door knows its caller and
/// before it asks its resolver anything or applies a rule of its own.
///
/// `launch_mob` is the launch mob's id as the door knows it. A request that
/// names no mob, or that id, is for the launch mob, whoever asks, and the
/// caller is handed back as it came. Any other id is another mob: a caller
/// who may address it ([`AccessView::may_address_other_mob`]) is handed
/// back with its view taken for that mob, and every other caller is
/// answered [`mob_not_served`], the door's answer for an id no mob has.
/// Nothing is resolved, loaded or looked up to judge: the question reads
/// the caller's access view alone.
pub(crate) fn judge_named_mob<C: DoorCaller>(
    named: &NamedMob,
    launch_mob: Option<&MobId>,
    caller: C,
) -> Result<(AddressedMob, C), JsonRpcError> {
    let Some(requested) = named.named() else {
        return Ok((AddressedMob::Launch, caller));
    };
    if launch_mob == Some(requested) {
        return Ok((AddressedMob::Launch, caller));
    }
    let may_address = caller
        .access_view()
        .is_some_and(|view| view.may_address_other_mob(requested));
    if !may_address {
        return Err(mob_not_served(requested.as_str()));
    }
    Ok((
        AddressedMob::Other(requested.clone()),
        caller.seeing_mob(requested),
    ))
}

/// The launch mob's id as a door with the host's resolver knows it: the
/// mob the resolver serves a request that names none. The question names no
/// mob and loads nothing.
pub(crate) async fn launch_mob_of(resolver: &MobResolver) -> Option<MobId> {
    resolver.served_mob_id(None).await.ok()
}

/// [`judge_named_mob`] for a door that only needs to know that the mob is
/// one its host serves (it resolves none): the judgement first, then the
/// resolver's answer for the id, which loads nothing. A door without the
/// host's resolver knows the one mob `own_mob` ([`refuse_unless_default`]).
/// Every refusal is [`mob_not_served`].
pub(crate) async fn judge_served_mob<C: DoorCaller>(
    named: &NamedMob,
    resolver: Option<&MobResolver>,
    own_mob: Option<&MobId>,
    caller: C,
    door: &'static str,
) -> Result<(AddressedMob, C), JsonRpcError> {
    let Some(resolver) = resolver else {
        return refuse_unless_default(named, own_mob, caller, door)
            .map(|caller| (AddressedMob::Launch, caller));
    };
    let judged = judge_named_mob(named, launch_mob_of(resolver).await.as_ref(), caller)?;
    resolver
        .served_mob_id(named.named())
        .await
        .map_err(|error| mob_refusal_for_a_caller(&error))?;
    Ok(judged)
}

/// The door a handler answers on, when the stdio dispatch and a door that
/// has callers both reach it: such a handler refuses a mob id in the form
/// of the door it was called on. A door reads the id of a request before
/// the method's handler does; this is for a handler that reads its own
/// `mob_id` parameter again (the member declaration family), so that its
/// answer is the door's answer whatever the shape of the request's params.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobRefusalDoor {
    /// The stdio dispatch: its one caller is the host, which is answered
    /// [`foreign_mob_target`].
    Host,
    /// A door that has callers (the console's request doors), which
    /// answers [`mob_not_served`].
    Callers,
}

/// Set once a door answered without its host's resolver. One flag for the
/// whole process: every door shares it, so a row that reads it depends on
/// the rows that ran before it in the same process. Under nextest each row
/// runs in a process of its own; under `cargo test` the rows that read it
/// depend on their order.
static DOOR_WITHOUT_RESOLVER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether a door of this process answered without its host's resolver.
#[cfg(test)]
pub(crate) fn a_door_answered_without_its_resolver() -> bool {
    DOOR_WITHOUT_RESOLVER.load(std::sync::atomic::Ordering::Relaxed)
}

/// The answer of a door that was mounted without its host's mob resolver (a
/// router a host built by hand from the public handler functions, or a
/// test's): it knows only the mob its state was built over. The request's
/// mob is that mob or none; every other id is refused as a door that has
/// callers refuses it ([`mob_not_served`]: the door's own mob is not
/// named), and an id is never taken for the default mob's. No router the
/// runtime builds reaches this; the first use is logged.
///
/// The id is judged with the door's caller like on every other door
/// ([`judge_named_mob`], with the door's own mob as the launch mob), and
/// the caller is handed back. The door knows one mob, so it serves no other
/// to any caller: a caller who may address another mob is answered the
/// same refusal.
pub(crate) fn refuse_unless_default<C: DoorCaller>(
    named: &NamedMob,
    default_mob: Option<&MobId>,
    caller: C,
    door: &'static str,
) -> Result<C, JsonRpcError> {
    if !DOOR_WITHOUT_RESOLVER.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(
            door,
            "a request door has no mob resolver: it serves its own mob only and refuses every other mob id"
        );
    }
    match judge_named_mob(named, default_mob, caller)? {
        (AddressedMob::Launch, caller) => Ok(caller),
        (AddressedMob::Other(requested), _) => Err(mob_not_served(requested.as_str())),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A request that names no mob is handed on exactly as it came.
    #[test]
    fn a_request_that_names_no_mob_keeps_its_params_untouched() {
        for method in ["meta/send", "meta/workgraph/facts", "meta/models/catalog"] {
            for original in [
                json!({"identity": "worker", "text": "hello"}),
                json!({}),
                Value::Null,
                json!(["positional"]),
            ] {
                let mut params = original.clone();
                let named = take_named_mob(method, &mut params).expect("no mob named");
                assert_eq!(named.named(), None, "{method}");
                assert_eq!(params, original, "{method}");
                assert_eq!(params.to_string(), original.to_string(), "{method}");
            }
        }
    }

    /// The id is read and taken off, so the method's own parse never meets
    /// it; a `null` id names no mob and is taken off as well.
    #[test]
    fn the_id_is_taken_off_the_params_of_an_addressed_method() {
        let mut params = json!({"mob_id": "b", "identity": "worker"});
        let named = take_named_mob("meta/send", &mut params).expect("a mob named");
        assert_eq!(named.named().map(MobId::as_str), Some("b"));
        assert_eq!(params, json!({"identity": "worker"}));

        let mut params = json!({"mob_id": null, "identity": "worker"});
        let named = take_named_mob("meta/send", &mut params).expect("no mob named");
        assert_eq!(named.named(), None);
        assert_eq!(params, json!({"identity": "worker"}));

        let mut params = json!({"mob_id": ""});
        let named = take_named_mob("live/webrtc/answer", &mut params).expect("a blank id");
        assert_eq!(named.named().map(MobId::as_str), Some(""));
    }

    /// A method whose own parameter the id is keeps it, and the family that
    /// resolves its own mob is left alone.
    #[test]
    fn a_method_that_carries_its_own_mob_keeps_it() {
        for method in METHODS_KEEPING_THE_MOB {
            let mut params = json!({"mob_id": "b"});
            let named = take_named_mob(method, &mut params).expect("a mob named");
            assert_eq!(named.named().map(MobId::as_str), Some("b"), "{method}");
            assert_eq!(params, json!({"mob_id": "b"}), "{method}");
        }
        for method in ANNOUNCEMENT_METHODS_NAMING_A_MOB {
            assert!(
                crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS.contains(method),
                "{method} is an announcement method"
            );
            let mut params = json!({"mob_id": "b"});
            let named = take_named_mob(method, &mut params).expect("left to the family");
            assert_eq!(named.named(), None, "{method}");
            assert_eq!(params, json!({"mob_id": "b"}), "{method}");
        }
        // The methods over the mobs of a host that act on one mob read
        // their own `mob_id`, whatever it is; the two that name no mob
        // take the addressing id like every method of the host.
        for method in crate::rpc::mob_hosting::MOB_HOSTING_METHODS_NAMING_A_MOB {
            for mob_id in [json!("b"), json!(7), Value::Null] {
                let mut params = json!({"mob_id": mob_id});
                let named = take_named_mob(method, &mut params).expect("left to the family");
                assert_eq!(named.named(), None, "{method}");
                assert_eq!(params, json!({"mob_id": mob_id}), "{method}");
            }
        }
        for method in [
            crate::rpc::mob_hosting::MOBS_LIST,
            crate::rpc::mob_hosting::MOBS_CREATE,
        ] {
            let mut params = json!({"mob_id": "b", "mob_config": "text"});
            let named = take_named_mob(method, &mut params).expect("a mob named");
            assert_eq!(named.named().map(MobId::as_str), Some("b"), "{method}");
            assert_eq!(params, json!({"mob_config": "text"}), "{method}");
        }
    }

    /// What is no request to a mob is not read: a module's method keeps a
    /// `mob_id` of its own.
    #[test]
    fn a_method_that_addresses_no_mob_is_not_read() {
        for method in ["routing/resolve", "meta/init", "meta/shutdown", "jobs/list"] {
            let mut params = json!({"mob_id": 7});
            let named = take_named_mob(method, &mut params).expect("not read");
            assert_eq!(named.named(), None, "{method}");
            assert_eq!(params, json!({"mob_id": 7}), "{method}");
        }
    }

    /// The stdio dispatch reads the mob a request names by position on a
    /// method whose own parameter the id is, through that method's own
    /// parameter type, and leaves the params as they came. The shared
    /// reading, which the doors with callers use, reads no id from an
    /// array.
    #[test]
    fn the_stdio_dispatch_reads_a_mob_named_by_position_on_a_method_that_keeps_its_mob() {
        let query = json!([null, null, null, null, "b"]);
        let declaration = json!(["b", "worker-1"]);
        for (method, by_position) in [
            ("meta/mob_events/query", &query),
            ("meta/mob_events/subscribe", &query),
            (
                crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION,
                &declaration,
            ),
        ] {
            let mut params = by_position.clone();
            let named = take_named_mob_on_stdio(method, &mut params).expect("a mob named");
            assert_eq!(named.named().map(MobId::as_str), Some("b"), "{method}");
            assert_eq!(&params, by_position, "{method}");

            let mut params = by_position.clone();
            let shared = take_named_mob(method, &mut params).expect("not read");
            assert_eq!(shared.named(), None, "{method}");
        }

        // By name it reads what the shared reading reads.
        let mut params = json!({"mob_id": "b"});
        let named = take_named_mob_on_stdio("meta/mob_events/query", &mut params).expect("named");
        assert_eq!(named.named().map(MobId::as_str), Some("b"));
        assert_eq!(params, json!({"mob_id": "b"}));

        // An array the method's own type refuses names no mob: the method's
        // handler answers it.
        let mut params = json!(["b"]);
        let named = take_named_mob_on_stdio(
            crate::rpc::mob_methods::MEMBER_TOOL_DECLARATION,
            &mut params,
        )
        .expect("left to the method");
        assert_eq!(named.named(), None);

        // An array that names no mob names none, and a method that does not
        // keep its mob reads no id from an array.
        let mut params = json!([null, null, null, null, null]);
        let named = take_named_mob_on_stdio("meta/mob_events/query", &mut params).expect("none");
        assert_eq!(named.named(), None);
        let mut params = json!(["b"]);
        let named = take_named_mob_on_stdio("meta/send", &mut params).expect("not read");
        assert_eq!(named.named(), None);
        assert_eq!(params, json!(["b"]));
    }

    /// An id that is no string is refused before anything else reads it.
    #[test]
    fn an_id_that_is_no_string_is_refused() {
        let mut params = json!({"mob_id": 7});
        let refusal = take_named_mob("meta/send", &mut params).expect_err("refused");
        assert_eq!(refusal.code, -32602);
        assert_eq!(refusal.message, "Invalid params: mob_id must be a string");
    }

    /// The callers of the judgement's rows, under one enforced access
    /// configuration: an administrator, a person whose allow selects the
    /// mob `second`, and a person no rule names.
    fn judged_callers() -> [(&'static str, AccessView); 3] {
        use crate::access::{AccessControlConfig, AccessController, AccessRule};
        let controller = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![AccessRule {
                id: "sees-second".to_string(),
                subjects: vec!["second@example.test".to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec!["second".to_string()],
                ..AccessRule::default()
            }],
            ..AccessControlConfig::default()
        })
        .expect("access configuration");
        [
            "root@example.test",
            "second@example.test",
            "nobody@example.test",
        ]
        .map(|subject| (subject, controller.view_for_subject(Some(subject))))
    }

    /// A surface that knows only its default mob serves that mob by its id
    /// and by no id, and refuses every other id by the id the request
    /// named, without naming its own mob: to every caller alike, a caller
    /// who may address another mob included.
    #[test]
    fn a_surface_with_one_mob_refuses_every_other_id() {
        let default_mob = MobId::from("a");
        let none = NamedMob::default();
        let own = NamedMob::from_query(Some("a".to_string()));
        let other = NamedMob::from_query(Some("second".to_string()));
        let unidentified: Option<AccessView> = None;
        assert!(!a_door_answered_without_its_resolver());
        assert!(
            refuse_unless_default(&none, Some(&default_mob), unidentified.clone(), "row").is_ok()
        );
        assert!(a_door_answered_without_its_resolver());
        let callers: Vec<(&str, Option<AccessView>)> = judged_callers()
            .into_iter()
            .map(|(who, view)| (who, Some(view)))
            .chain([("a caller the door did not identify", unidentified)])
            .collect();
        for (who, caller) in callers {
            assert!(
                refuse_unless_default(&own, Some(&default_mob), caller.clone(), "row").is_ok(),
                "{who}"
            );
            assert!(
                refuse_unless_default(&none, None, caller.clone(), "row").is_ok(),
                "{who}"
            );
            let refusal = refuse_unless_default(&other, Some(&default_mob), caller.clone(), "row")
                .expect_err("refused");
            assert_eq!(refusal.code, -32602, "{who}");
            assert_eq!(
                refusal.data,
                Some(json!({
                    "kind": "foreign_mob_target",
                    "requested_mob_id": "second",
                })),
                "{who}"
            );
            assert_eq!(
                refusal.message,
                "request targets mob 'second', which this gateway does not serve to the caller",
                "{who}"
            );
            assert!(
                refuse_unless_default(&own, None, caller, "row").is_err(),
                "{who}"
            );
        }
    }

    /// A request that names no mob or the launch mob's id is for the launch
    /// mob, whoever asks. Another mob is for the callers who may address
    /// it; every other caller, and a caller the door did not identify, is
    /// answered what the door answers for an id no mob has.
    #[test]
    fn the_mob_a_request_names_is_judged_with_its_caller() {
        let launch = MobId::from("launch");
        let named = |mob_id: &str| NamedMob::from_query(Some(mob_id.to_string()));
        for (who, caller) in judged_callers() {
            for for_the_launch_mob in [NamedMob::default(), named("launch")] {
                let (addressed, _) =
                    judge_named_mob(&for_the_launch_mob, Some(&launch), caller.clone())
                        .expect("the launch mob is every caller's");
                assert_eq!(addressed, AddressedMob::Launch, "{who}");
            }
            let of_second = judge_named_mob(&named("second"), Some(&launch), caller.clone());
            let of_third = judge_named_mob(&named("third"), Some(&launch), caller.clone());
            match who {
                "root@example.test" => {
                    for (judged, mob) in [(of_second, "second"), (of_third, "third")] {
                        let (addressed, _) = judged.expect("an administrator addresses every mob");
                        assert_eq!(addressed, AddressedMob::Other(MobId::from(mob)));
                    }
                }
                "second@example.test" => {
                    let (addressed, _) = of_second.expect("the allow selects the mob");
                    assert_eq!(addressed, AddressedMob::Other(MobId::from("second")));
                    assert_eq!(
                        of_third.map(|(addressed, _)| addressed),
                        Err(mob_not_served("third"))
                    );
                }
                _ => {
                    assert_eq!(
                        of_second.map(|(addressed, _)| addressed),
                        Err(mob_not_served("second")),
                        "{who}"
                    );
                    assert_eq!(
                        of_third.map(|(addressed, _)| addressed),
                        Err(mob_not_served("third")),
                        "{who}"
                    );
                }
            }
            // A door that knows no mob by its id: every named id is another
            // mob's, the caller's own judgement as above.
            assert_eq!(
                judge_named_mob(&named("launch"), None, caller.clone()).is_ok(),
                who == "root@example.test",
                "{who}"
            );
        }
        let unidentified: Option<AccessView> = None;
        assert_eq!(
            judge_named_mob(&NamedMob::default(), Some(&launch), unidentified.clone())
                .map(|(addressed, _)| addressed),
            Ok(AddressedMob::Launch)
        );
        assert_eq!(
            judge_named_mob(&named("second"), Some(&launch), unidentified)
                .map(|(addressed, _)| addressed),
            Err(mob_not_served("second"))
        );
    }

    /// On a door that serves the launch mob only, every caller is answered
    /// for another mob what the door answers for an id no mob has, an
    /// administrator included, and is served the launch mob as it came.
    #[test]
    fn a_door_of_the_launch_mob_only_refuses_every_other_mob_to_every_caller() {
        let launch = MobId::from("launch");
        let named = |mob_id: &str| NamedMob::from_query(Some(mob_id.to_string()));
        for (who, caller) in judged_callers() {
            for for_the_launch_mob in [NamedMob::default(), named("launch")] {
                let (addressed, LaunchMobOnly(handed_back)) = judge_named_mob(
                    &for_the_launch_mob,
                    Some(&launch),
                    LaunchMobOnly(caller.clone()),
                )
                .map_err(|refusal| refusal.message)
                .expect("the launch mob is every caller's");
                assert_eq!(addressed, AddressedMob::Launch, "{who}");
                assert_eq!(handed_back.subject(), caller.subject(), "{who}");
            }
            assert_eq!(
                judge_named_mob(&named("second"), Some(&launch), LaunchMobOnly(caller))
                    .map(|(addressed, _)| addressed),
                Err(mob_not_served("second")),
                "{who}"
            );
        }
    }

    /// The caller a door goes on with is the caller's view in the mob the
    /// request is for: unchanged for the launch mob, and for another mob a
    /// view whose agent checks read a name as that mob's agent.
    #[test]
    fn the_caller_a_door_goes_on_with_sees_the_mob_the_request_is_for() {
        let launch = MobId::from("launch");
        let [_, (_, of_second), _] = judged_callers();
        assert!(
            !of_second.can_view_agent("alice"),
            "the allow selects the second mob's agents only"
        );
        let (_, unchanged) =
            judge_named_mob(&NamedMob::default(), Some(&launch), of_second.clone())
                .expect("the launch mob");
        assert!(!unchanged.can_view_agent("alice"));
        let (addressed, in_second) = judge_named_mob(
            &NamedMob::from_query(Some("second".to_string())),
            Some(&launch),
            of_second,
        )
        .expect("the allow selects the mob");
        assert_eq!(addressed, AddressedMob::Other(MobId::from("second")));
        assert!(
            in_second.can_view_agent("alice"),
            "the view handed back reads the name as the second mob's agent"
        );
    }

    /// The stdio dispatch answers its host with the id the request named
    /// and the host's default mob; a door that has callers answers the
    /// same refusal with the requested id only, for every reason a mob does
    /// not resolve.
    #[test]
    fn a_mob_that_did_not_resolve_is_answered_by_the_door_it_was_asked_on() {
        let not_hosted = MobResolveError::NotHosted {
            requested: MobId::from("b"),
            default_mob: MobId::from("a"),
        };
        let to_the_host = mob_resolve_refusal(&not_hosted);
        assert_eq!(
            to_the_host.data,
            Some(json!({
                "kind": "foreign_mob_target",
                "requested_mob_id": "b",
                "gateway_mob_id": "a",
            }))
        );
        assert_eq!(
            to_the_host.message,
            "request targets mob 'b' but this gateway serves 'a'"
        );
        let to_a_caller = mob_refusal_for_a_caller(&not_hosted);
        assert_eq!(to_a_caller.code, to_the_host.code);
        assert_eq!(
            to_a_caller.data,
            Some(json!({
                "kind": "foreign_mob_target",
                "requested_mob_id": "b",
            }))
        );
        assert_eq!(
            to_a_caller.message,
            "request targets mob 'b', which this gateway does not serve to the caller"
        );
    }
}
