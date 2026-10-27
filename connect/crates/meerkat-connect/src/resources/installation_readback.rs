//! Fixed installed-product readback transport. Canonical backend.py remains
//! the owner of software verification; it is never selected by an agent path.
use super::*;
use meerkat_core::{approval::{ApprovalDecision, ApprovalRecord, ApprovalStatus}, McpServerConfig};
use serde_json::{Value, json};
use toolkit_mobkit::private_originals::{PrivateOriginalError, ServiceInstallationReadback,
    ServiceInstallationReadbackSource};
use crate::{connector_install::native_launch_configuration,
    service_install::{SelectedDocument, InstalledSelection, NativeConnectorEntry, RemoteSelection, ServiceCredentialSlot, public_launch_configuration, secret_launch_configuration}};
use std::collections::BTreeSet;

pub(crate) struct ProductInstallationReadback {
    pub storage: meerkat_core::ResolvedStorage,
}

fn denied() -> PrivateOriginalError { PrivateOriginalError::Denied }
fn unavailable() -> PrivateOriginalError { PrivateOriginalError::Unavailable }

fn selected_review(review: &ApprovalRecord, realm: &meerkat_core::RealmLocator)
    -> Result<(InstalledSelection, McpServerConfig, Value), PrivateOriginalError>
{
    if review.status != ApprovalStatus::Approved
        || !review.decision.as_ref().is_some_and(|d| d.decision == ApprovalDecision::Approve) {
        return Err(denied());
    }
    selected_body(review.request_body.as_ref().ok_or_else(denied)?, realm)
}

fn selected_body(body: &Value, realm: &meerkat_core::RealmLocator)
    -> Result<(InstalledSelection, McpServerConfig, Value), PrivateOriginalError>
{
    let software = &body["software"];
    let text = |key: &str, limit: usize| -> Result<String, PrivateOriginalError> {
        software[key].as_str().filter(|v| !v.is_empty() && v.len() <= limit
            && !v.chars().any(char::is_control)).map(str::to_owned).ok_or_else(denied)
    };
    let hash = |key| -> Result<String, PrivateOriginalError> {
        let value = text(key, 64)?;
        if !valid_digest(&value) { return Err(denied()); }
        Ok(value)
    };
    let id = |key| -> Result<String, PrivateOriginalError> {
        let value = text(key, 32)?;
        if value.len() != 32 || !value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(denied());
        }
        Ok(value)
    };
    let public = body["phase"] == "installed_public_service_access";
    let credential = body["phase"] == "installed_credential_service_access";
    let selected_package = matches!(software["family"].as_str(), Some("selected-python" | "selected-node"));
    if (!public && !credential && body["phase"] != "installed_account_access") || body["realm"] != realm.realm.as_str()
        || body["realm_state_root"] != json!(realm.state_root)
        || (!public && !credential && software["family"] != "services"
            && !(software["family"] == "agno" && software["profile"] == "salesforce"))
        || ((public || credential) && !matches!(software["family"].as_str(),
            Some("agno" | "services" | "activepieces" | "selected-python" | "selected-node"))) {
        return Err(denied());
    }
    let service_evidence = if public || credential {
        let authentication = if credential { json!({"kind":"declared_secret_slots"}) } else { json!({"kind":"none"}) };
        if body["authentication"] != authentication { return Err(denied()); }
        let basis = &body["operation_basis"];
        if selected_package {
            let document = SelectedDocument::from_value(basis).map_err(|_| denied())?;
            if basis["kind"] != document.basis_kind()
                || (if credential {
                    !matches!(serde_json::from_value::<toolkit_package::AuthenticationRequirement>(basis["authentication"].clone()),
                        Ok(toolkit_package::AuthenticationRequirement::Binding { .. }))
                } else { basis["authentication"] != json!({"kind":"none"}) })
                || software["family"] != format!("selected-{}", text("profile", 256)?)
                || !software["configuration_sha256"].is_null()
                || !["package_sha256", "effect_sha256"]
                    .iter().all(|key| basis[*key].as_str().is_some_and(valid_digest)) {
                return Err(denied());
            }
        } else if basis != &json!({"kind":"package_published_metadata"}) {
            return Err(denied());
        }
        let mut evidence = json!({"operation_basis":basis,
            "operations":body["application_selection"]["operations"]});
        public_operations(&evidence)?;
        if credential {
            evidence["credential_slots"] = body["credential_slots"].clone();
            credential_slots(&evidence, selected_package)?;
        }
        Some(evidence)
    } else { None };
    let path = |key| -> Result<PathBuf, PrivateOriginalError> {
        let path = PathBuf::from(text(key, 4096)?);
        if !path.is_absolute() || path.components().any(|p| matches!(p, Component::ParentDir | Component::CurDir)) {
            return Err(denied());
        }
        Ok(path)
    };
    let selected = InstalledSelection {
        realm: realm.clone(),
        session: serde_json::from_value(body["session"].clone()).map_err(|_| denied())?,
        job_id: id("job_id")?, validation_id: id("validation_id")?,
        family: text("family", 64)?, profile: text("profile", 256)?,
        mcp_path: path("mcp_path")?, receipt_path: path("receipt_path")?,
        receipt_sha256: Some(hash("receipt_sha256")?),
        configuration_sha256: if selected_package { None } else { Some(hash("configuration_sha256")?) },
        mcp_document_sha256: hash("document_sha256")?,
        recovery_request_sha256: Some(hash("recovery_request_sha256")?),
        server_name: text("server_name", 256)?,
    };
    let installation: McpServerConfig = serde_json::from_value(body["installation"].clone()).map_err(|_| denied())?;
    if installation.name != selected.server_name || ((public || credential) && installation.connector.is_some()) {
        return Err(denied());
    }
    // Only canonical, authenticated operator recovery calls this port. The
    // package request carries no account credential or runtime authority.
    let mut request = json!({"software":software, "realm_state_root":realm.state_root, "realm":realm.realm.as_str()});
    if let Some(evidence) = service_evidence {
        request[if credential { "credential_service" } else { "public_service" }] = evidence;
    }
    Ok((selected, installation, request))
}

fn selected_remote_review(review: &ApprovalRecord, realm: &meerkat_core::RealmLocator)
    -> Result<(RemoteSelection, McpServerConfig, Value), PrivateOriginalError>
{
    if review.status != ApprovalStatus::Approved
        || !review.decision.as_ref().is_some_and(|d|d.decision==ApprovalDecision::Approve) { return Err(denied()); }
    selected_remote_body(review.request_body.as_ref().ok_or_else(denied)?, realm)
}

fn selected_remote_body(body: &Value, realm: &meerkat_core::RealmLocator)
    -> Result<(RemoteSelection, McpServerConfig, Value), PrivateOriginalError>
{
    if body["phase"]!="installed_public_service_access" || body.get("software").is_some()
        || body["realm"]!=realm.realm.as_str() || body["realm_state_root"]!=json!(realm.state_root)
        || body["authentication"]!=json!({"kind":"none"}) { return Err(denied()); }
    let selected=RemoteSelection::from_review_source(&body["remote_export"],realm,
        serde_json::from_value(body["session"].clone()).map_err(|_|denied())?).map_err(|_|denied())?;
    if body["operation_basis"]!=selected.operation_basis() { return Err(denied()); }
    let operations=public_operations(&body["application_selection"])?;
    let reviewed: McpServerConfig=serde_json::from_value(body["installation"].clone()).map_err(|_|denied())?;
    if selected.public_configuration(&operations).ok().as_ref()!=Some(&reviewed) { return Err(denied()); }
    let request=selected.readback_request(&operations);
    Ok((selected,reviewed,request))
}

fn public_operations(evidence: &Value) -> Result<BTreeSet<String>, PrivateOriginalError> {
    let rows = evidence["operations"].as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 128).ok_or_else(denied)?;
    let operations = rows.iter().map(|value| value.as_str()
        .filter(|name| !name.is_empty() && name.len() <= 128
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
        .map(str::to_owned).ok_or_else(denied)).collect::<Result<BTreeSet<_>, _>>()?;
    if operations.len() != rows.len() { return Err(denied()); }
    Ok(operations)
}

fn credential_slots(evidence: &Value, selected_package: bool)
    -> Result<Vec<ServiceCredentialSlot>, PrivateOriginalError>
{
    let rows = evidence["credential_slots"].as_array()
        .filter(|rows| !rows.is_empty() && rows.len() <= 256).ok_or_else(denied)?;
    let slots: Vec<ServiceCredentialSlot> = serde_json::from_value(evidence["credential_slots"].clone())
        .map_err(|_| denied())?;
    if serde_json::to_value(&slots).map_err(|_| denied())? != evidence["credential_slots"]
        || slots.iter().any(|slot| match slot {
            ServiceCredentialSlot::SelectedEnvironment(_) => !selected_package,
            ServiceCredentialSlot::LibraryField(_) => selected_package,
        })
        || slots.iter().map(ServiceCredentialSlot::environment_variable).collect::<BTreeSet<_>>().len() != rows.len()
    { return Err(denied()); }
    Ok(slots)
}

fn checked_readback(selected: &InstalledSelection, reviewed: &McpServerConfig, value: &Value,
    public: Option<&Value>, credential: Option<&Value>)
    -> Result<McpServerConfig, PrivateOriginalError>
{
    let selected_package = matches!(selected.family.as_str(), "selected-python" | "selected-node");
    if value["scope"] != "retained_payload_and_launch_binding_only"
        || (!selected_package && (value["family"] != selected.family || value["profile"] != selected.profile))
        || (selected_package && value["selection_kind"] != selected.profile)
        || value["mcp_path"] != json!(selected.mcp_path)
        || value["receipt_path"] != json!(selected.receipt_path)
        || value["receipt_sha256"].as_str() != selected.receipt_sha256.as_deref()
        || value["configuration_sha256"].as_str() != selected.configuration_sha256.as_deref()
        || digest(&serde_json::to_vec(&value["mcp_configuration"]).map_err(|_| denied())?) != selected.mcp_document_sha256 {
        return Err(denied());
    }
    if public.is_some() && credential.is_some() { return Err(denied()); }
    if let Some(evidence) = public.or(credential) {
        let slots = credential.map(|value| credential_slots(value, selected_package)).transpose()?;
        let operations = public_operations(evidence)?;
        if selected_package {
            let basis = &evidence["operation_basis"];
            let document = SelectedDocument::from_value(basis).map_err(|_| denied())?;
            if basis["kind"] != document.basis_kind()
                || SelectedDocument::from_value(value).map_err(|_| denied())? != document
                || value["declared_service"]["authentication"] != basis["authentication"]
                || ["package_sha256", "effect_sha256"]
                    .iter().any(|key| value[*key] != basis[*key])
                || json!({"plugin_source":value.get("plugin_source"),
                          "acquired_source":value.get("acquired_source")}) != basis["source"] {
                return Err(denied());
            }
            if let Some(slots) = &slots {
                let declared: toolkit_package::EnvironmentSecretSlots = serde_json::from_value(
                    value["declared_service"]["environment_secret_slots"].clone()).map_err(|_| denied())?;
                if !matches!(serde_json::from_value::<toolkit_package::AuthenticationRequirement>(basis["authentication"].clone()),
                    Ok(toolkit_package::AuthenticationRequirement::Binding { .. }))
                    || declared.names().iter().map(String::as_str).collect::<BTreeSet<_>>()
                        != slots.iter().map(ServiceCredentialSlot::environment_variable).collect::<BTreeSet<_>>()
                { return Err(denied()); }
            } else if basis["authentication"] != json!({"kind":"none"}) { return Err(denied()); }
            let names = value["declared_service"]["operation_tools"].as_object()
                .filter(|names| !names.is_empty() && names.len() <= 128).ok_or_else(denied)?;
            let raw = public_operations(&json!({"operations":names.values().collect::<Vec<_>>()}))?;
            if !operations.is_subset(&raw) { return Err(denied()); }
        }
        // The approved review retains the checked authentication and slot
        // declarations. Reuse the original launch projector after canonical
        // payload verification; secret values and native grants never enter it.
        let derived = match slots {
            Some(slots) => secret_launch_configuration(selected, &value["mcp_configuration"], &operations, &slots),
            None => public_launch_configuration(selected, &value["mcp_configuration"], &operations),
        }.map_err(|_| denied())?;
        if &derived != reviewed { return Err(denied()); }
        return Ok(derived);
    }
    if selected_package { return Err(denied()); }
    let entry = NativeConnectorEntry::observed(&value["native_connector"], &selected.family, &selected.profile)
        .map_err(|_| denied())?;
    let mut derived = native_launch_configuration(selected, &value["mcp_configuration"], &entry)
        .map_err(|_| denied())?;
    // Reuse the exact approved account/operation fields. Full descriptor
    // equality refuses added transport, environment or authority fields.
    derived.connector = reviewed.connector.clone();
    derived.access = reviewed.access.clone();
    if &derived != reviewed { return Err(denied()); }
    derived.validate_access().map_err(|_| denied())?;
    Ok(derived)
}

fn selected_application_body(body: &Value, configuration: &McpServerConfig,
    realm: &meerkat_core::RealmLocator) -> Result<(McpServerConfig, Value), PrivateOriginalError> {
    let reviewed = if body.get("remote_export").is_some() {
        selected_remote_body(body, realm)?.1
    } else { selected_body(body, realm)?.1 };
    if &reviewed != configuration { return Err(denied()); }
    Ok((reviewed, body.clone()))
}

#[async_trait::async_trait]
impl ServiceInstallationReadback for ProductInstallationReadback {
    async fn readback_source(&self, source: &ServiceInstallationReadbackSource)
        -> Result<McpServerConfig, PrivateOriginalError> {
        if let ServiceInstallationReadbackSource::ReviewedBatchMember(source) = source {
            let body = source.document();
            let resources = super::readback_process::locate_readback_resources("resources_before").await.map_err(|_| unavailable())?;
            if body.get("remote_export").is_some() {
                let (selected, reviewed, request) = selected_remote_body(body, &self.storage.locator)?;
                let value = resources.readback(&self.storage, &request).await.map_err(|_| unavailable())?;
                if value != selected.export { return Err(denied()); }
                return Ok(reviewed);
            }
            let (selected, reviewed, request) = selected_body(body, &self.storage.locator)?;
            let value = resources.readback(&self.storage, &request).await.map_err(|_| unavailable())?;
            return checked_readback(&selected, &reviewed, &value, request.get("public_service"), request.get("credential_service"));
        }
        let ServiceInstallationReadbackSource::ApplicationPolicy(source) = source else {
            let ServiceInstallationReadbackSource::SettledReview(review) = source else { return Err(denied()); };
            return self.readback(review).await;
        };
        let (configuration, body) = selected_application_body(source.document(), &source.configuration()?, &self.storage.locator)?;
        if body.get("remote_export").is_some() {
            let (selected, reviewed, request) = selected_remote_body(&body, &self.storage.locator)?;
            let resources = super::readback_process::locate_readback_resources("resources_before").await.map_err(|_| unavailable())?;
            let value = resources.readback(&self.storage, &request).await.map_err(|_| unavailable())?;
            if value != selected.export || reviewed != configuration { return Err(denied()); }
            return Ok(reviewed);
        }
        let (selected, reviewed, request) = selected_body(&body, &self.storage.locator)?;
        let resources = super::readback_process::locate_readback_resources("resources_before").await.map_err(|_| unavailable())?;
        let value = resources.readback(&self.storage, &request).await.map_err(|_| unavailable())?;
        let observed = checked_readback(&selected, &reviewed, &value, request.get("public_service"), request.get("credential_service"))?;
        if observed != configuration { return Err(denied()); }
        Ok(observed)
    }

    async fn readback(&self, review: &ApprovalRecord) -> Result<McpServerConfig, PrivateOriginalError> {
        if review.request_body.as_ref().is_some_and(|body|body.get("remote_export").is_some()) {
            let (selected,reviewed,request)=selected_remote_review(review,&self.storage.locator)?;
            let resources=super::readback_process::locate_readback_resources("resources_before").await.map_err(|_|unavailable())?;
            let value=resources.readback(&self.storage,&request).await.map_err(|_|unavailable())?;
            if value!=selected.export { return Err(denied()); }
            return Ok(reviewed);
        }
        let (selected, reviewed, request) = selected_review(review, &self.storage.locator)?;
        // No resources or process can be chosen by the approval body.
        let resources = super::readback_process::locate_readback_resources("resources_before").await.map_err(|_| unavailable())?;
        let value = resources.readback(&self.storage, &request).await.map_err(|_| unavailable())?;
        checked_readback(&selected, &reviewed, &value, request.get("public_service"), request.get("credential_service"))
    }
}

#[cfg(test)]
pub(crate) fn checked_application_body_for_test(body: &Value, config: &McpServerConfig,
    realm: &meerkat_core::RealmLocator) -> Result<McpServerConfig, PrivateOriginalError> {
    selected_application_body(body, config, realm).map(|(config, _)| config)
}

#[cfg(test)]
pub(crate) fn checked_credential_review_for_test(review: &ApprovalRecord, realm: &meerkat_core::RealmLocator,
    value: &Value) -> Result<(McpServerConfig, Value), PrivateOriginalError>
{
    let (selected, reviewed, request) = selected_review(review, realm)?;
    let config = checked_readback(&selected, &reviewed, value,
        request.get("public_service"), request.get("credential_service"))?;
    Ok((config, request))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::approval::{ApprovalActionKind, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
        ApprovalResourceRef, ApprovalRisk};
    use meerkat_core::{ApprovalService, SessionId};

    // Shape controls for the package-port parser. These are not authenticated
    // installed artifacts, private presentation, native attachment or readiness.
    fn fixture() -> (meerkat_core::RealmLocator, ApprovalRecord, Value) {
        let realm = meerkat_core::RealmLocator {
            realm: meerkat_core::connection::RealmId::parse("work").unwrap(),
            state_root: PathBuf::from("/fixture/state"),
        };
        let session = SessionId::new();
        let installation_root = "/fixture/state/.meerkat-connect/work/setup-one/prepared/session/installations/22222222222222222222222222222222";
        let command = format!("{installation_root}/runtime/bin/python");
        let args = vec!["-I".into(), "-m".into(), "meerkat_toolkit_services.adapters.services_mcp".into(),
            "serve".into(), "--config".into(), format!("{installation_root}/configuration/selected.json")];
        let mcp = json!({"mcpServers":{"mail":{"command":command,"args":args}}});
        let mut native_args = args;
        native_args[3] = "serve-native".into();
        let config = McpServerConfig::stdio("mail", command, native_args, Default::default());
        let software = json!({"job_id":"1".repeat(32),"validation_id":"2".repeat(32),
            "family":"services","profile":"gmail","mcp_path":format!("{installation_root}/ready/mcp.json"),
            "receipt_path":format!("{installation_root}/ready/installed.json"),
            "receipt_sha256":"3".repeat(64),"configuration_sha256":"4".repeat(64),
            "document_sha256":digest(&serde_json::to_vec(&mcp).unwrap()),
            "recovery_request_sha256":"5".repeat(64),"server_name":"mail"});
        let body = json!({"phase":"installed_account_access","realm":"work",
            "realm_state_root":realm.state_root,"session":session,"software":software,"installation":config});
        let approvals = ApprovalService::new();
        let pending = approvals.request(ApprovalRequest {
            requester: ApprovalPrincipalId::new("parser-fixture").unwrap(),
            owner: ApprovalOwnerRef::Session { session_id: session },
            resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime, id: ApprovalResourceId::new("fixture") },
            proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
                summary:"Fixture parser record".into(),body:Some(body.clone()) },
            risk: ApprovalRisk::High, request_body: Some(body),
            allowed_decisions:[ApprovalDecision::Approve,ApprovalDecision::Deny].into(),
            expires_at:None,metadata:Default::default(),request_provenance:None,
        }).unwrap();
        let approved = approvals.decide(&pending.approval_id,ApprovalDecision::Approve,
            ApprovalPrincipalId::new("parser-fixture").unwrap(),None,None).unwrap();
        let result = json!({"scope":"retained_payload_and_launch_binding_only","family":"services","profile":"gmail",
            "mcp_path":software["mcp_path"],"receipt_path":software["receipt_path"],
            "receipt_sha256":software["receipt_sha256"],"configuration_sha256":software["configuration_sha256"],
            "mcp_configuration":mcp,"native_connector":{"schema_version":1,
                "module":"meerkat_toolkit_services.adapters.services_mcp","mode":"serve-native","configuration_argument":"--config",
                "member":"meerkat_toolkit_services/adapters/services_mcp.py","member_sha256":"6".repeat(64),
                "wheel":{"filename":"meerkat_toolkit_services-0.1.0-py3-none-any.whl","sha256":"7".repeat(64)},
                "package_sources_sha256":"8".repeat(64)}});
        (realm,approved,result)
    }

    #[tokio::test]
    async fn remote_review_parser_preserves_export_anchor_and_exact_native_projection() {
        let root=tempfile::tempdir().unwrap();
        let remote=crate::stdio::tests::remote_public_fixture(root.path()).await;
        let selected=remote.public_remote();
        let operations=BTreeSet::from(["actual.lookup".to_owned()]);
        let config=selected.public_configuration(&operations).unwrap();
        let (_,mut approved,_)=fixture();
        let mut body=selected.review_source();
        body["phase"]=json!("installed_public_service_access");
        body["realm"]=json!(selected.realm().realm.as_str());
        body["realm_state_root"]=json!(selected.realm().state_root);
        body["session"]=json!(selected.session());
        body["authentication"]=json!({"kind":"none"});
        body["operation_basis"]=selected.operation_basis();
        body["application_selection"]=json!({"agent":"original-agent","expires_in_ms":60000,"operations":operations});
        body["installation"]=serde_json::to_value(&config).unwrap();
        approved.request_body=Some(body.clone());
        let (source,projected,request)=selected_remote_review(&approved,selected.realm()).unwrap();
        assert_eq!(projected,config);
        assert_eq!(request,source.readback_request(&operations));
        assert!(request.get("software").is_none());
        assert_eq!(request["remote_export"]["recovery_request_sha256"],"6".repeat(64));
        for mode in ["account","software","operations","declaration","url","scope","anchor","decision"] {
            let mut changed=approved.clone();let value=changed.request_body.as_mut().unwrap();
            match mode {
                "account" => value["authentication"]=json!({"kind":"binding"}),
                "software" => value["software"]=json!({}),
                "operations" => value["application_selection"]["operations"]=json!(["other.write"]),
                "declaration" => { value["remote_export"]["export"].as_object_mut().unwrap().remove("declared_remote_service"); },
                "url" => value["remote_export"]["export"]["mcp_configuration"]["mcpServers"]["outside"]["url"]=json!("https://changed.invalid/mcp"),
                "scope" => value["realm"]=json!("another"),
                "anchor" => value["remote_export"]["recovery_request_sha256"]=json!("not-a-digest"),
                "decision" => changed.status=ApprovalStatus::Pending,
                _ => unreachable!(),
            }
            assert!(selected_remote_review(&changed,selected.realm()).is_err(),"{mode}");
        }
        remote.owner.retire();
        // This parser projects a durable decision only. The fixed canonical
        // readback process must still re-open the selected request afterwards.
        assert!(selected_remote_review(&approved,selected.realm()).is_ok());
    }

    #[test]
    fn package_recovery_requires_reviewed_request_reference_before_resource_lookup() {
        let (realm, record, _) = fixture();
        assert!(selected_review(&record, &realm).is_ok());
        for field in ["recovery_request_sha256", "receipt_sha256", "configuration_sha256", "document_sha256", "mcp_path"] {
            let mut changed = record.clone();
            changed.request_body.as_mut().unwrap()["software"].as_object_mut().unwrap().remove(field);
            assert!(matches!(selected_review(&changed, &realm), Err(PrivateOriginalError::Denied)), "{field}");
        }
        let mut changed = record.clone();
        changed.request_body.as_mut().unwrap()["realm"] = "other-realm".into();
        assert!(matches!(selected_review(&changed, &realm), Err(PrivateOriginalError::Denied)));
    }

    #[test]
    fn package_readback_keeps_reviewed_transport_and_exact_owner_digests() {
        let (realm, record, result) = fixture();
        let (selected, reviewed, _) = selected_review(&record, &realm).unwrap();
        assert_eq!(checked_readback(&selected, &reviewed, &result, None, None).unwrap(), reviewed);
        for field in ["receipt_sha256", "configuration_sha256", "mcp_path", "profile"] {
            let mut changed = result.clone(); changed[field] = "changed".into();
            assert!(matches!(checked_readback(&selected, &reviewed, &changed, None, None), Err(PrivateOriginalError::Denied)), "{field}");
        }
        let mut changed = result.clone();
        changed["mcp_configuration"]["mcpServers"]["mail"]["command"] = "other-command".into();
        assert!(checked_readback(&selected, &reviewed, &changed, None, None).is_err());
        let mut rebound = reviewed.clone();
        rebound.name = "other-server".into();
        assert!(checked_readback(&selected, &rebound, &result, None, None).is_err());
    }

    #[test]
    fn package_readback_refuses_ordinary_installation_without_native_entry() {
        let (realm, record, mut result) = fixture();
        let (selected, reviewed, _) = selected_review(&record, &realm).unwrap();
        // Historical ordinary readback may be matched without declaring a
        // native entry. It cannot authorize a serve-native launch by name.
        result.as_object_mut().unwrap().remove("native_connector");
        assert!(matches!(
            checked_readback(&selected, &reviewed, &result, None, None),
            Err(PrivateOriginalError::Denied)
        ));
    }

    #[test]
    fn salesforce_account_readback_requires_the_selected_profile_and_native_declaration() {
        // Parser controls only. The canonical verifier owns wheel and source
        // byte equality; this does not establish Salesforce login or use.
        let (realm, mut record, mut result) = fixture();
        let body = record.request_body.as_mut().unwrap();
        body["software"]["family"] = json!("agno");
        body["software"]["profile"] = json!("salesforce");
        body["installation"]["args"][2] = json!("meerkat_toolkit_agno.agno_mcp");
        result["family"] = json!("agno");
        result["profile"] = json!("salesforce");
        result["mcp_configuration"]["mcpServers"]["mail"]["args"][2] = json!("meerkat_toolkit_agno.agno_mcp");
        body["software"]["document_sha256"] = digest(&serde_json::to_vec(&result["mcp_configuration"]).unwrap()).into();
        result["native_connector"]["module"] = json!("meerkat_toolkit_agno.agno_mcp");
        result["native_connector"]["member"] = json!("meerkat_toolkit_agno/agno_mcp.py");
        result["native_connector"]["profiles"] = json!(["salesforce"]);
        result["native_connector"]["wheel"]["filename"] = json!("meerkat_toolkit_agno-0.1.0-py3-none-any.whl");
        let (selected, reviewed, request) = selected_review(&record, &realm).unwrap();
        assert!(request.get("public_service").is_none());
        assert_eq!(checked_readback(&selected, &reviewed, &result, None, None).unwrap(), reviewed);
        for profile in [json!("tavily"), json!("hackernews"), Value::Null] {
            let mut changed = record.clone();
            changed.request_body.as_mut().unwrap()["software"]["profile"] = profile;
            assert!(selected_review(&changed, &realm).is_err());
        }
        for declaration in [Value::Null, json!("salesforce"), json!([]),
            json!(["tavily"]), json!(["salesforce", "tavily"])] {
            let mut changed = result.clone();
            changed["native_connector"]["profiles"] = declaration;
            assert!(checked_readback(&selected, &reviewed, &changed, None, None).is_err());
        }
        let mut changed = result.clone();
        changed.as_object_mut().unwrap().remove("native_connector");
        assert!(checked_readback(&selected, &reviewed, &changed, None, None).is_err());
        let mut changed = result.clone();
        changed["profile"] = json!("tavily");
        assert!(checked_readback(&selected, &reviewed, &changed, None, None).is_err());
        let mut changed = result;
        changed["native_connector"]["mode"] = json!("serve");
        assert!(checked_readback(&selected, &reviewed, &changed, None, None).is_err());
    }

    fn public_fixture(profile: Option<&str>) -> (meerkat_core::RealmLocator, ApprovalRecord, Value) {
        let (realm, mut record, mut result) = fixture();
        let body = record.request_body.as_mut().unwrap();
        body["phase"] = "installed_public_service_access".into();
        body["authentication"] = json!({"kind":"none"});
        body["application_selection"] = json!({"agent":"original-agent", "expires_in_ms":null,
            "operations":["actual_read"]});
        body["operation_basis"] = json!({"kind":"package_published_metadata"});
        body["software"]["family"] = "agno".into();
        body["software"]["profile"] = "public-fixture".into();
        result["family"] = "agno".into();
        result["profile"] = "public-fixture".into();
        result.as_object_mut().unwrap().remove("native_connector");
        let command = "/fixture/state/public-runtime/bin/python";
        let args = vec!["-I".to_owned(), "-m".to_owned(), "fixture_public_mcp".to_owned()];
        result["mcp_configuration"] = json!({"mcpServers":{"mail":{"command":command,"args":args}}});
        body["software"]["document_sha256"] = digest(&serde_json::to_vec(&result["mcp_configuration"]).unwrap()).into();
        let mut config = McpServerConfig::stdio("mail", command, args, Default::default());
        config.access = Some(meerkat_core::McpServerAccess::allowing(["actual_read"]));
        body["installation"] = serde_json::to_value(config).unwrap();
        if let Some(profile) = profile {
            body["software"]["family"] = format!("selected-{profile}").into();
            body["software"]["profile"] = profile.into();
            body["software"]["configuration_sha256"] = Value::Null;
            body["operation_basis"] = json!({"kind":"selected_manifest_declarations",
                "authentication":{"kind":"none"},"package_sha256":"a".repeat(64),
                "selection_sha256":"b".repeat(64),"manifest_digest":"c".repeat(64),
                "effect_sha256":"d".repeat(64),"source":{"plugin_source":null,"acquired_source":null}});
            for field in ["family", "profile", "configuration_sha256"] {
                result.as_object_mut().unwrap().remove(field);
            }
            for field in ["package_sha256", "selection_sha256", "manifest_digest", "effect_sha256"] {
                result[field] = body["operation_basis"][field].clone();
            }
            result["selection_kind"] = profile.into();
            result["declared_service"] = json!({"authentication":{"kind":"none"},
                "operation_tools":{"declared/read":"actual_read", "declared/other":"other_read"}});
        }
        (realm, record, result)
    }

    #[test]
    fn application_software_body_reuses_exact_public_package_checks_without_human_record() {
        for profile in [None, Some("python"), Some("node")] {
            let (realm, human_fixture, result) = public_fixture(profile);
            // Only the existing software fixture body is used below. No human
            // decision is made or accepted by this application parser.
            let body = human_fixture.request_body.unwrap();
            let config: McpServerConfig = serde_json::from_value(body["installation"].clone()).unwrap();
            let (selected_config, document) = selected_application_body(&body, &config, &realm).unwrap();
            let (selected, reviewed, request) = selected_body(&document, &realm).unwrap();
            assert_eq!(selected_config, reviewed);
            assert_eq!(checked_readback(&selected, &reviewed, &result,
                request.get("public_service"), None).unwrap(), config);
            let mut other = config.clone(); other.name = "other-server".into();
            assert!(selected_application_body(&body, &other, &realm).is_err());
            for field in ["receipt_sha256", "document_sha256", "recovery_request_sha256"] {
                let mut changed = body.clone(); changed["software"][field] = "invalid".into();
                assert!(selected_application_body(&changed, &config, &realm).is_err(), "{field}");
            }
            let mut changed = result.clone(); changed["receipt_sha256"] = "f".repeat(64).into();
            assert!(checked_readback(&selected, &reviewed, &changed, request.get("public_service"), None).is_err());
        }
    }

    #[test]
    fn public_readback_preserves_launch_without_synthesizing_native_connector() {
        for profile in [None, Some("python"), Some("node")] {
            let (realm, record, result) = public_fixture(profile);
            let (selected, reviewed, request) = selected_review(&record, &realm).unwrap();
            let public = request.get("public_service");
            assert_eq!(checked_readback(&selected, &reviewed, &result, public, None).unwrap(), reviewed);
            assert!(reviewed.connector.is_none());
            assert!(checked_readback(&selected, &reviewed, &result, None, None).is_err());
            let mut widened = reviewed.clone();
            widened.access = None;
            assert!(checked_readback(&selected, &widened, &result, public, None).is_err());
            let mut changed = result.clone();
            changed["receipt_sha256"] = "f".repeat(64).into();
            assert!(checked_readback(&selected, &reviewed, &changed, public, None).is_err());
            let mut changed = result.clone();
            changed["mcp_configuration"]["mcpServers"]["mail"]["env"] = json!({"TOKEN":"value"});
            let mut rebound = selected.clone();
            rebound.mcp_document_sha256 = digest(&serde_json::to_vec(&changed["mcp_configuration"]).unwrap());
            assert!(checked_readback(&rebound, &reviewed, &changed, public, None).is_err());
        }
    }

    #[test]
    fn public_review_requires_explicit_authentication_and_matching_selection_kind() {
        let (realm, record, _) = public_fixture(Some("python"));
        for (field, value) in [("authentication", Value::Null),
            ("authentication", json!({"kind":"binding"})), ("phase", json!("installed_account_access"))] {
            let mut changed = record.clone();
            changed.request_body.as_mut().unwrap()[field] = value;
            assert!(selected_review(&changed, &realm).is_err());
        }
        for (field, value) in [("profile", json!("node")), ("family", json!("selected-mcpb")),
            ("configuration_sha256", json!("e".repeat(64)))] {
            let mut changed = record.clone();
            changed.request_body.as_mut().unwrap()["software"][field] = value;
            assert!(selected_review(&changed, &realm).is_err());
        }
        let mut changed = record.clone();
        changed.request_body.as_mut().unwrap()["application_selection"]["operations"] = json!(["actual_read", "actual_read"]);
        assert!(selected_review(&changed, &realm).is_err());
        for field in ["package_sha256", "selection_sha256", "manifest_digest", "effect_sha256"] {
            let mut changed = record.clone();
            changed.request_body.as_mut().unwrap()["operation_basis"].as_object_mut().unwrap().remove(field);
            assert!(selected_review(&changed, &realm).is_err(), "{field}");
        }
    }

    #[test]
    fn selected_readback_binds_declarations_inputs_provenance_and_raw_names() {
        let (realm, record, result) = public_fixture(Some("python"));
        let (selected, reviewed, request) = selected_review(&record, &realm).unwrap();
        let public = request.get("public_service");
        for field in ["package_sha256", "selection_sha256", "manifest_digest", "effect_sha256", "selection_kind"] {
            let mut changed = result.clone(); changed[field] = "changed".into();
            assert!(checked_readback(&selected, &reviewed, &changed, public, None).is_err(), "{field}");
        }
        for declaration in [Value::Null, json!({"authentication":{"kind":"binding"},
                "operation_tools":{"declared/read":"actual_read"}}),
            json!({"authentication":{"kind":"none"},"operation_tools":{"declared/read":"different_read"}}),
            json!({"authentication":{"kind":"none"},"operation_tools":{"one":"actual_read","two":"actual_read"}})] {
            let mut changed = result.clone(); changed["declared_service"] = declaration;
            assert!(checked_readback(&selected, &reviewed, &changed, public, None).is_err());
        }
        let mut changed = result.clone(); changed["acquired_source"] = json!({"archive_sha256":"f".repeat(64)});
        assert!(checked_readback(&selected, &reviewed, &changed, public, None).is_err());
    }
    #[test]
    fn software_readback_requires_matching_kind_and_canonical_digest_through_final_projection() {
        for profile in ["python","node"] {
            let (realm,mut record,mut result)=public_fixture(Some(profile));
            let basis=&mut record.request_body.as_mut().unwrap()["operation_basis"];
            basis["kind"]=json!("selected_software_declarations");
            for value in [basis,&mut result] {
                let digest=value.as_object_mut().unwrap().remove("manifest_digest").unwrap();
                value["document_kind"]=json!("local_software_declaration");
                value["declaration_digest"]=digest;
            }
            let (selected,reviewed,request)=selected_review(&record,&realm).unwrap();
            let public=request.get("public_service");
            assert_eq!(checked_readback(&selected,&reviewed,&result,public,None).unwrap(),reviewed);
            for mode in ["missing-kind","wrong-kind","dual-digest","missing-digest","changed-digest","raw-digest"] {
                let mut changed=result.clone();
                match mode {
                    "missing-kind" => { changed.as_object_mut().unwrap().remove("document_kind"); },
                    "wrong-kind" => changed["document_kind"]=json!("activation_manifest"),
                    "dual-digest" => changed["manifest_digest"]=json!("c".repeat(64)),
                    "missing-digest" => { changed.as_object_mut().unwrap().remove("declaration_digest"); },
                    "changed-digest" => changed["declaration_digest"]=json!("e".repeat(64)),
                    "raw-digest" => changed["selection_sha256"]=json!("e".repeat(64)),
                    _ => unreachable!(),
                }
                assert!(checked_readback(&selected,&reviewed,&changed,public,None).is_err(),"{mode}");
            }
            for mode in ["basis","dual-digest","missing-kind"] {
                let mut changed=record.clone();
                let basis=&mut changed.request_body.as_mut().unwrap()["operation_basis"];
                match mode {
                    "basis" => basis["kind"]=json!("selected_manifest_declarations"),
                    "dual-digest" => basis["manifest_digest"]=json!("c".repeat(64)),
                    _ => { basis.as_object_mut().unwrap().remove("document_kind"); },
                }
                assert!(selected_review(&changed,&realm).is_err(),"{mode}");
            }
        }
    }

}
