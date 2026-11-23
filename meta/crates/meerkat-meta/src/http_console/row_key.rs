//! The row key of a member, as a method of the host reads it.
//!
//! A console names a member by its row key: the bare name for a member of
//! the launch mob, and `<its own mob id>/<name>` for every other member. A
//! request to one mob never carries a row key: it names its mob with
//! `mob_id` and the member by the name that mob's door knows. A method of
//! the host takes no mob, so where it names a member it names it by the row
//! key, and the door reads the key here, once, before the method's handler.
//!
//! This door is told of the mobs its host's resolver serves. A key whose
//! first part is the id of such a mob, other than the launch mob, names a
//! member of that mob, unless the whole key is a name the launch mob's door
//! knows; every other key is a name in the launch mob, as it was before a
//! host served a second mob (a launch mob member's name may hold the
//! separator, and a member of a mob a member created is named so on the
//! launch mob's door). A whole key the launch mob's door knows names that
//! member, as [`crate::console_member_ref`] rules for the member of a
//! nested mob that a primary member's name spells.

use meerkat_mob::MobId;
use serde_json::Value;

use crate::access::AccessView;
use crate::mob_handle_runtime::MobRuntime;
use crate::unified_runtime::MobResolver;

/// The separator of a row key: the member's own mob, then its name there.
const SEPARATOR: char = '/';

/// The methods of the host that name a member by its row key, each with
/// the member of its params that holds the key. Sorted by method.
const HOST_METHODS_NAMING_A_MEMBER: &[(&str, &str)] = &[("meta/access/preview", "identity")];

/// The member a row key names: its own mob as a stored record names it
/// (none for the launch mob), and its name as that mob's door knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowKeyMember {
    pub(crate) mob: Option<MobId>,
    pub(crate) name: String,
}

impl RowKeyMember {
    /// A member of the launch mob named `name`.
    fn of_the_launch_mob(name: &str) -> Self {
        Self {
            mob: None,
            name: name.to_string(),
        }
    }
}

/// The launch mob's door, as the reader of a row key asks it whether a
/// string is a name it knows: the launch mob's runtime, and the caller's
/// view of the console, whose access records hold the launch mob's roster.
#[derive(Clone, Copy)]
pub(super) struct LaunchDoor<'a> {
    pub(super) runtime: &'a MobRuntime,
    pub(super) view: &'a AccessView,
}

impl LaunchDoor<'_> {
    /// Whether the launch mob's door knows `name` whole: a member of the
    /// launch mob, or a member of a mob nested under it by its composed
    /// name. The launch mob's roster is read into the view's access records
    /// first, so a member seated since their last read is known.
    async fn knows(&self, name: &str) -> bool {
        super::prime_access_cache_from_runtime(self.runtime, &self.view.controller()).await;
        self.view.in_stored_mob(None).knows_agent(name)
    }
}

/// The member the row key `row_key` names on the host of `resolver`
/// (none: a door that knows the launch mob alone). The question asked of
/// the resolver is whether it serves the id the key's first part names,
/// which loads nothing and resolves no mob. A key whose first part is such
/// a mob can be spelled by two members: that mob's member of the rest, and
/// a member the launch mob's door knows under the whole key. `launch` is
/// asked for the second, and only then; where it knows the name, the key
/// names that member.
pub(super) async fn member_of_row_key(
    row_key: &str,
    resolver: Option<&MobResolver>,
    launch: LaunchDoor<'_>,
) -> RowKeyMember {
    let Some(resolver) = resolver else {
        return RowKeyMember::of_the_launch_mob(row_key);
    };
    let Some((mob, name)) = row_key.split_once(SEPARATOR) else {
        return RowKeyMember::of_the_launch_mob(row_key);
    };
    if mob.is_empty() || name.is_empty() {
        return RowKeyMember::of_the_launch_mob(row_key);
    }
    let mob = MobId::from(mob);
    let launch_mob = crate::rpc::mob_addressing::launch_mob_of(resolver).await;
    if launch_mob.as_ref() == Some(&mob) || resolver.served_mob_id(Some(&mob)).await.is_err() {
        return RowKeyMember::of_the_launch_mob(row_key);
    }
    if launch.knows(row_key).await {
        return RowKeyMember::of_the_launch_mob(row_key);
    }
    RowKeyMember {
        mob: Some(mob),
        name: name.to_string(),
    }
}

/// The row key of the member `name` of the mob `mob`, a mob other than the
/// launch mob: the mob's id, the separator, and the name as that mob knows
/// it. A member of the launch mob is keyed by its bare name.
pub(super) fn row_key_of(mob: &MobId, name: &str) -> String {
    format!("{}{SEPARATOR}{name}", mob.as_str())
}

/// Key a member row of the mob `mob`, a mob other than the launch mob, as
/// a console reads rows: every name the row carries of its own member
/// (`identity`, `member_id`, `agent_id`, and the label `agent_identity`
/// where the row has one) becomes the row key, the members it names
/// (`wired_to`, `side_chat_of`) are named by their row keys, and the row
/// names the mob whose door it was read on (`mob_id`: `mob`). No name of
/// the row is left bare: a bare name is the launch mob's member of that
/// name. The mob the member sits in and its name there (`member_mob_id`,
/// `member_name`) stay as the door's document wrote them.
pub(super) fn key_member_row(row: &mut Value, mob: &MobId) {
    let keyed = |value: &mut Value| {
        if let Some(name) = value.as_str() {
            *value = Value::String(row_key_of(mob, name));
        }
    };
    for field in ["identity", "member_id", "agent_id", "side_chat_of"] {
        if let Some(value) = row.get_mut(field) {
            keyed(value);
        }
    }
    if let Some(label) = row.pointer_mut("/labels/agent_identity") {
        keyed(label);
    }
    if let Some(peers) = row.get_mut("wired_to").and_then(Value::as_array_mut) {
        peers.iter_mut().for_each(keyed);
    }
    if let Some(row) = row.as_object_mut() {
        row.insert(
            "mob_id".to_string(),
            Value::String(mob.as_str().to_string()),
        );
    }
}

/// The member a request of the host method `method` names by a row key in
/// `params`, read for the dispatch: none for a method that names no member
/// so, and for a request that names none.
pub(super) async fn host_method_member(
    method: &str,
    params: &Value,
    resolver: Option<&MobResolver>,
    launch: LaunchDoor<'_>,
) -> Option<RowKeyMember> {
    let (_, member) = HOST_METHODS_NAMING_A_MEMBER
        .iter()
        .find(|(listed, _)| *listed == method)?;
    let row_key = params.get(member).and_then(Value::as_str)?;
    Some(member_of_row_key(row_key, resolver, launch).await)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A member row of another mob is keyed whole: its own names, the
    /// members it names and its mob. A row that carries no label, no peers
    /// and no side chat source gains none, and fields that name no member
    /// are left as they are.
    #[test]
    fn a_member_row_of_another_mob_carries_no_bare_name() {
        let mob = MobId::from("research");
        let mut row = serde_json::json!({
            "identity": "reviewer",
            "member_id": "reviewer",
            "agent_id": "reviewer",
            "label": "Reviewer",
            "kind": "mob_agent",
            "role": "worker",
            "labels": { "agent_identity": "reviewer", "group": "desk" },
            "wired_to": ["analyst", "audit/singleton"],
            "side_chat_of": "analyst",
        });
        key_member_row(&mut row, &mob);
        assert_eq!(
            row,
            serde_json::json!({
                "identity": "research/reviewer",
                "member_id": "research/reviewer",
                "agent_id": "research/reviewer",
                "label": "Reviewer",
                "kind": "mob_agent",
                "role": "worker",
                "labels": { "agent_identity": "research/reviewer", "group": "desk" },
                "wired_to": ["research/analyst", "research/audit/singleton"],
                "side_chat_of": "research/analyst",
                "mob_id": "research",
            })
        );
        let mut plain = serde_json::json!({ "identity": "reviewer", "wired_to": [] });
        key_member_row(&mut plain, &mob);
        assert_eq!(
            plain,
            serde_json::json!({
                "identity": "research/reviewer",
                "wired_to": [],
                "mob_id": "research",
            })
        );
    }

    /// A runtime over one mob whose id starts with `tag`.
    async fn runtime(tag: &str) -> crate::UnifiedRuntime {
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"row-key-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            uuid::Uuid::new_v4().simple()
        ))
        .expect("mob definition");
        crate::UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await
            .expect("runtime builds")
    }

    /// A row key names a member of another mob only when its first part is
    /// the id of a mob the host serves beside its launch mob: then the
    /// rest is the member's name there, separators included. Every other
    /// key is a name in the launch mob: a bare name, a launch mob member's
    /// name that holds the separator, a key under the launch mob's own id,
    /// under an id no mob has, or with an empty part. Reading a key
    /// resolves no mob. The launch mob of this row has no member, so its
    /// door knows no key of the row whole; the row of a key it does know is
    /// at the door (`console_hosted_mobs_tests`).
    #[tokio::test]
    async fn a_row_key_names_a_member_of_a_mob_the_host_serves() {
        let launch = runtime("launch").await;
        let second = runtime("second").await;
        let launch_mob = launch.mob_id();
        let second_mob = second.mob_id();
        let resolver = launch
            .mob_resolver()
            .serving_a_second_mob_in_tests(&second.mob_resolver());
        let controller =
            crate::access::AccessController::new(crate::access::AccessControlConfig::default())
                .expect("access configuration");
        let view = controller.view_for_subject(None);
        let launch_door = LaunchDoor {
            runtime: launch.mob_runtime(),
            view: &view,
        };
        let of_the_launch_mob = |name: &str| RowKeyMember {
            mob: None,
            name: name.to_string(),
        };
        let of_the_second_mob = |name: &str| RowKeyMember {
            mob: Some(MobId::from(second_mob.as_str())),
            name: name.to_string(),
        };
        for (row_key, member) in [
            ("reviewer".to_string(), of_the_launch_mob("reviewer")),
            (
                "audit/singleton".to_string(),
                of_the_launch_mob("audit/singleton"),
            ),
            (
                format!("{second_mob}/reviewer"),
                of_the_second_mob("reviewer"),
            ),
            (
                format!("{second_mob}/audit/singleton"),
                of_the_second_mob("audit/singleton"),
            ),
            (
                format!("{launch_mob}/reviewer"),
                of_the_launch_mob(&format!("{launch_mob}/reviewer")),
            ),
            (
                "no-mob-has-this-id/reviewer".to_string(),
                of_the_launch_mob("no-mob-has-this-id/reviewer"),
            ),
            (
                format!("{second_mob}/"),
                of_the_launch_mob(&format!("{second_mob}/")),
            ),
            ("/reviewer".to_string(), of_the_launch_mob("/reviewer")),
        ] {
            assert_eq!(
                member_of_row_key(&row_key, Some(&resolver), launch_door).await,
                member,
                "{row_key}"
            );
            // A door without the host's resolver knows the launch mob alone.
            assert_eq!(
                member_of_row_key(&row_key, None, launch_door).await,
                of_the_launch_mob(&row_key),
                "{row_key}, without a resolver"
            );
        }
        assert!(
            resolver.resolved_for_in_tests().is_empty(),
            "reading a row key resolved a mob: {:?}",
            resolver.resolved_for_in_tests()
        );

        let preview = serde_json::json!({ "identity": format!("{second_mob}/reviewer") });
        assert_eq!(
            host_method_member(
                "meta/access/preview",
                &preview,
                Some(&resolver),
                launch_door
            )
            .await,
            Some(of_the_second_mob("reviewer"))
        );
        assert_eq!(
            host_method_member(
                "meta/access/preview",
                &serde_json::json!({}),
                Some(&resolver),
                launch_door
            )
            .await,
            None
        );
        assert_eq!(
            host_method_member("meta/access/get", &preview, Some(&resolver), launch_door).await,
            None
        );
        launch.shutdown().await;
        second.shutdown().await;
    }
}
