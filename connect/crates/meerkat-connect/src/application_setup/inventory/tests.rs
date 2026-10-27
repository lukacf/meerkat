use super::*;
use serde_json::json;

fn input() -> ApplicationInventoryInput {
    serde_json::from_value(json!({
        "schema": 1,
        "mob_id": "household",
        "helper_identity": "helper:setup",
        "roster": [
            {"identity":"domain:reading", "profile":"research", "binding":{"kind":"session"},
                "labels":{"source":"original"}, "context":{"retain":"exactly"},
                "additional_instructions":["Keep the application selection"]},
            {"identity":"helper:setup", "profile":"research"},
            {"identity":"rt:domain:reading:4", "profile":"observer"}
        ]
    })).unwrap()
}

#[test]
fn declared_inventory_uses_native_lowering_without_changing_roster_or_selecting_originals() {
    let input = input();
    assert!(input.roster[0].binding.is_some());
    assert_eq!(serde_json::to_value(&input.roster[0].binding).unwrap(), json!({"kind":"session"}));
    assert_eq!(input.roster[0].context, Some(json!({"retain":"exactly"})));
    assert_eq!(input.roster[0].additional_instructions, vec!["Keep the application selection"]);
    let before = serde_json::to_value(&input).unwrap();
    let inventory = input.lower().unwrap();
    assert_eq!(inventory.realm_id, meerkat_core::mob_realm_id(&input.mob_id).unwrap().to_string());
    assert_eq!(inventory.helper_identity, "helper:setup");
    assert_eq!(inventory.members.len(), input.roster.len());
    for (actual, declared) in inventory.members.iter().zip(&input.roster) {
        assert_eq!(actual.identity, declared.identity.as_str());
        assert_eq!(actual.member.mob_id, input.mob_id);
        assert_eq!(actual.member.role, declared.profile.as_str());
        assert_eq!(actual.member.member, roster_member_id_for_identity(declared.identity.as_str()).to_string());
        actual.member.comms_name().unwrap();
        // It is the same public native mapping the preparation validator uses.
        assert_eq!(super::super::matching_roster(&input.roster, &actual.member).unwrap(),
            inventory.members.iter().position(|row| row == actual).unwrap());
    }
    assert_ne!(inventory.members[0].member, inventory.members[1].member);
    // A durable identity that resembles an incarnation alias stays distinct.
    assert_ne!(inventory.members[0].member.member, inventory.members[2].member.member);
    assert_eq!(serde_json::to_value(&input).unwrap(), before);
    let encoded = toolkit_package::canonical_json(&inventory).unwrap();
    let round_trip: ApplicationInventory = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(round_trip, inventory);
    assert!(!serde_json::to_value(&inventory).unwrap().as_object().unwrap().contains_key("grants"));
}

#[test]
fn inventory_refuses_duplicate_missing_or_native_invalid_selections() {
    let original = input();
    let mut duplicate = original.clone();
    duplicate.roster.push(duplicate.roster[0].clone());
    assert!(duplicate.lower().unwrap_err().contains("repeats"));
    let mut missing = original.clone();
    missing.helper_identity = "not-selected".into();
    assert!(missing.lower().unwrap_err().contains("absent"));
    missing.helper_identity = roster_member_id_for_identity("helper:setup").to_string();
    assert!(missing.lower().unwrap_err().contains("absent"));
    let mut empty = original.clone();
    empty.roster.clear();
    assert!(empty.lower().is_err());
    let mut foreign = original.clone();
    foreign.mob_id = "not/a/mob".into();
    assert!(foreign.lower().is_err());
    let mut invalid_role = original.clone();
    invalid_role.roster[0].profile = meerkat_mob::ProfileName::from("not/a/profile");
    assert!(invalid_role.lower().is_err());
    let mut wrong_schema = original;
    wrong_schema.schema = 4;
    assert!(wrong_schema.lower().is_err());
    assert!(serde_json::from_value::<ApplicationInventoryInput>(json!({
        "schema":1,"mob_id":"household","helper_identity":"helper:setup",
        "roster":[{"identity":"bad identity","profile":"research"}]
    })).is_err());
}

#[test]
fn inventory_command_uses_existing_dispatch_and_creates_no_owner_or_artifact() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("compiler-input.json");
    let bytes = serde_json::to_vec(&input()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let args = vec!["--lower-application-inventory".into(), "--input".into(),
        path.to_str().unwrap().to_owned()];
    let mut progress = InstallerProgress::default();
    command(&args, &mut progress).unwrap();
    assert_eq!(progress.phase, "receipt");
    assert!(!progress.effects_may_have_occurred);
    assert_eq!(crate::application_bootstrap::run(&args), Some(std::process::ExitCode::SUCCESS));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    let invalid = br#"{"schema":1,"schema":1,"mob_id":"household","roster":[],"helper_identity":"none"}"#;
    std::fs::write(&path, invalid).unwrap();
    let mut refused = InstallerProgress::default();
    assert!(command(&args, &mut refused).is_err());
    assert!(!refused.effects_may_have_occurred);
    assert_eq!(std::fs::read(&path).unwrap(), invalid);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
}
