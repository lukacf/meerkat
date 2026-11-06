//! One SetupStore record owns membership and exact physical outcome delivery.
//! Native WorkGraph owns lifecycle; this record neither grants effect entry
//! nor makes a Controller job ready for any particular acquisition.
use super::*;
use std::collections::BTreeMap;

const SCHEMA: &str = "meerkat_toolkit.shared_preparation";
const MAX_MEMBERS: usize = 128;
const CAS_ATTEMPTS: usize = 16;

/// Complete private observation from the host-owned physical Controller.
/// Construct only from its inherited channel, after the native selected check.
/// The raw request remains part of equality, including account/reference
/// context; artifact name or digest alone never identifies shared work.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPreparationSelection {
    controller: String,
    arguments_sha256: String,
    arguments: serde_json::Value,
    effect: serde_json::Value,
    plan: serde_json::Value,
    result: serde_json::Value,
}
impl SharedPreparationSelection {
    pub fn from_controller(value: serde_json::Value) -> Result<Self, SetupError> {
        let selected: Self = serde_json::from_value(value)?;
        selected.check()?;
        Ok(selected)
    }
    pub fn arguments(&self) -> &serde_json::Value {
        &self.arguments
    }
    pub fn validation_id(&self) -> Result<&str, SetupError> {
        self.result
            .get("validation_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                value.len() == 32
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .ok_or(SetupError::InvalidSelection)
    }
    fn check(&self) -> Result<(), SetupError> {
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        };
        if !hash(&self.controller)
            || !hash(&self.arguments_sha256)
            || !self.arguments.is_object()
            || !self.effect.is_object()
            || !self.plan.is_object()
            || !self.result.is_object()
            || canonical_json(self)?.len() > 2 * 1024 * 1024
        {
            return Err(SetupError::InvalidSelection);
        }
        self.validation_id()?;
        Ok(())
    }
}

/// Exact terminal physical observation from the live Controller after its
/// worker joined. This is software evidence, never native Ready or admission.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPreparationCompletion {
    controller: String,
    validation_id: String,
    job: serde_json::Value,
}
impl SharedPreparationCompletion {
    pub fn from_controller(value: serde_json::Value) -> Result<Self, SetupError> {
        let value: Self = serde_json::from_value(value)?;
        value.check_shape()?;
        Ok(value)
    }
    fn check_shape(&self) -> Result<(), SetupError> {
        if self.controller.len() != 64
            || self.validation_id.len() != 32
            || !self
                .controller
                .bytes()
                .chain(self.validation_id.bytes())
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || self.job.as_object().is_none_or(|job| job.len() != 6)
            || self.job["id"].as_str().is_none_or(|id| {
                id.len() != 32
                    || !id
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            || self.job["kind"] != "install"
            || !matches!(
                self.job["status"].as_str(),
                Some("succeeded" | "unconfirmed")
            )
            || !(self.job["result"].is_null() || self.job["result"].is_object())
            || !(self.job["error"].is_null() || self.job["error"].is_string())
            || canonical_json(self)?.len() > 2 * 1024 * 1024
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(())
    }
    fn check_selection(&self, selected: &SharedPreparationSelection) -> Result<(), SetupError> {
        self.check_shape()?;
        if self.controller != selected.controller
            || self.validation_id != selected.validation_id()?
            || !matches!(
                selected.effect["request"]["profile"].as_str(),
                Some("python" | "node" | "mcpb")
            )
            || self.job["family"]
                != format!(
                    "selected-{}",
                    selected.effect["request"]["profile"].as_str().unwrap()
                )
        {
            return Err(SetupError::WrongScope);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedDelivery {
    evidence: ContinuationAdmissionEvidence,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    backend: SetupWorkBackendPin,
    parent: WorkItemRef,
    parent_owner: WorkOwnerKey,
    cycle_label: String,
}
impl From<&SetupWorkContext> for Group {
    fn from(value: &SetupWorkContext) -> Self {
        Self {
            backend: value.backend.clone(),
            parent: value.parent.clone(),
            parent_owner: value.parent_owner.clone(),
            cycle_label: value.cycle_label.clone(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    acquisition: StoreKey,
    input_digest: ContentDigest,
    source: StoreKey,
    work: SetupWorkContext,
    helper: meerkat_core::MobMemberBinding,
    leaving: bool,
    left: bool,
    delivered: Option<SharedDelivery>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    group: Group,
    selection: SharedPreparationSelection,
    members: BTreeMap<String, Member>,
    /// Seal the physical result. Late members join against this same immutable
    /// value through the member CAS and still need their own reviewed delivery.
    completion: Option<SharedPreparationCompletion>,
    /// Last leave seals the exact effect against late joins before stop.
    closed: bool,
}

/// Locator retained by the existing source owner. Reopening it requires its
/// exact member and source records; the locator is never an entry permit.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPreparationMember {
    key: StoreKey,
    member: String,
    identity: Member,
}
impl SharedPreparationMember {
    pub fn key(&self) -> &StoreKey {
        &self.key
    }
    pub fn acquisition(&self) -> &StoreKey {
        &self.identity.acquisition
    }
    pub fn source(&self) -> &StoreKey {
        &self.identity.source
    }
    pub fn helper(&self) -> &meerkat_core::MobMemberBinding {
        &self.identity.helper
    }
}

impl SetupStore<'_> {
    /// Read the exact historical selection for an existing source member.
    /// Leaving, left and closed records remain readable for physical cleanup;
    /// this observation neither rejoins P nor permits completion or delivery.
    pub async fn shared_preparation_selection(
        &self,
        member: &SharedPreparationMember,
    ) -> Result<SharedPreparationSelection, SetupError> {
        let stored = self
            .store
            .get(&member.key)
            .await?
            .ok_or(SetupError::InvalidRecord)?;
        let record = decode(&stored)?;
        self.check_shared_member(member, &record.selection)?;
        check_selection(&record, member, &record.selection)?;
        let current = record
            .members
            .get(&member.member)
            .ok_or(SetupError::WrongScope)?;
        if !same_member(current, &member.identity) {
            return Err(SetupError::WrongScope);
        }
        Ok(record.selection)
    }

    /// Observe an exact member's completed canonical leave. This says nothing
    /// about physical cleanup, which still requires the selected resource owner.
    pub async fn shared_preparation_left_selection(
        &self,
        member: &SharedPreparationMember,
    ) -> Result<(SharedPreparationSelection, bool), SetupError> {
        let stored = self
            .store
            .get(&member.key)
            .await?
            .ok_or(SetupError::InvalidRecord)?;
        let record = decode(&stored)?;
        self.check_shared_member(member, &record.selection)?;
        check_selection(&record, member, &record.selection)?;
        let current = record
            .members
            .get(&member.member)
            .ok_or(SetupError::WrongScope)?;
        if !same_member(current, &member.identity) || !current.left {
            return Err(SetupError::Denied);
        }
        let last = record.members.values().all(|member| member.left);
        Ok((record.selection, last))
    }

    /// Derive the locator before joining, so the existing source can retain
    /// cleanup custody even when the subsequent CAS answer is lost. Caller
    /// holds that source's writer; native admission remains a later boundary.
    pub fn shared_preparation_member(
        &self,
        snapshot: &SetupSnapshot,
        source: StoreKey,
        selection: &SharedPreparationSelection,
    ) -> Result<SharedPreparationMember, SetupError> {
        self.check_snapshot_scope(snapshot)?;
        selection.check()?;
        let host = snapshot.request().host();
        let work = host.work_context.as_ref().ok_or(SetupError::Denied)?;
        work.check_structure()?;
        let helper = host.setup_helper.clone().ok_or(SetupError::Denied)?;
        if source.realm != snapshot.key().realm {
            return Err(SetupError::WrongScope);
        }
        let key = StoreKey {
            realm: snapshot.key().realm.clone(),
            owner: format!("{}/shared-preparation", snapshot.key().owner),
            id: digest(&(Group::from(work), selection))?.to_string(),
        };
        let identity = Member {
            acquisition: snapshot.key().clone(),
            input_digest: snapshot.request().input_digest().clone(),
            source,
            work: work.clone(),
            helper,
            leaving: false,
            left: false,
            delivered: None,
        };
        Ok(SharedPreparationMember {
            key,
            member: digest(&identity)?.to_string(),
            identity,
        })
    }

    /// Single-record CAS linearizes join against leave/completion sealing.
    /// Rechecks and source gate registration belong to the calling source
    /// owner; this method does not claim a transaction across those owners.
    pub async fn join_shared_preparation(
        &self,
        member: &SharedPreparationMember,
        selection: &SharedPreparationSelection,
        backend: &SetupWorkBackend,
    ) -> Result<(), SetupError> {
        self.check_shared_member(member, selection)?;
        member.identity.work.check_current(backend).await?;
        for _ in 0..CAS_ATTEMPTS {
            let prior = self.store.get(&member.key).await?;
            if let Some(stored) = prior {
                let mut record = decode(&stored)?;
                check_selection(&record, member, selection)?;
                if let Some(existing) = record.members.get(&member.member) {
                    if !same_member(existing, &member.identity) || existing.leaving || existing.left
                    {
                        return Err(SetupError::Denied);
                    }
                    return Ok(());
                }
                if record.closed || record.members.len() >= MAX_MEMBERS {
                    return Err(SetupError::Denied);
                }
                record
                    .members
                    .insert(member.member.clone(), member.identity.clone());
                match self.shared_commit(&stored, record, "member_joined").await {
                    Ok(_) => return Ok(()),
                    Err(SetupError::Store(StoreError::Conflict)) => continue,
                    Err(error) => return Err(error),
                }
            } else {
                let record = Preparation {
                    group: Group::from(&member.identity.work),
                    selection: selection.clone(),
                    members: [(member.member.clone(), member.identity.clone())].into(),
                    completion: None,
                    closed: false,
                };
                match self
                    .store
                    .admit(AdmissionRequest {
                        key: member.key.clone(),
                        admission_key: member.key.id.clone(),
                        data: encode(&record)?,
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                {
                    Ok(AdmissionOutcome::Created(_)) => return Ok(()),
                    Ok(AdmissionOutcome::Existing(_))
                    | Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => continue,
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Err(StoreError::Conflict.into())
    }

    pub async fn check_shared_preparation(
        &self,
        member: &SharedPreparationMember,
        backend: &SetupWorkBackend,
    ) -> Result<(), SetupError> {
        let stored = self
            .store
            .get(&member.key)
            .await?
            .ok_or(StoreError::NotFound)?;
        let record = decode(&stored)?;
        self.check_shared_member(member, &record.selection)?;
        let current = record
            .members
            .get(&member.member)
            .ok_or(SetupError::Denied)?;
        if record.closed
            || current.leaving
            || current.left
            || !same_member(current, &member.identity)
        {
            return Err(SetupError::Denied);
        }
        member.identity.work.check_current(backend).await
    }

    /// Called with the exact live resource's post-join observation while the
    /// existing source writer fences this member. The first physical result
    /// stays immutable across late joins, caller loss and reconciliation.
    pub async fn complete_shared_preparation(
        &self,
        member: &SharedPreparationMember,
        completion: &SharedPreparationCompletion,
        backend: &SetupWorkBackend,
    ) -> Result<(), SetupError> {
        member.identity.work.check_current(backend).await?;
        for _ in 0..CAS_ATTEMPTS {
            let stored = self
                .store
                .get(&member.key)
                .await?
                .ok_or(StoreError::NotFound)?;
            let mut record = decode(&stored)?;
            self.check_shared_member(member, &record.selection)?;
            completion.check_selection(&record.selection)?;
            let current = record
                .members
                .get(&member.member)
                .ok_or(SetupError::Denied)?;
            if record.closed
                || current.leaving
                || current.left
                || !same_member(current, &member.identity)
            {
                return Err(SetupError::Denied);
            }
            if let Some(prior) = &record.completion {
                return if prior == completion {
                    Ok(())
                } else {
                    Err(SetupError::InvalidRecord)
                };
            }
            record.completion = Some(completion.clone());
            match self
                .shared_commit(&stored, record, "physical_completion_observed")
                .await
            {
                Ok(_) => return Ok(()),
                Err(SetupError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Conflict.into())
    }

    /// Called only by the native continuation owner after authenticating this
    /// exact admission, before its existing outbox acknowledges delivery. A
    /// lost P write therefore remains ordinary outbox reconciliation. Physical
    /// completion is a separate fact and never follows from this receipt.
    pub async fn acknowledge_shared_preparation_delivery(
        &self,
        member: &SharedPreparationMember,
        evidence: &ContinuationAdmissionEvidence,
    ) -> Result<(), SetupError> {
        let acquisition = self
            .store
            .get(member.acquisition())
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(acquisition)?;
        let obligation = continuation_obligation(&snapshot.record, &snapshot.request)?;
        if snapshot.request.input_digest() != &member.identity.input_digest
            || snapshot
                .record
                .continuation_receipt
                .as_ref()
                .is_some_and(|canonical| !evidence.confirms(canonical))
        {
            return Err(SetupError::InvalidRecord);
        }
        evidence.check(&obligation, &evidence.receipt)?;
        for _ in 0..CAS_ATTEMPTS {
            let stored = self
                .store
                .get(&member.key)
                .await?
                .ok_or(StoreError::NotFound)?;
            let mut record = decode(&stored)?;
            self.check_shared_member(member, &record.selection)?;
            let current = record
                .members
                .get_mut(&member.member)
                .ok_or(SetupError::Denied)?;
            if !same_member(current, &member.identity) {
                return Err(SetupError::Denied);
            }
            // A receipt authenticated by the native owner remains historical
            // admission even after leave. Recording it never reopens the row.
            let delivery = SharedDelivery {
                evidence: evidence.clone(),
            };
            if let Some(prior) = &current.delivered {
                return if evidence.confirms(&prior.evidence) {
                    Ok(())
                } else {
                    Err(SetupError::InvalidRecord)
                };
            }
            current.delivered = Some(delivery);
            match self
                .shared_commit(&stored, record, "member_delivery_observed")
                .await
            {
                Ok(_) => return Ok(()),
                Err(SetupError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Conflict.into())
    }

    /// Publish member closure while the source writer still fences new gate
    /// reservations. This is pending leave, not removal or physical stop.
    pub async fn begin_shared_preparation_leave(
        &self,
        member: &SharedPreparationMember,
    ) -> Result<(), SetupError> {
        for _ in 0..CAS_ATTEMPTS {
            let Some(stored) = self.store.get(&member.key).await? else {
                return Ok(());
            };
            let mut record = decode(&stored)?;
            self.check_shared_member(member, &record.selection)?;
            let Some(current) = record.members.get_mut(&member.member) else {
                return Ok(());
            };
            if !same_member(current, &member.identity) {
                return Err(SetupError::Denied);
            }
            if current.leaving || current.left {
                return Ok(());
            }
            current.leaving = true;
            match self.shared_commit(&stored, record, "member_closing").await {
                Ok(_) => return Ok(()),
                Err(SetupError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Conflict.into())
    }

    /// Close/drain the local admission gate before this durable leave. True
    /// means this member belongs to a durably sealed empty preparation; the
    /// caller still owns physical stop and join. An absent record or member
    /// grants no last-member authority, even if another member joins later.
    pub async fn leave_shared_preparation(
        &self,
        member: &SharedPreparationMember,
    ) -> Result<bool, SetupError> {
        for _ in 0..CAS_ATTEMPTS {
            let Some(stored) = self.store.get(&member.key).await? else {
                return Ok(false);
            };
            let mut record = decode(&stored)?;
            self.check_shared_member(member, &record.selection)?;
            let Some(current) = record.members.get_mut(&member.member) else {
                return Ok(false);
            };
            if !same_member(current, &member.identity) {
                return Err(SetupError::Denied);
            }
            if current.left {
                return Ok(record.members.values().all(|value| value.left));
            }
            if !current.leaving {
                return Err(SetupError::Denied);
            }
            current.left = true;
            let last = record.members.values().all(|value| value.left);
            record.closed |= last;
            match self.shared_commit(&stored, record, "member_left").await {
                Ok(_) => return Ok(last),
                Err(SetupError::Store(StoreError::Conflict)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(StoreError::Conflict.into())
    }

    fn check_shared_member(
        &self,
        member: &SharedPreparationMember,
        selection: &SharedPreparationSelection,
    ) -> Result<(), SetupError> {
        selection.check()?;
        member.identity.work.check_structure()?;
        let scope = self
            .scope
            .key(&Id::new("shared-scope").map_err(|_| SetupError::InvalidSelection)?);
        if member.key.realm != scope.realm
            || member.key.owner != format!("{}/shared-preparation", scope.owner)
            || member.identity.acquisition.realm != scope.realm
            || member.identity.acquisition.owner != scope.owner
            || member.key.id != digest(&(Group::from(&member.identity.work), selection))?.as_str()
            || member.member != digest(&member.identity)?.as_str()
            || member.identity.leaving
            || member.identity.left
            || member.identity.delivered.is_some()
        {
            return Err(SetupError::WrongScope);
        }
        Ok(())
    }

    async fn shared_commit(
        &self,
        stored: &StoredRecord,
        record: Preparation,
        kind: &str,
    ) -> Result<StoredRecord, SetupError> {
        let data = encode(&record)?;
        Ok(self
            .store
            .compare_and_swap(Transition {
                key: stored.key.clone(),
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest.clone()),
                claim: None,
                data: data.clone(),
                events: vec![EventWrite {
                    kind: kind.into(),
                    data,
                }],
                effects: vec![],
            })
            .await?)
    }
}

fn same_member(current: &Member, identity: &Member) -> bool {
    let mut observed = current.clone();
    observed.leaving = false;
    observed.left = false;
    observed.delivered = None;
    &observed == identity
}
fn check_selection(
    record: &Preparation,
    member: &SharedPreparationMember,
    selection: &SharedPreparationSelection,
) -> Result<(), SetupError> {
    if record.group != Group::from(&member.identity.work) || &record.selection != selection {
        return Err(SetupError::WrongScope);
    }
    Ok(())
}
fn encode(record: &Preparation) -> Result<RecordData, SetupError> {
    Ok(RecordData {
        schema: SCHEMA.into(),
        version: 1,
        body: serde_json::to_value(record)?,
    })
}
fn decode(stored: &StoredRecord) -> Result<Preparation, SetupError> {
    if stored.data.schema != SCHEMA
        || stored.data.version != 1
        || stored.content_digest != stored.data.digest()?
    {
        return Err(SetupError::InvalidRecord);
    }
    let record: Preparation = serde_json::from_value(stored.data.body.clone())?;
    if record.members.is_empty() || record.members.len() > MAX_MEMBERS {
        return Err(SetupError::InvalidRecord);
    }
    record.selection.check()?;
    if let Some(completion) = &record.completion {
        completion.check_selection(&record.selection)?;
    }
    if record.closed != record.members.values().all(|value| value.left) {
        return Err(SetupError::InvalidRecord);
    }
    for (id, member) in &record.members {
        member.work.check_structure()?;
        if member.left && !member.leaving {
            return Err(SetupError::InvalidRecord);
        }
        let mut identity = member.clone();
        identity.leaving = false;
        identity.left = false;
        identity.delivered = None;
        if *id != digest(&identity)?.as_str() || Group::from(&member.work) != record.group {
            return Err(SetupError::InvalidRecord);
        }
    }
    Ok(record)
}
