//! Reading's concrete management shape. The application remains the revision,
//! principal, activation and delivery owner; no local recipe or routing table.
use super::*;

const MAX_INTEGER: u64 = (1 << 53) - 1;
const MAX_TELEGRAM_ID: u64 = (1 << 52) - 1;

fn ensure(value: bool) -> Result<(), ForgeError> {
    if value { Ok(()) } else { Err(invalid()) }
}
fn fields(value: &Value, names: &[&str]) -> Result<(), ForgeError> {
    ensure(
        value
            .as_object()
            .is_some_and(|o| o.len() == names.len() && names.iter().all(|k| o.contains_key(*k))),
    )
}
fn integer(value: &Value, min: u64, max: u64) -> bool {
    value.as_u64().is_some_and(|n| n >= min && n <= max)
}
fn text(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        !s.is_empty() && s.chars().count() <= 1024 && !s.chars().any(char::is_control)
    })
}
fn one_of(value: &Value, choices: &[&str]) -> bool {
    value.as_str().is_some_and(|s| choices.contains(&s))
}
fn host(value: &Value) -> Result<ForgeHost, ForgeError> {
    let host: ForgeHost = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    ensure(host.valid())?;
    Ok(host)
}
fn binding(value: &Value) -> Result<(), ForgeError> {
    fields(value, &["host", "principal"])?;
    host(&value["host"])?;
    ensure(text(&value["principal"]))
}
fn configuration(value: &Value, historical: bool) -> Result<(), ForgeError> {
    fields(
        value,
        &[
            "component_id",
            "profile",
            "participant_id",
            "route_id",
            "telegram_user_id",
            "telegram_dm_chat_id",
            "native_identity",
            "bot_token",
            "exclusive_polling_acknowledged",
        ],
    )?;
    ensure(
        value["component_id"] == "reading-telegram"
            && value["profile"] == "private-text-dm"
            && value["participant_id"] == "reading-owner"
            && value["route_id"] == "reading-lead-dm"
            && value["native_identity"] == "lead:main"
            && integer(
                &value["telegram_user_id"],
                1,
                if historical {
                    MAX_INTEGER
                } else {
                    MAX_TELEGRAM_ID
                },
            )
            && value["telegram_dm_chat_id"] == value["telegram_user_id"]
            && value["exclusive_polling_acknowledged"] == true
            && value["bot_token"] == json!({"private_field":"READING_TELEGRAM_BOT_TOKEN"}),
    )
}

fn activation(value: &Value) -> Result<(), ForgeError> {
    if value["binding"] == "not-implemented" {
        fields(
            value,
            &[
                "state",
                "binding",
                "application_started",
                "provider_observation",
                "native_admission",
                "reply_receipt",
            ],
        )?;
        return ensure(
            one_of(&value["state"], &["unbound", "not-started"])
                && value["application_started"].is_boolean()
                && ["provider_observation", "native_admission", "reply_receipt"]
                    .iter()
                    .all(|k| value[*k].is_null()),
        );
    }
    fields(
        value,
        &[
            "state",
            "binding",
            "application_started",
            "active_revision",
            "provider_observation",
            "native_admission",
            "reply_receipt",
            "ownership_scope",
            "retained_history",
            "unfinished_management",
        ],
    )?;
    // Nested provider/native/receipt observations stay finite, bounded JSON from
    // the authenticated application. This client never promotes them into
    // input, destination, approval, completion or retry authority.
    ensure(
        value["binding"] == "reading-private-text-dm"
            && text(&value["state"])
            && value["application_started"].is_boolean()
            && (value["active_revision"].is_null()
                || integer(&value["active_revision"], 0, MAX_INTEGER))
            && value["ownership_scope"]
                == "same-application-local-locks-and-explicit-human-acknowledgement"
            && value["unfinished_management"]
                == json!(["pause", "disable", "remove", "pending-work-revocation"])
            && [
                "provider_observation",
                "native_admission",
                "reply_receipt",
                "retained_history",
            ]
            .iter()
            .all(|k| value[*k].is_null() || value[*k].is_object()),
    )
}

pub(super) fn inventory(value: &Value) -> Result<(), ForgeError> {
    fields(
        value,
        &[
            "storage",
            "revision",
            "configuration_status",
            "configuration",
            "binding",
            "binding_matches_current_source",
            "owner_principal",
            "participants",
            "routes",
            "component",
            "activation",
        ],
    )?;
    ensure(
        one_of(
            &value["storage"],
            &["available", "not-initialized", "unavailable"],
        ) && text(&value["owner_principal"]),
    )?;
    if value["revision"].is_null() {
        ensure(
            value["storage"] != "available"
                && value["configuration_status"] == "unknown"
                && value["configuration"].is_null()
                && value["binding"].is_null(),
        )?;
    } else {
        ensure(value["storage"] == "available" && integer(&value["revision"], 0, MAX_INTEGER))?;
        if value["configuration"].is_null() {
            ensure(
                value["configuration_status"] == "not-configured" && value["binding"].is_null(),
            )?;
        } else {
            configuration(&value["configuration"], true)?;
            binding(&value["binding"])?;
            ensure(value["configuration_status"] == "committed")?;
        }
    }
    ensure(if value["binding"].is_null() {
        value["binding_matches_current_source"].is_null()
    } else {
        value["binding_matches_current_source"].is_boolean()
    })?;
    let participants = value["participants"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(invalid)?;
    let participant = &participants[0];
    fields(
        participant,
        &[
            "id",
            "label",
            "principal",
            "telegram_user_id",
            "allowed_profile",
        ],
    )?;
    ensure(
        participant["id"] == "reading-owner"
            && participant["principal"] == value["owner_principal"]
            && text(&participant["label"])
            && participant["allowed_profile"] == "private-text-dm"
            && participant["telegram_user_id"] == value["configuration"]["telegram_user_id"],
    )?;
    let routes = value["routes"]
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(invalid)?;
    let route = &routes[0];
    fields(
        route,
        &[
            "id",
            "label",
            "participant_id",
            "native_identity",
            "destination_rule",
            "telegram_dm_chat_id",
        ],
    )?;
    ensure(
        route["id"] == "reading-lead-dm"
            && route["participant_id"] == participant["id"]
            && text(&route["label"])
            && route["native_identity"] == "lead:main"
            && route["destination_rule"] == "same-authorized-human-private-dm"
            && route["telegram_dm_chat_id"] == value["configuration"]["telegram_dm_chat_id"],
    )?;
    let component = &value["component"];
    fields(
        component,
        &[
            "id",
            "profile",
            "support",
            "native_identity",
            "private_reference",
            "participant",
            "outside_first_profile",
        ],
    )?;
    ensure(
        component["id"] == "reading-telegram"
            && component["profile"] == participant["allowed_profile"]
            && one_of(
                &component["support"],
                &["configuration-only", "private-text-dm-source-candidate"],
            )
            && component["native_identity"] == route["native_identity"]
            && component["private_reference"]
                == json!({"private_field":"READING_TELEGRAM_BOT_TOKEN"})
            && text(&component["participant"])
            && component["outside_first_profile"]
                == json!(["groups", "topics", "media", "multiple-participants"]),
    )?;
    activation(&value["activation"])
}

pub(super) fn schema() -> Value {
    json!({"type":"object","additionalProperties":false,
        "required":["participant_id","route_id","telegram_user_id","exclusive_polling_acknowledged","expected_revision"],
        "properties":{"participant_id":{"type":"string"},"route_id":{"type":"string"},
        "telegram_user_id":{"type":"integer","minimum":1,"maximum":MAX_TELEGRAM_ID},
        "exclusive_polling_acknowledged":{"const":true},
        "expected_revision":{"type":"integer","minimum":0,"maximum":MAX_INTEGER-1}}})
}
pub(super) fn prepare_body(raw: &str, observed: &Value) -> Result<Value, ForgeError> {
    inventory(observed)?;
    let input: Value = parse(raw)?;
    fields(
        &input,
        &[
            "participant_id",
            "route_id",
            "telegram_user_id",
            "exclusive_polling_acknowledged",
            "expected_revision",
        ],
    )?;
    ensure(
        observed["storage"] == "available"
            && integer(&input["expected_revision"], 0, MAX_INTEGER - 1)
            && input["expected_revision"] == observed["revision"]
            && input["participant_id"] == observed["participants"][0]["id"]
            && input["route_id"] == observed["routes"][0]["id"],
    )?;
    let value = json!({"component_id":observed["component"]["id"],"profile":observed["component"]["profile"],
        "participant_id":input["participant_id"],"route_id":input["route_id"],
        "telegram_user_id":input["telegram_user_id"],"telegram_dm_chat_id":input["telegram_user_id"],
        "native_identity":observed["routes"][0]["native_identity"],"bot_token":observed["component"]["private_reference"],
        "exclusive_polling_acknowledged":input["exclusive_polling_acknowledged"]});
    configuration(&value, false)?;
    Ok(json!({"configuration":value,"expected_revision":input["expected_revision"]}))
}
fn proposal_shape(value: &Value) -> Result<(), ForgeError> {
    fields(
        value,
        &[
            "change_id",
            "configuration",
            "expected_revision",
            "host",
            "principal",
            "expires_at",
        ],
    )?;
    ensure(
        value["change_id"].as_str().is_some_and(|s| {
            s.strip_prefix("reading-telegram:").is_some_and(|tail| {
                tail.len() == 32
                    && tail
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) && integer(&value["expected_revision"], 0, MAX_INTEGER - 1)
            && text(&value["principal"])
            && integer(&value["expires_at"], 1, MAX_INTEGER),
    )?;
    host(&value["host"])?;
    configuration(&value["configuration"], true)
}
pub(super) fn proposal(
    value: &Value,
    request: &Value,
    expected: &ForgeHost,
) -> Result<(), ForgeError> {
    proposal_shape(value)?;
    configuration(&value["configuration"], false)?;
    ensure(
        host(&value["host"])? == *expected
            && value["configuration"] == request["configuration"]
            && value["expected_revision"] == request["expected_revision"],
    )
}
pub(super) fn retained(value: &Value, reference: &RecoveryReference) -> Result<(), ForgeError> {
    proposal_shape(value)?;
    ensure(value["change_id"] == reference.change_id && host(&value["host"])? == reference.host)
}
pub(super) fn submission(value: &Value, reference: &RecoveryReference) -> Result<(), ForgeError> {
    let proposal: Value = parse(reference.proposal.get())?;
    retained(&proposal, reference)?;
    ensure(value["change_id"] == reference.change_id)?;
    if one_of(
        &value["status"],
        &["unknown", "change-conflict", "source-conflict"],
    ) {
        fields(value, &["change_id", "status", "activation"])?;
        return activation(&value["activation"]);
    }
    let names: &[&str] = match value["status"].as_str() {
        Some("committed") => &[
            "change_id",
            "status",
            "revision",
            "configuration_status",
            "activation_at_submission",
            "approval",
        ],
        Some("revision-conflict") => &[
            "change_id",
            "status",
            "revision",
            "activation_at_submission",
            "approval",
        ],
        Some("expired" | "owner-conflict") => &[
            "change_id",
            "status",
            "activation_at_submission",
            "approval",
        ],
        _ => return Err(invalid()),
    };
    fields(value, names)?;
    fields(&value["approval"], &["approved", "principal", "expires_at"])?;
    ensure(
        value["approval"]["approved"] == true
            && value["approval"]["principal"] == proposal["principal"]
            && integer(&value["approval"]["expires_at"], 1, MAX_INTEGER),
    )?;
    if value["status"] == "committed" {
        ensure(
            value["configuration_status"] == "committed"
                && value["revision"].as_u64()
                    == proposal["expected_revision"].as_u64().map(|r| r + 1),
        )?;
    } else if value["status"] == "revision-conflict" {
        ensure(
            integer(&value["revision"], 0, MAX_INTEGER)
                && value["revision"] != proposal["expected_revision"],
        )?;
    }
    activation(&value["activation_at_submission"])
}
pub(super) fn status(
    value: &Value,
    reference: &RecoveryReference,
    current: &ForgeHost,
) -> Result<(), ForgeError> {
    ensure(value["change_id"] == reference.change_id)?;
    if value["status"] == "unknown" {
        fields(value, &["change_id", "status", "activation", "storage"])?;
        ensure(one_of(
            &value["storage"],
            &["available", "not-initialized", "unavailable"],
        ))?;
        return activation(&value["activation"]);
    }
    fields(
        value,
        &[
            "change_id",
            "status",
            "proposal",
            "result",
            "storage",
            "binding_matches_current_source",
            "activation",
        ],
    )?;
    let original: Value = parse(reference.proposal.get())?;
    retained(&value["proposal"], reference)?;
    ensure(
        value["proposal"] == original
            && value["storage"] == "available"
            && value["binding_matches_current_source"] == json!(reference.host == *current),
    )?;
    if value["result"].is_null() {
        ensure(value["status"] == "prepared")?;
    } else {
        submission(&value["result"], reference)?;
        ensure(value["status"] == value["result"]["status"])?;
    }
    activation(&value["activation"])
}
