//! The one owner of the console identity of a member of a nested mob.
//!
//! A console names the members of its own mob (the primary mob) by their
//! bare names, as it always has. A member of any other mob the gateway holds
//! (a nested mob) is named by its mob and its name in one string,
//! `<mob id>/<name>`, in every identity field the console reads or writes:
//! rows, topology nodes, identity records, frames, sends and the access
//! attribute cache. Two members with one name in two mobs so never share an
//! identity, and a bare name never names a nested member.
//!
//! The form is unambiguous because a mob that seats a member has an id with
//! no `/`: the id is the first component of every member's comms name
//! `mob_id/role/member`, and `meerkat_core::MemberCommsName` refuses a
//! component outside `[A-Za-z_][A-Za-z0-9_-]*` (the spawn fails closed). So
//! the first `/` of a nested member's identity always ends its mob id, and
//! the name after it may hold any character, a `/` included. A mob id that
//! holds a `/` composes nothing ([`nested_member_identity`] answers `None`,
//! and the member is listed nowhere).
//!
//! Parsing is syntactic: whether the part before the first `/` names a mob
//! the gateway holds is the caller's question (a runtime's namespace is
//! stripped first, see the aggregator). A whole string that is a primary
//! member's name names that member, as before nested mobs were listed; a
//! nested member whose identity a primary member's name spells is hidden.
//!
//! No other code composes or splits the form; the console's own helper
//! (`meta/packages/console-core/src/member-ref.ts`) mirrors this pair over
//! the shared cases in `console_member_ref_cases.json`.

/// The separator between a nested mob's id and its member's name.
const SEPARATOR: char = '/';

/// The console identity of the member `name` of the nested mob `mob_id`:
/// `<mob id>/<name>`. `None` when either part is empty or the mob id holds
/// the separator (no mob that seats a member has such an id): the member
/// then has no console identity and is listed nowhere.
pub(crate) fn nested_member_identity(mob_id: &str, name: &str) -> Option<String> {
    if mob_id.is_empty() || name.is_empty() || mob_id.contains(SEPARATOR) {
        return None;
    }
    Some(format!("{mob_id}{SEPARATOR}{name}"))
}

/// The mob id and the member name a nested member's console identity holds,
/// split at its first `/`. `None` for a string with no `/`, or with nothing
/// before or after the first one: a bare name, which names a primary member.
pub(crate) fn parse_nested_member_identity(identity: &str) -> Option<(&str, &str)> {
    let (mob_id, name) = identity.split_once(SEPARATOR)?;
    (!mob_id.is_empty() && !name.is_empty()).then_some((mob_id, name))
}

/// The name in the nested mob `mob_id` of the member whose console identity
/// is `identity`: what the identity holds after that mob's id and the
/// separator. `None` for an identity that is not a member of that mob's:
/// a bare name, or one that begins with another mob's id.
pub(crate) fn nested_member_name<'a>(identity: &'a str, mob_id: &str) -> Option<&'a str> {
    parse_nested_member_identity(identity)
        .filter(|(mob, _)| *mob == mob_id)
        .map(|(_, name)| name)
}

/// The nested mob whose own entries the access records keep the member of
/// `identity` and `labels`, seen from the mob `own_mob`: the mob written on
/// the member (`source_mob_id`), when it is another mob than `own_mob` and
/// `identity` is the string the door gives a member of that mob (its mob
/// and its name there). None for a member of `own_mob`, and for a nested
/// mob's member named in another form, which is recorded among the agents
/// of `own_mob`. The walk that records the roster's rows, the roster that
/// decides them and the listing of identities ask this one test.
pub(crate) fn keyed_nested_mob<'a>(
    identity: &str,
    labels: &'a std::collections::BTreeMap<String, String>,
    own_mob: Option<&str>,
) -> Option<&'a str> {
    let mob_id = labels.get("source_mob_id")?;
    (own_mob != Some(mob_id.as_str()) && nested_member_name(identity, mob_id).is_some())
        .then_some(mob_id.as_str())
}

/// The member name to show for the console identity `identity` of a member
/// of the mob `mob_id`: the name without the mob for a nested member's
/// identity of that mob, else the identity as it is.
pub(crate) fn member_name_in_mob<'a>(identity: &'a str, mob_id: Option<&str>) -> &'a str {
    match (parse_nested_member_identity(identity), mob_id) {
        (Some((mob, name)), Some(mob_id)) if mob == mob_id => name,
        _ => identity,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// The cases the console's helper is checked against too.
    const CASES: &str = include_str!("console_member_ref_cases.json");

    #[test]
    fn the_shared_cases_compose_and_parse_as_written() {
        let cases: serde_json::Value = serde_json::from_str(CASES).expect("the cases are JSON");
        let compose = cases["compose"].as_array().expect("compose cases");
        assert!(!compose.is_empty());
        for case in compose {
            let composed = nested_member_identity(
                case["mob_id"].as_str().expect("mob_id"),
                case["name"].as_str().expect("name"),
            );
            assert_eq!(composed.as_deref(), case["identity"].as_str(), "{case}");
        }
        let parse = cases["parse"].as_array().expect("parse cases");
        assert!(!parse.is_empty());
        for case in parse {
            let parsed = parse_nested_member_identity(case["identity"].as_str().expect("identity"));
            assert_eq!(
                parsed,
                case["mob_id"].as_str().zip(case["name"].as_str()),
                "{case}"
            );
        }
    }

    /// What composes parses back to its parts, a name holding `/` included,
    /// and a mob id holding `/` composes nothing.
    #[test]
    fn a_composed_identity_parses_back_and_a_mob_id_with_a_slash_composes_nothing() {
        for (mob_id, name) in [("team", "reviewer"), ("team", "a/b"), ("t", "rt:x:0")] {
            let identity = nested_member_identity(mob_id, name).expect("composes");
            assert_eq!(
                parse_nested_member_identity(&identity),
                Some((mob_id, name))
            );
            assert_eq!(member_name_in_mob(&identity, Some(mob_id)), name);
            assert_eq!(member_name_in_mob(&identity, Some("other")), identity);
            assert_eq!(member_name_in_mob(&identity, None), identity);
        }
        assert_eq!(nested_member_identity("a/b", "reviewer"), None);
        assert_eq!(member_name_in_mob("reviewer", Some("team")), "reviewer");
    }
}
