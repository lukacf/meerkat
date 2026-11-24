//! The `mobs` selector of a console request.
//!
//! Three of the console's reads are over values of the host, not of one
//! mob: the experience, a timeline read that names no identity, and the
//! timeline stream that names none. Such a read answers for the one mob its
//! request is served for, unless the request selects every mob its caller
//! may address: `mobs=*`, the word an access rule's own `mobs` has for
//! every mob.
//!
//! The lists of the host that name members take the selector too. Such a
//! list is one list of the host whatever mob a request names: without the
//! selector it answers, of the entries members made, the launch mob's
//! members' alone, the list a console of one mob was answered, and with
//! every mob selected the entries of each mob the caller may address.
//!
//! The selector is a parameter of its own, not a value of `mob_id`: the
//! judgement of the one mob a request names
//! ([`crate::rpc::mob_addressing::judge_named_mob`]) stays a judgement of
//! one id. It has one value, it never stands beside a `mob_id`, and it is a
//! parameter of no other read: a request that carries it otherwise is one
//! the door cannot read, refused by its shape alone, before anything of a
//! mob is judged, asked or read.

use axum::Json;
use axum::http::{StatusCode, Uri};
use meerkat_mob::MobId;
use serde_json::{Value, json};

use crate::access::AccessView;
use crate::console_aggregator::ConsoleFrame;
use crate::rpc::JsonRpcError;

/// The name of the selector: a query parameter of a route, a top-level
/// member of a method's params.
const MOBS: &str = "mobs";

/// The selector's one value: every mob the caller may address.
const EVERY_MOB: &str = "*";

/// The method of the request doors that takes the selector.
const QUERY_TIMELINE_METHOD: &str = "meta/console/query_timeline";

/// The lists of the host that name members, and the console audit read:
/// each takes the selector too. Such a list is one list of the host
/// whatever mob a request names; without the selector it answers, of the
/// entries members made, the launch mob's members' alone. The decision
/// list is not among them: a decision names no member and no mob. Sorted,
/// one name per line.
const HOST_LIST_METHODS: &[&str] = &[
    "meta/console/attention",
    "meta/console/audit",
    "meta/gating/audit",
    "meta/gating/pending",
    "meta/gating/questions",
];

/// The console methods of the host: a method on this list answers from
/// values of the host and takes no mob as a selector of what it answers.
/// A mob named beside one is judged like any other id (a mob the host does
/// not serve to the caller is refused) and selects nothing: the request is
/// then served as one that names no mob, and the named mob is not
/// resolved, so it loads nothing. The door that takes a request's mob
/// reads this list.
///
/// The list is closed. It names the access administration and its
/// preview, the blob upload, the caller's own attention entries and their
/// dismissal, the console audit read, who the caller is, the decision list
/// and its rendering, and the host's one gating state (its pending
/// entries, its audit, its decision and its open question sets). The
/// lists among them
/// ([`HOST_LIST_METHODS`]) answer, of the entries members made, the launch
/// mob's members' alone unless the request selects every mob. A method
/// joins it only with the row that asks a request that names another mob
/// to be answered as one that names none. Sorted, one name per line.
pub(super) const CONSOLE_METHODS_OF_THE_HOST: &[&str] = &[
    "meta/access/enable",
    "meta/access/get",
    "meta/access/groups/delete",
    "meta/access/groups/set",
    "meta/access/preview",
    "meta/access/rules/delete",
    "meta/access/rules/upsert",
    "meta/access/set",
    "meta/access/status",
    "meta/blob/upload",
    "meta/console/attention",
    "meta/console/attention/dismiss",
    "meta/console/audit",
    "meta/console/whoami",
    "meta/decisions/list",
    "meta/decisions/rendering",
    "meta/gating/audit",
    "meta/gating/decide",
    "meta/gating/pending",
    "meta/gating/questions",
];

/// Whether `method` is a console method of the host
/// ([`CONSOLE_METHODS_OF_THE_HOST`]).
pub(super) fn console_method_is_of_the_host(method: &str) -> bool {
    CONSOLE_METHODS_OF_THE_HOST.binary_search(&method).is_ok()
}

/// `mob` as a record of a host whose default mob is `default_mob` stores
/// it: none for the default mob, which a record may also name by its id.
fn stored_form<'mob>(default_mob: Option<&MobId>, mob: Option<&'mob MobId>) -> Option<&'mob MobId> {
    mob.filter(|mob| Some(*mob) != default_mob)
}

/// Which mobs a read over the host's values answers for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MobsSelector {
    /// The request carries no selector: the read answers for the one mob
    /// the request is served for (the launch mob when it names none).
    #[default]
    ServedMob,
    /// `mobs=*`: the read answers for every mob its caller may address.
    EveryAddressable,
}

/// Whether the selector is a parameter of a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MobsParameter {
    /// The read is over the host's values and takes the selector.
    Taken,
    /// The read would take the selector, but its request names an
    /// identity: a name in one mob, so the read is of one mob's member.
    NamesAnIdentity,
    /// The selector is no parameter of the read.
    NotTaken,
}

/// What is wrong with the `mobs` of a request. Every answer is built from
/// the request's shape alone: it names no mob and reads none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MobsSelectorRefusal {
    /// The read does not take the selector.
    NotAParameter,
    /// The request names an identity beside the selector.
    BesideAnIdentity,
    /// The request names a mob beside the selector.
    BesideAMobId,
    /// The selector's value is not its one value.
    NotEveryMob,
}

impl MobsSelectorRefusal {
    /// What is wrong, in words.
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::NotAParameter => "mobs is not a parameter of this request",
            Self::BesideAnIdentity => {
                "mobs cannot be combined with identity: an identity is a name in one mob"
            }
            Self::BesideAMobId => "mobs cannot be combined with mob_id",
            Self::NotEveryMob => "mobs must be \"*\": every mob the caller may address",
        }
    }

    /// The refusal as a request door answers it: invalid params.
    pub(super) fn json_rpc_error(self) -> JsonRpcError {
        JsonRpcError {
            code: -32602,
            message: format!("Invalid params: {}", self.message()),
            data: None,
        }
    }

    /// The refusal as a route answers it: 400 `invalid_request`.
    pub(super) fn over_http(self) -> (StatusCode, Json<Value>) {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_request", "message": self.message() })),
        )
    }
}

impl MobsSelector {
    /// The one reading of a request's selector, from its shape: whether
    /// the request carries one (`selects_every_mob`: whether it has the
    /// selector's one value), whether the read takes it, and whether the
    /// request names a mob beside it.
    fn read(
        carried: Option<bool>,
        parameter: MobsParameter,
        names_a_mob: bool,
    ) -> Result<Self, MobsSelectorRefusal> {
        let Some(selects_every_mob) = carried else {
            return Ok(Self::ServedMob);
        };
        match parameter {
            MobsParameter::NotTaken => return Err(MobsSelectorRefusal::NotAParameter),
            MobsParameter::NamesAnIdentity => return Err(MobsSelectorRefusal::BesideAnIdentity),
            MobsParameter::Taken => {}
        }
        if names_a_mob {
            return Err(MobsSelectorRefusal::BesideAMobId);
        }
        if !selects_every_mob {
            return Err(MobsSelectorRefusal::NotEveryMob);
        }
        Ok(Self::EveryAddressable)
    }

    /// The selector of a route's request, read from its query. A query
    /// that carries `mobs` more than once does not have its one value.
    pub(super) fn in_query(
        uri: &Uri,
        parameter: MobsParameter,
    ) -> Result<Self, MobsSelectorRefusal> {
        let mut carried = None;
        let mut names_a_mob = false;
        for (name, value) in form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
            if name == MOBS {
                carried = Some(carried.is_none() && value == EVERY_MOB);
            } else if name == "mob_id" {
                names_a_mob = true;
            }
        }
        Self::read(carried, parameter, names_a_mob)
    }

    /// The selector of a request door's request of `method`, read from its
    /// params and taken off them, so the method's own parse never sees it.
    /// An absent or `null` `mobs` selects nothing. It is read on every
    /// `meta/*` method, the methods whose `mob_id` the door reads, and is a
    /// parameter of the timeline query that names no identity and of the
    /// host's lists that name members ([`HOST_LIST_METHODS`]). A method of
    /// any other family keeps its params as they came, and so does a
    /// method of the announcement family: `mobs` is a member of that
    /// family's own requests (the mobs a subscription or a declaration is
    /// for), which the door judges with the caller as the family names
    /// them.
    pub(super) fn take_from_params(
        method: &str,
        params: &mut Value,
    ) -> Result<Self, MobsSelectorRefusal> {
        if !method.starts_with("meta/")
            || crate::mob_announcements::rpc::is_announcement_method(method)
        {
            return Ok(Self::ServedMob);
        }
        let Some(object) = params.as_object_mut() else {
            return Ok(Self::ServedMob);
        };
        let carried = match object.remove(MOBS) {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_str() == Some(EVERY_MOB)),
        };
        let named = |member: &str| object.get(member).is_some_and(|value| !value.is_null());
        let parameter = if HOST_LIST_METHODS.contains(&method) {
            MobsParameter::Taken
        } else if method != QUERY_TIMELINE_METHOD {
            MobsParameter::NotTaken
        } else if named("identity") {
            MobsParameter::NamesAnIdentity
        } else {
            MobsParameter::Taken
        };
        Self::read(carried, parameter, named("mob_id"))
    }

    /// The selector's one judgement: whether a read with it answers for
    /// the mob `mob`, to the caller whose view is `caller`, on a request
    /// served for the mob `served`. Both mobs are named as a stored record
    /// names its mob: none for the host's default mob.
    ///
    /// Without the selector the read answers for the mob its request is
    /// served for and for no other. With every mob selected it answers for
    /// the default mob, which every caller the door let in addresses, and
    /// for each other mob the caller may address
    /// ([`AccessView::may_address_other_mob`]); a caller the door has no
    /// view of addresses the default mob only. The question reads the
    /// caller's view alone: no mob is resolved, loaded or looked up for it.
    /// What the caller may see in a mob it may address is judged afterwards,
    /// on the caller's view in that mob.
    pub(super) fn answers_for(
        self,
        caller: Option<&AccessView>,
        served: Option<&MobId>,
        mob: Option<&MobId>,
    ) -> bool {
        match self {
            Self::ServedMob => mob == served,
            Self::EveryAddressable => {
                mob.is_none_or(|mob| caller.is_some_and(|caller| caller.may_address_other_mob(mob)))
            }
        }
    }

    /// Whether a timeline read with this selector answers `frame`
    /// ([`Self::answers_for`] of the frame's mob), on a host whose default
    /// mob is `default_mob`: a frame that names that mob by its id is the
    /// default mob's, like one that names no mob. An update marker carries
    /// a second frame: the read answers the marker only when it answers
    /// for both frames' mobs.
    pub(super) fn answers_frame(
        self,
        caller: Option<&AccessView>,
        default_mob: Option<&MobId>,
        served: Option<&MobId>,
        frame: &ConsoleFrame,
    ) -> bool {
        if !self.answers_for(
            caller,
            served,
            stored_form(default_mob, frame.mob_id.as_ref()),
        ) {
            return false;
        }
        if frame.kind != "frame_updated" {
            return true;
        }
        let updated_mob = frame
            .payload
            .get("frame")
            .and_then(|updated| updated.get("mob_id"))
            .and_then(Value::as_str)
            .map(MobId::from);
        self.answers_for(
            caller,
            served,
            stored_form(default_mob, updated_mob.as_ref()),
        )
    }

    /// Whether a list of the host read with this selector answers an entry
    /// whose member is of the mob `mob` (none: an entry that names no mob),
    /// on a host whose default mob is `default_mob`
    /// ([`Self::answers_for`]). The host's lists are one list whatever mob a
    /// request names, so a mob named beside the method selects nothing:
    /// without the selector the list answers the default mob's entries and
    /// no other mob's, the list a console of one mob was answered; with
    /// every mob selected, the entries of each mob the caller may address.
    /// An entry that names the default mob by its id is the default mob's.
    pub(super) fn answers_host_entry(
        self,
        caller: Option<&AccessView>,
        default_mob: Option<&MobId>,
        mob: Option<&MobId>,
    ) -> bool {
        self.answers_for(caller, None, stored_form(default_mob, mob))
    }

    /// The mobs of `hosted` a read with this selector lists to `caller`,
    /// on a request served for the launch mob: `hosted` are the mobs the
    /// host serves beside its launch mob. A mob is listed when the read
    /// answers for it ([`Self::answers_for`]): one evaluation of the
    /// caller's view per mob, and nothing of a mob is loaded or asked. A
    /// mob that is not listed is left out whole, so the list and every
    /// count taken from it are the ones of a host that does not have that
    /// mob.
    pub(super) fn listed<'mob>(
        self,
        caller: Option<&AccessView>,
        hosted: impl IntoIterator<Item = &'mob MobId>,
    ) -> Vec<&'mob MobId> {
        hosted
            .into_iter()
            .filter(|mob| self.answers_for(caller, None, Some(mob)))
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::access::{
        ACTION_AGENT_VIEW, AccessControlConfig, AccessController, AccessEffect, AccessRule,
    };

    const ADMIN: &str = "root@example.test";
    const WITHOUT_RULES: &str = "nobody@example.test";
    const OF_THE_SECOND_MOB: &str = "second@example.test";
    const OF_EVERY_MOB: &str = "every@example.test";
    const ALLOWED_AND_DENIED: &str = "both@example.test";

    /// An enforced access configuration about a second mob `second`: one
    /// administrator; a person whose allow selects the second mob; a person
    /// whose allow selects every mob; a person whose allow selects the
    /// second mob and whose deny takes every action in it away; and an
    /// allow that names no mob for everyone (the launch mob's agents).
    fn enforced(second: &MobId) -> AccessController {
        let access = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![
                AccessRule {
                    id: "sees-the-launch-mob".to_string(),
                    subjects: vec!["*".to_string()],
                    actions: vec![ACTION_AGENT_VIEW.to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "sees-the-second-mob".to_string(),
                    subjects: vec![
                        OF_THE_SECOND_MOB.to_string(),
                        ALLOWED_AND_DENIED.to_string(),
                    ],
                    actions: vec![ACTION_AGENT_VIEW.to_string()],
                    mobs: vec![second.as_str().to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "sees-every-mob".to_string(),
                    subjects: vec![OF_EVERY_MOB.to_string()],
                    actions: vec![ACTION_AGENT_VIEW.to_string()],
                    mobs: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "nothing-in-the-second-mob".to_string(),
                    effect: AccessEffect::Deny,
                    subjects: vec![ALLOWED_AND_DENIED.to_string()],
                    actions: vec!["*".to_string()],
                    mobs: vec![second.as_str().to_string()],
                    ..AccessRule::default()
                },
            ],
            ..AccessControlConfig::default()
        })
        .expect("access configuration");
        // A host tells its controller which mob is its default mob.
        access.bind_default_mob(&crate::member_key::DefaultMob::new(MobId::from("launch")));
        access
    }

    fn uri(query: &str) -> Uri {
        format!("/console/timeline?{query}").parse().expect("uri")
    }

    /// The selector's reading from a query: absent selects nothing on any
    /// read; `*` selects every mob on a read that takes it; any other
    /// value, a repeated selector, a `mob_id` beside it, an identity beside
    /// it and a read that does not take it are each refused for what is
    /// wrong.
    #[test]
    fn the_selector_is_read_from_a_query_by_its_shape() {
        use MobsParameter::{NamesAnIdentity, NotTaken, Taken};
        use MobsSelectorRefusal::{BesideAMobId, BesideAnIdentity, NotAParameter, NotEveryMob};
        for parameter in [Taken, NamesAnIdentity, NotTaken] {
            for query in ["", "limit=5", "mob_id=second", "xmobs=*"] {
                assert_eq!(
                    MobsSelector::in_query(&uri(query), parameter),
                    Ok(MobsSelector::ServedMob),
                    "{query} on {parameter:?}"
                );
            }
        }
        for (query, parameter, read) in [
            ("mobs=*", Taken, Ok(MobsSelector::EveryAddressable)),
            (
                "limit=5&mobs=%2A",
                Taken,
                Ok(MobsSelector::EveryAddressable),
            ),
            ("mobs=", Taken, Err(NotEveryMob)),
            ("mobs=second", Taken, Err(NotEveryMob)),
            ("mobs=**", Taken, Err(NotEveryMob)),
            ("mobs=*&mobs=*", Taken, Err(NotEveryMob)),
            ("mobs=second&mobs=*", Taken, Err(NotEveryMob)),
            ("mobs=*&mob_id=second", Taken, Err(BesideAMobId)),
            ("mob_id=&mobs=*", Taken, Err(BesideAMobId)),
            ("mobs=second&mob_id=second", Taken, Err(BesideAMobId)),
            ("mobs=*", NamesAnIdentity, Err(BesideAnIdentity)),
            (
                "mobs=*&mob_id=second",
                NamesAnIdentity,
                Err(BesideAnIdentity),
            ),
            ("mobs=*", NotTaken, Err(NotAParameter)),
            ("mobs=", NotTaken, Err(NotAParameter)),
            ("mobs=*&mob_id=second", NotTaken, Err(NotAParameter)),
        ] {
            assert_eq!(
                MobsSelector::in_query(&uri(query), parameter),
                read,
                "{query} on {parameter:?}"
            );
        }
    }

    /// The selector's reading from a method's params: it is taken off the
    /// params of every `meta/*` method, `null` like absent selects nothing,
    /// and only the timeline query that names no identity takes it. A
    /// method of another family keeps its params.
    #[test]
    fn the_selector_is_taken_off_a_methods_params() {
        use MobsSelectorRefusal::{BesideAMobId, BesideAnIdentity, NotAParameter, NotEveryMob};
        for (method, params, read, left) in [
            (
                QUERY_TIMELINE_METHOD,
                json!({ "limit": 5 }),
                Ok(MobsSelector::ServedMob),
                json!({ "limit": 5 }),
            ),
            (
                QUERY_TIMELINE_METHOD,
                json!({ "mobs": null, "limit": 5 }),
                Ok(MobsSelector::ServedMob),
                json!({ "limit": 5 }),
            ),
            (
                QUERY_TIMELINE_METHOD,
                json!({ "mobs": "*", "limit": 5, "identity": null }),
                Ok(MobsSelector::EveryAddressable),
                json!({ "limit": 5, "identity": null }),
            ),
            (
                QUERY_TIMELINE_METHOD,
                json!({ "mobs": ["*"] }),
                Err(NotEveryMob),
                json!({}),
            ),
            (
                QUERY_TIMELINE_METHOD,
                json!({ "mobs": "second" }),
                Err(NotEveryMob),
                json!({}),
            ),
            (
                QUERY_TIMELINE_METHOD,
                json!({ "mobs": "*", "mob_id": "second" }),
                Err(BesideAMobId),
                json!({ "mob_id": "second" }),
            ),
            (
                QUERY_TIMELINE_METHOD,
                json!({ "mobs": "*", "identity": "alice" }),
                Err(BesideAnIdentity),
                json!({ "identity": "alice" }),
            ),
            (
                "meta/status",
                json!({ "mobs": null }),
                Ok(MobsSelector::ServedMob),
                json!({}),
            ),
            (
                "meta/status",
                json!({ "mobs": "*" }),
                Err(NotAParameter),
                json!({}),
            ),
            (
                "meta/gating/decide",
                json!({ "mobs": 7 }),
                Err(NotAParameter),
                json!({}),
            ),
            (
                "meta/gating/pending",
                json!({ "mobs": 7 }),
                Err(NotEveryMob),
                json!({}),
            ),
            (
                "meta/gating/pending",
                json!({ "mobs": "*", "limit": 5 }),
                Ok(MobsSelector::EveryAddressable),
                json!({ "limit": 5 }),
            ),
            (
                "meta/gating/audit",
                json!({ "mobs": "*", "mob_id": "second" }),
                Err(BesideAMobId),
                json!({ "mob_id": "second" }),
            ),
            (
                "meta/console/attention",
                json!({ "mobs": "*" }),
                Ok(MobsSelector::EveryAddressable),
                json!({}),
            ),
            (
                "meta/decisions/list",
                json!({ "mobs": "*", "identity": "alice" }),
                Err(NotAParameter),
                json!({ "identity": "alice" }),
            ),
            (
                "meta/console/audit",
                json!({ "mobs": "*", "limit": 5 }),
                Ok(MobsSelector::EveryAddressable),
                json!({ "limit": 5 }),
            ),
            (
                "meta/gating/questions",
                json!({ "mobs": "*" }),
                Ok(MobsSelector::EveryAddressable),
                json!({}),
            ),
            (
                "module/own_method",
                json!({ "mobs": "*" }),
                Ok(MobsSelector::ServedMob),
                json!({ "mobs": "*" }),
            ),
            (
                "meta/status",
                json!(["mobs"]),
                Ok(MobsSelector::ServedMob),
                json!(["mobs"]),
            ),
        ] {
            let mut taken = params.clone();
            assert_eq!(
                MobsSelector::take_from_params(method, &mut taken),
                read,
                "{method} {params}"
            );
            assert_eq!(taken, left, "{method} {params}: what is left of the params");
        }
    }

    /// A request of the announcement family keeps its params as they came.
    /// `mobs` is a member of that family's own requests (the mobs a
    /// subscription or a declaration is for): it is not the selector, the
    /// door does not read it and does not take it off, whatever its value
    /// and whatever stands beside it.
    #[test]
    fn a_member_mobs_of_the_announcement_family_is_not_the_selector() {
        for (method, params) in [
            (
                "meta/mob_announcements/subscriptions/put",
                json!({ "mobs": ["second"] }),
            ),
            (
                "meta/mob_announcements/subscriptions/put",
                json!({ "mobs": "*" }),
            ),
            (
                "meta/mob_announcements/declarations/apply",
                json!({ "mobs": ["second"], "mob_id": "second" }),
            ),
            (
                "meta/mob_announcements/subscriptions/list",
                json!({ "mobs": 7, "identity": "alice" }),
            ),
        ] {
            let mut taken = params.clone();
            assert_eq!(
                MobsSelector::take_from_params(method, &mut taken),
                Ok(MobsSelector::ServedMob),
                "{method} {params}"
            );
            assert_eq!(taken, params, "{method}: its params are left as they came");
        }
    }

    /// A list of the host that takes the selector is a method of the host:
    /// a mob named beside it selects nothing, and `mobs` is how a request
    /// asks it for more than the launch mob's members' entries. The host's
    /// methods are sorted and name a method once.
    #[test]
    fn a_list_that_takes_the_selector_is_a_method_of_the_host() {
        assert!(
            CONSOLE_METHODS_OF_THE_HOST
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "sorted, each name once"
        );
        for method in HOST_LIST_METHODS {
            assert!(console_method_is_of_the_host(method), "{method}");
        }
        for method in [QUERY_TIMELINE_METHOD, "meta/status", "meta/list_members"] {
            assert!(!console_method_is_of_the_host(method), "{method}");
        }
    }

    /// A refusal is built from the request's shape alone: no answer of it
    /// carries a member a mob's id could be read from.
    #[test]
    fn a_refusal_of_the_selector_names_no_mob() {
        use MobsSelectorRefusal::{BesideAMobId, BesideAnIdentity, NotAParameter, NotEveryMob};
        for refusal in [NotAParameter, BesideAnIdentity, BesideAMobId, NotEveryMob] {
            let rpc = refusal.json_rpc_error();
            assert_eq!(rpc.code, -32602);
            assert!(rpc.data.is_none(), "{refusal:?}");
            assert!(rpc.message.contains("mobs"), "{refusal:?}");
            let (status, Json(body)) = refusal.over_http();
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(
                body,
                json!({ "error": "invalid_request", "message": refusal.message() }),
                "{refusal:?}"
            );
        }
    }

    /// The one judgement, through an enforced access view. Without the
    /// selector a read answers for the mob its request is served for and
    /// for no other, whoever asks. With every mob selected it answers for
    /// the launch mob to everyone, and for the second mob to an
    /// administrator and to a person an allow rule selects that mob for (a
    /// deny neither gives nor takes the address), and not to a person no
    /// rule selects it for, nor to a caller the door has no view of.
    #[test]
    fn a_read_answers_for_the_mobs_its_selector_and_its_caller_give() {
        let second = MobId::from("second");
        let third = MobId::from("third");
        let access = enforced(&second);
        let view = |subject: &str| access.view_for_subject(Some(subject));
        for subject in [ADMIN, WITHOUT_RULES, OF_THE_SECOND_MOB] {
            let caller = view(subject);
            let served_mob = MobsSelector::ServedMob;
            assert!(
                served_mob.answers_for(Some(&caller), None, None),
                "{subject}"
            );
            assert!(
                !served_mob.answers_for(Some(&caller), None, Some(&second)),
                "{subject}"
            );
            assert!(
                served_mob.answers_for(Some(&caller), Some(&second), Some(&second)),
                "{subject}"
            );
            assert!(
                !served_mob.answers_for(Some(&caller), Some(&second), None),
                "{subject}"
            );
            assert!(
                !served_mob.answers_for(Some(&caller), Some(&second), Some(&third)),
                "{subject}"
            );
        }
        let every = MobsSelector::EveryAddressable;
        for (subject, of_the_second, of_the_third) in [
            (ADMIN, true, true),
            (WITHOUT_RULES, false, false),
            (OF_THE_SECOND_MOB, true, false),
            (OF_EVERY_MOB, true, true),
            (ALLOWED_AND_DENIED, true, false),
        ] {
            let caller = view(subject);
            assert!(every.answers_for(Some(&caller), None, None), "{subject}");
            assert_eq!(
                every.answers_for(Some(&caller), None, Some(&second)),
                of_the_second,
                "{subject}, the second mob"
            );
            assert_eq!(
                every.answers_for(Some(&caller), None, Some(&third)),
                of_the_third,
                "{subject}, a third mob"
            );
            // The judgement is the one a request that names the mob gets.
            assert_eq!(
                every.answers_for(Some(&caller), None, Some(&second)),
                caller.may_address_other_mob(&second),
                "{subject}"
            );
        }
        assert!(every.answers_for(None, None, None));
        assert!(!every.answers_for(None, None, Some(&second)));
        // Access that enforces nothing: every caller addresses every mob.
        let open = AccessController::disabled().view_for_subject(None);
        assert!(every.answers_for(Some(&open), None, Some(&second)));
        assert!(!MobsSelector::ServedMob.answers_for(Some(&open), None, Some(&second)));
    }

    /// The list of the host's other mobs, through an enforced access view
    /// over a host that serves a second mob. A person no rule selects the
    /// second mob for is listed exactly what a host without that mob lists
    /// (the list and its count are equal), a person a rule selects it for
    /// and an administrator are listed it, and without the selector nobody
    /// is listed another mob.
    #[test]
    fn the_listed_mobs_are_the_ones_the_caller_may_address() {
        let second = MobId::from("second");
        let access = enforced(&second);
        let host_of_two = [second.clone()];
        let host_of_one: [MobId; 0] = [];
        let every = MobsSelector::EveryAddressable;
        for (subject, listed) in [
            (ADMIN, vec![&second]),
            (OF_THE_SECOND_MOB, vec![&second]),
            (OF_EVERY_MOB, vec![&second]),
            (ALLOWED_AND_DENIED, vec![&second]),
            (WITHOUT_RULES, Vec::new()),
        ] {
            let caller = access.view_for_subject(Some(subject));
            assert_eq!(
                every.listed(Some(&caller), &host_of_two),
                listed,
                "{subject}"
            );
            assert!(
                MobsSelector::ServedMob
                    .listed(Some(&caller), &host_of_two)
                    .is_empty(),
                "{subject}, without the selector"
            );
        }
        let without_rules = access.view_for_subject(Some(WITHOUT_RULES));
        let with_the_second_mob = every.listed(Some(&without_rules), &host_of_two);
        let without_it = every.listed(Some(&without_rules), &host_of_one);
        assert_eq!(with_the_second_mob, without_it);
        assert_eq!(with_the_second_mob.len(), without_it.len());
        assert!(every.listed(None, &host_of_two).is_empty());
    }

    /// A host list judges each entry with the caller's view in the entry's
    /// own mob, and never asks whether the caller may address that mob. The
    /// two agree: under enforced access a caller who may view an agent of
    /// another mob is a caller who may address that mob, so an entry's own
    /// view never shows a caller an entry of a mob it may not address. The
    /// launch mob's agent of the same name is another agent: seeing it says
    /// nothing about the other mob.
    #[test]
    fn a_caller_who_may_view_an_agent_of_another_mob_may_address_that_mob() {
        let second = MobId::from("second");
        let third = MobId::from("third");
        let access = enforced(&second);
        for (subject, views_the_second_mobs_agent) in [
            (ADMIN, true),
            (WITHOUT_RULES, false),
            (OF_THE_SECOND_MOB, true),
            (OF_EVERY_MOB, true),
            (ALLOWED_AND_DENIED, false),
        ] {
            let caller = access.view_for_subject(Some(subject));
            assert!(
                caller.in_stored_mob(None).can_view_agent("alice"),
                "{subject}: the launch mob's agent"
            );
            assert_eq!(
                caller.in_stored_mob(Some(&second)).can_view_agent("alice"),
                views_the_second_mobs_agent,
                "{subject}: the second mob's agent"
            );
            for mob in [&second, &third] {
                let views = caller.in_stored_mob(Some(mob)).can_view_agent("alice");
                assert!(
                    !views || caller.may_address_other_mob(mob),
                    "{subject} views an agent of {} and may not address it",
                    mob.as_str()
                );
            }
        }
    }

    fn frame(mob_id: Option<&str>, kind: &str, payload: Value) -> ConsoleFrame {
        serde_json::from_value(json!({
            "id": "frame",
            "cursor": "console:1",
            "dedupe_key": "frame",
            "timestamp_ms": 1,
            "runtime_key": "runtime",
            "identity": "alice",
            "mob_id": mob_id,
            "kind": kind,
            "status": "completed",
            "frame_version": 1,
            "payload": payload,
            "source": { "kind": "session_history" },
        }))
        .expect("a stored frame")
    }

    /// A timeline read answers a frame by the frame's mob, and an update
    /// marker only when it answers for the mob of the frame it carries too.
    #[test]
    fn a_timeline_read_answers_a_frame_by_its_mob() {
        let second = MobId::from("second");
        let access = enforced(&second);
        let admin = access.view_for_subject(Some(ADMIN));
        let without_rules = access.view_for_subject(Some(WITHOUT_RULES));
        let of_the_launch_mob = frame(None, "text_complete", json!({}));
        let of_the_second_mob = frame(Some("second"), "text_complete", json!({}));
        let marker_of_the_second_mobs_frame =
            frame(None, "frame_updated", json!({ "frame": of_the_second_mob }));
        let marker_of_the_launch_mobs_frame =
            frame(None, "frame_updated", json!({ "frame": of_the_launch_mob }));
        let served_mob = MobsSelector::ServedMob;
        let every = MobsSelector::EveryAddressable;
        for (selector, caller, frame, answered) in [
            (served_mob, &admin, &of_the_launch_mob, true),
            (served_mob, &admin, &of_the_second_mob, false),
            (served_mob, &admin, &marker_of_the_launch_mobs_frame, true),
            (served_mob, &admin, &marker_of_the_second_mobs_frame, false),
            (every, &admin, &of_the_second_mob, true),
            (every, &admin, &marker_of_the_second_mobs_frame, true),
            (every, &without_rules, &of_the_launch_mob, true),
            (every, &without_rules, &of_the_second_mob, false),
            (
                every,
                &without_rules,
                &marker_of_the_launch_mobs_frame,
                true,
            ),
            (
                every,
                &without_rules,
                &marker_of_the_second_mobs_frame,
                false,
            ),
        ] {
            assert_eq!(
                selector.answers_frame(Some(caller), None, None, frame),
                answered,
                "{selector:?}, a {} of mob {:?}",
                frame.kind,
                frame.mob_id
            );
        }
        assert!(served_mob.answers_frame(Some(&admin), None, Some(&second), &of_the_second_mob));
        assert!(!served_mob.answers_frame(Some(&admin), None, Some(&second), &of_the_launch_mob));
    }

    /// A record may name the host's default mob by its id: such a frame and
    /// such a list entry are the default mob's, like one that names no mob,
    /// for a read without the selector and for one with every mob selected,
    /// whoever asks. On a host whose default mob the door does not know,
    /// the id names a mob like any other.
    #[test]
    fn a_record_that_names_the_default_mob_by_its_id_is_the_default_mobs() {
        let launch = MobId::from("launch");
        let second = MobId::from("second");
        let access = enforced(&second);
        let without_rules = access.view_for_subject(Some(WITHOUT_RULES));
        let naming_the_launch_mob = frame(Some("launch"), "text_complete", json!({}));
        let marker_naming_it = frame(
            Some("launch"),
            "frame_updated",
            json!({ "frame": naming_the_launch_mob }),
        );
        for selector in [MobsSelector::ServedMob, MobsSelector::EveryAddressable] {
            for record in [&naming_the_launch_mob, &marker_naming_it] {
                assert!(
                    selector.answers_frame(Some(&without_rules), Some(&launch), None, record),
                    "{selector:?}, a {}",
                    record.kind
                );
                assert!(
                    !selector.answers_frame(Some(&without_rules), None, None, record),
                    "{selector:?}, a {} on a host whose default mob is not known",
                    record.kind
                );
            }
            assert!(selector.answers_host_entry(
                Some(&without_rules),
                Some(&launch),
                Some(&launch)
            ));
            assert!(selector.answers_host_entry(Some(&without_rules), Some(&launch), None));
        }
    }

    /// A list of the host answers an entry by the entry's mob and never by
    /// the mob a request names: without the selector, of the entries
    /// members made, the launch mob's members' alone, to an administrator
    /// too; with every mob selected the entries of each mob the caller may
    /// address.
    #[test]
    fn a_list_of_the_host_answers_an_entry_by_the_entrys_mob() {
        let launch = MobId::from("launch");
        let second = MobId::from("second");
        let access = enforced(&second);
        for (subject, of_the_second_with_every_mob) in [
            (ADMIN, true),
            (OF_THE_SECOND_MOB, true),
            (ALLOWED_AND_DENIED, true),
            (WITHOUT_RULES, false),
        ] {
            let caller = access.view_for_subject(Some(subject));
            let served_mob = MobsSelector::ServedMob;
            let every = MobsSelector::EveryAddressable;
            assert!(served_mob.answers_host_entry(Some(&caller), Some(&launch), None));
            assert!(
                !served_mob.answers_host_entry(Some(&caller), Some(&launch), Some(&second)),
                "{subject}: without the selector"
            );
            assert!(every.answers_host_entry(Some(&caller), Some(&launch), None));
            assert_eq!(
                every.answers_host_entry(Some(&caller), Some(&launch), Some(&second)),
                of_the_second_with_every_mob,
                "{subject}: with every mob selected"
            );
        }
        assert!(!MobsSelector::EveryAddressable.answers_host_entry(
            None,
            Some(&launch),
            Some(&second)
        ));
    }
}
