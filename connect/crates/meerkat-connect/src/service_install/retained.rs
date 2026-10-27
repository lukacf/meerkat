//! Current-session selections from the fixed package owner's retained requests.
//! These entries contain observations only, never checked install proposals.
use super::*;
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct Returning { entries: BTreeMap<String, Entry>, bytes: usize }
struct Entry { session: SessionId, value: Retained, bytes: usize }
enum Retained {
    Candidate { locator: Value, family: String, profile: String, verified: Option<VerifiedRetained> },
    Cursor(Value),
}
#[derive(Clone)]
pub(crate) struct VerifiedRetained {
    pub(crate) installed: InstalledSelection,
    pub(crate) configuration: Arc<Value>,
    metadata: OperationEvidence,
    operations: BTreeSet<String>,
    native_connector: Option<NativeConnectorEntry>,
    references: Option<Vec<ServiceCredentialReference>>,
    bytes: usize,
}
impl VerifiedRetained {
    fn selection(&self, owner: &Arc<CheckedInstalls>) -> InstalledConnectorSelection {
        InstalledConnectorSelection { installed: self.installed.clone(), configuration: self.configuration.clone(),
            metadata: self.metadata.clone(), raw_operations: self.operations.clone(), native_connector: self.native_connector.clone(),
            no_credential_references: self.references.as_ref().is_some_and(Vec::is_empty),
            credential_references: self.references.clone(), owner: owner.clone() }
    }
    pub(crate) fn selected_inputs(&self) -> Option<&SelectedServiceInputs> {
        match &self.metadata { OperationEvidence::Selected { inputs, .. } => Some(inputs), _ => None }
    }
}
impl Returning {
    pub(super) fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub(super) fn selection(&self, owner: &Arc<CheckedInstalls>, session: &SessionId, job: &str)
        -> Option<InstalledConnectorSelection> {
        let mut matches = self.entries.values().filter(|entry| entry.session == *session).filter_map(|entry| {
            match &entry.value {
                Retained::Candidate { verified: Some(value), .. } if value.installed.job_id == job => Some(value),
                _ => None,
            }
        });
        let result = matches.next()?;
        if matches.next().is_some() { return None; }
        Some(result.selection(owner))
    }
}
fn safe_child(value: &Value, prefix: &str) -> bool {
    value.as_str().is_some_and(|s| s.starts_with(prefix) && s.len() > prefix.len() && s.len() <= 128
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}
fn workspace(value: &Value) -> bool {
    value.as_str().is_some_and(|s| s.strip_prefix("work-").is_some_and(|s| {
        s.len() == 32 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }))
}
fn locator(value: &Value) -> bool {
    exact_keys(value, &["setup", "workspace", "job_id", "request_sha256", "checkpoint_sha256"])
        && safe_child(&value["setup"], "setup-") && workspace(&value["workspace"])
        && hex(value, "job_id", 32).is_some() && hex(value, "request_sha256", 64).is_some()
        && hex(value, "checkpoint_sha256", 64).is_some()
}
fn cursor(value: &Value) -> bool {
    exact_keys(value, &["setup", "workspace", "after_job"])
        && safe_child(&value["setup"], "setup-")
        && (value["workspace"] == "" || workspace(&value["workspace"]))
        && (value["after_job"] == "" || hex(value, "after_job", 32).is_some())
}
fn family_profile(value: &Value) -> Result<(&str, &str), ToolError> {
    let family = value["family"].as_str().ok_or_else(unconfirmed)?;
    let profile = operation_name(&value["profile"])?;
    if !matches!(family, "agno" | "services" | "activepieces")
        && !matches!((family, profile), ("selected-python", "python") | ("selected-node", "node")) {
        return Err(unconfirmed());
    }
    Ok((family, profile))
}
fn handle(session: &SessionId, kind: &str, value: &Value) -> Result<String, ToolError> {
    Ok(format!("{kind}-{:x}", Sha256::digest(serde_json::to_vec(&json!([session, kind, value])).map_err(|_| unconfirmed())?)))
}
fn current(state: &State, realm: &RealmLocator) -> Result<(), ToolError> {
    if state.retired || state.realm.as_ref() != Some(realm) { return Err(unconfirmed()); }
    Ok(())
}

impl CheckedInstalls {
    pub(crate) fn retained_list_request(&self, realm: &RealmLocator, session: &SessionId, after: Option<&str>)
        -> Result<Value, ToolError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        current(&state, realm)?;
        let cursor = match after {
            None => Value::Null,
            Some(key) => match state.returning.entries.get(key) {
                Some(Entry {session: owner, value: Retained::Cursor(value), ..}) if owner == session => value.clone(),
                _ => return Err(unconfirmed()),
            },
        };
        Ok(json!({"realm":realm.realm.as_str(), "realm_state_root":realm.state_root,
            "retained_installations":{"action":"list", "cursor":cursor}}))
    }
    pub(crate) fn retain_candidates(&self, realm: &RealmLocator, session: &SessionId, page: Value)
        -> Result<Value, ToolError> {
        if serde_json::to_vec(&page).map_err(|_| unconfirmed())?.len() > MAX_DOCUMENT
            || !exact_keys(&page, &["kind", "candidates", "next_cursor", "unconfirmed"])
            || page["kind"] != "retained_installation_page" || !page["unconfirmed"].is_boolean() {
            return Err(unconfirmed());
        }
        let rows = page["candidates"].as_array().filter(|v| v.len() <= 32).ok_or_else(unconfirmed)?;
        let mut pending = BTreeMap::new();
        let mut public = Vec::new();
        for row in rows {
            if !exact_keys(row, &["locator", "family", "profile"]) || !locator(&row["locator"]) { return Err(unconfirmed()); }
            let (family, profile) = family_profile(row)?;
            let key = handle(session, "installed", &row["locator"])?;
            let bytes = serde_json::to_vec(row).map_err(|_| unconfirmed())?.len();
            if pending.insert(key.clone(), Entry { session: session.clone(), bytes, value: Retained::Candidate {
                locator: row["locator"].clone(), family: family.into(), profile: profile.into(), verified: None,
            }}).is_some() { return Err(unconfirmed()); }
            public.push(json!({"candidate":key,"family":family,"profile":profile}));
        }
        let next = if page["next_cursor"].is_null() { None } else {
            if !cursor(&page["next_cursor"]) { return Err(unconfirmed()); }
            let key = handle(session, "page", &page["next_cursor"])?;
            let bytes = serde_json::to_vec(&page["next_cursor"]).map_err(|_| unconfirmed())?.len();
            pending.insert(key.clone(), Entry {session:session.clone(), bytes, value:Retained::Cursor(page["next_cursor"].clone())});
            Some(key)
        };
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        current(&state, realm)?;
        let added: Vec<_> = pending.into_iter().filter(|(key, _)| !state.returning.entries.contains_key(key)).collect();
        let bytes: usize = added.iter().map(|(_, entry)| entry.bytes).sum();
        if state.returning.entries.len() + added.len() > MAX_PROPOSALS || state.returning.bytes + bytes > MAX_RETAINED { return Err(unconfirmed()); }
        state.returning.bytes += bytes;
        state.returning.entries.extend(added);
        Ok(json!({"state":"retained_requests", "candidates":public, "next":next, "unconfirmed":page["unconfirmed"],
            "notice":"These are retained package requests, not verified installations or configured services. Select one candidate for current package verification; no previous approval or install permission is restored."}))
    }
    pub(crate) fn retained_select_request(&self, realm: &RealmLocator, session: &SessionId, key: &str)
        -> Result<Value, ToolError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        current(&state, realm)?;
        let entry = state.returning.entries.get_mut(key).filter(|e| e.session == *session).ok_or_else(unconfirmed)?;
        let Retained::Candidate {locator, verified, ..} = &mut entry.value else { return Err(unconfirmed()); };
        // A fresh read replaces only this candidate's observation. Failure must
        // not leave that older selection usable or erase another service.
        let removed = verified.take().map_or(0, |value| value.bytes);
        entry.bytes -= removed;
        let request = json!({"realm":realm.realm.as_str(),"realm_state_root":realm.state_root,
            "retained_installations":{"action":"select","locator":locator}});
        state.returning.bytes -= removed;
        Ok(request)
    }
    pub(crate) fn checked_retained(&self, realm: &RealmLocator, session: &SessionId, key: &str, response: &Value)
        -> Result<VerifiedRetained, ToolError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        current(&state, realm)?;
        let entry = state.returning.entries.get(key).filter(|e| e.session == *session).ok_or_else(unconfirmed)?;
        let Retained::Candidate {locator: expected, family, profile, ..} = &entry.value else { return Err(unconfirmed()); };
        let bytes = serde_json::to_vec(response).map_err(|_| unconfirmed())?.len();
        if bytes > MAX_DOCUMENT || !exact_keys(response, &["kind", "locator", "installation", "metadata"])
            || response["kind"] != "retained_installation_readback" || response["locator"] != *expected { return Err(unconfirmed()); }
        let result = &response["installation"];
        if result["scope"] != READBACK_SCOPE || result["family"] != *family || result["profile"] != *profile
            || result["job_id"] != expected["job_id"] || result["recovery_request_sha256"] != expected["request_sha256"] {
            return Err(unconfirmed());
        }
        let job = hex(result, "job_id", 32).ok_or_else(unconfirmed)?;
        let validation = hex(result, "validation_id", 32).ok_or_else(unconfirmed)?;
        let (mut installed, configuration, native_connector) = installed_launch(InstalledIdentity {
            realm, session, job_id:job, validation_id:validation, family, profile, mcp_name:None,
        }, result, true, None)?;
        installed.recovery_request_sha256 = Some(hex(result, "recovery_request_sha256", 64).ok_or_else(unconfirmed)?.into());
        let envelope = &response["metadata"];
        let (metadata, operations, references) = match envelope["kind"].as_str() {
            Some("library") if matches!(family.as_str(), "agno" | "services" | "activepieces") => {
                if !exact_keys(envelope, &["kind", "selection", "help", "configuration_check"])
                    || installed.configuration_sha256.is_none() { return Err(unconfirmed()); }
                let selection = &envelope["selection"];
                let help = &envelope["help"];
                if !exact_keys(selection, &["family", "profile", "operations", "values"])
                    || selection["family"] != *family || selection["profile"] != *profile
                    || help["family"] != *family || help["profile"] != *profile { return Err(unconfirmed()); }
                let metadata = publisher_metadata(help)?.ok_or_else(unconfirmed)?;
                let ids = selection["operations"].as_array().filter(|v| !v.is_empty() && v.len() <= 128).ok_or_else(unconfirmed)?;
                let operations = ids.iter().map(|id| metadata.names().get(id.as_str().ok_or_else(unconfirmed)?)
                    .cloned().ok_or_else(unconfirmed)).collect::<Result<BTreeSet<_>, _>>()?;
                if operations.len() != ids.len() { return Err(unconfirmed()); }
                let references = credential_references(selection, envelope["configuration_check"].get("credential_environment_references"), &metadata)?;
                (metadata, operations, references)
            },
            Some("selected") if matches!(family.as_str(), "selected-python" | "selected-node") => {
                if !exact_keys(envelope, &["kind", "declaration"]) { return Err(unconfirmed()); }
                let declaration = &envelope["declaration"];
                if declaration["profile"] != *profile || result["selection_kind"] != *profile
                    || result["declared_service"] != declaration["declared_service"]
                    || result["effect_sha256"] != declaration["effect_sha256"]
                    || result.get("package_sha256").is_some_and(|hash| hash != &declaration["files"]["package.json"])
                    || SelectedDocument::from_value(result)? != SelectedDocument::from_value(declaration)?
                    || result.get("plugin_source") != declaration.get("plugin_source")
                    || result.get("acquired_source") != declaration.get("acquired_source") { return Err(unconfirmed()); }
                let metadata = selected_service_metadata(declaration)?.ok_or_else(unconfirmed)?;
                (metadata.clone(), metadata.names().values().cloned().collect(), None)
            },
            _ => return Err(unconfirmed()),
        };
        Ok(VerifiedRetained {installed, configuration, metadata, operations, native_connector, references, bytes})
    }
    pub(crate) fn admit_retained(self: &Arc<Self>, realm: &RealmLocator, session: &SessionId, key: &str, verified: VerifiedRetained)
        -> Result<InstalledConnectorSelection, ToolError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        current(&state, realm)?;
        if verified.installed.realm != *realm || verified.installed.session != *session
            || state.returning.bytes + verified.bytes > MAX_RETAINED
            || state.proposals.values().filter_map(|p| p.readback.as_ref()).any(|readback| {
                readback.session == *session && readback.job.lock().is_ok_and(|job| job.as_deref() == Some(verified.installed.job_id.as_str()))
            }) { return Err(unconfirmed()); }
        for (other, entry) in &state.returning.entries {
            if other != key && entry.session == *session && matches!(&entry.value,
                Retained::Candidate {verified:Some(v), ..} if v.installed.job_id == verified.installed.job_id) { return Err(unconfirmed()); }
        }
        let entry = state.returning.entries.get_mut(key).filter(|e| e.session == *session).ok_or_else(unconfirmed)?;
        let Retained::Candidate {locator, family, profile, verified: slot} = &mut entry.value else { return Err(unconfirmed()); };
        if slot.is_some() || locator["job_id"] != verified.installed.job_id || *family != verified.installed.family
            || *profile != verified.installed.profile || locator["request_sha256"].as_str() != verified.installed.recovery_request_sha256.as_deref() {
            return Err(unconfirmed());
        }
        let result = verified.selection(self);
        let bytes = verified.bytes;
        *slot = Some(verified);
        entry.bytes += bytes;
        state.returning.bytes += bytes;
        Ok(result)
    }
}

#[cfg(test)]
mod controls;
