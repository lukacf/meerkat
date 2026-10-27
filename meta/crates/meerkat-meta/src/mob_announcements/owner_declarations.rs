//! The owner's declarations engine (contract 4.4 to 4.6): an apply's
//! revision checks, the reconcile of the records declarations manage
//! through the owner's own put paths, and the members of role templates,
//! resolved at each capture and reconcile.
//!
//! Every record a declaration makes is marked with the digest of its
//! declared form. The reconciler creates, replaces, ends and cancels only
//! records it marked as made; a runtime refusal leaves the record pending
//! with its typed refusal until the next trigger (an apply, a mob set
//! change, compose).

use std::collections::{BTreeMap, BTreeSet};

use crate::mob_announcements::declarations::{
    DeclarationRecordKind, DeclarationRecordView, DeclarationRefusal, DeclarationStatus,
    Declarations, DeclarationsSource, DeclarationsView, DeclaredCollection, DeclaredDelivery,
    DeclaredSource, DeclaredSubscription, read_sidecar, validate,
};
use crate::mob_announcements::declarations::{PutRoleTemplate, RoleTemplate};
use crate::mob_announcements::native::MemberRoute;
use crate::mob_announcements::sources::RosterFacts;
use crate::mob_announcements::sources::{SourceChange, SourceHealthStatus, SourceState};
use crate::mob_announcements::store::{
    AppliedDeclarations, DeclarationMark, RoleChildStatus, RoleExpansion, RoleExpansions,
    RoleMember, SubscriptionVerbOutcome, TemplateChildren,
};
use crate::mob_announcements::types::{
    AnnouncementStoreError, Member, MemberRouteRecord, MobId, OperationId, SubscriptionChange,
    SubscriptionId, SubscriptionState, sha256_hex,
};

use super::{AnnouncementError, AnnouncementService, member_problem};

/// The refusal a declared record or a role child is pending with: the
/// typed kind the same refusal carries on the wire, and its message.
fn refusal_from(error: &AnnouncementError) -> DeclarationRefusal {
    DeclarationRefusal {
        kind: crate::mob_announcements::rpc::refusal_kind(error),
        message: error.to_string(),
    }
}

/// A record made at runtime holds the id the declaration names: it is never
/// changed, and the declaration takes the id once that record is gone or
/// ended.
fn unmanaged_record(kind: DeclarationRecordKind, id: &str) -> DeclarationRefusal {
    DeclarationRefusal {
        kind: "unmanaged_record".to_string(),
        message: format!(
            "the {} {id} was made at runtime; the declaration never changes it, and takes the \
             id once that record is gone or ended",
            kind.as_str()
        ),
    }
}

/// Only a failure of the store itself stops a reconcile; every other
/// refusal leaves its record pending until the next trigger.
fn store_failure(error: &AnnouncementError) -> bool {
    matches!(
        error,
        AnnouncementError::Store(AnnouncementStoreError::Storage(_))
    )
}

fn invalid(message: &str) -> AnnouncementError {
    AnnouncementError::Store(AnnouncementStoreError::Invalid(message.to_string()))
}

/// The operation id of one reconcile step: derived from the record, its
/// declared digest and the revision it expects, so a reconcile run again
/// after a crash meets the receipt of the step it already committed.
fn step_operation(
    step: &str,
    id: &str,
    digest: &str,
    expected: Option<u64>,
) -> Result<OperationId, AnnouncementError> {
    let expected = expected.map_or_else(|| "none".to_string(), |revision| revision.to_string());
    let digest = sha256_hex(&[
        b"declarations-step",
        step.as_bytes(),
        id.as_bytes(),
        digest.as_bytes(),
        expected.as_bytes(),
    ]);
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    Ok(OperationId::new(format!("declared-{step}-{}", &hex[..32]))?)
}

/// The keys of a mark: its kind and id.
type MarkKey = (DeclarationRecordKind, String);

/// Whether the reconciler made the record of `before` and applied, or held,
/// the declaration of `digest`: only then does an operator's cancel or end
/// hold. A pending mark of the digest is a refused attempt, tried again.
fn held_at(before: Option<&DeclarationMark>, digest: &str) -> bool {
    before.is_some_and(|mark| {
        mark.managed
            && mark.digest == digest
            && matches!(
                mark.status,
                DeclarationStatus::Applied | DeclarationStatus::Suppressed
            )
    })
}

impl AnnouncementService {
    /// This gateway's own mob: a declared record names it when it names no
    /// mob.
    fn declaring_mob(&self) -> Result<MobId, AnnouncementError> {
        self.inner
            .native
            .own_mob()
            .ok_or_else(|| invalid("declarations need this gateway's own mob"))
    }

    /// `declarations/apply`: the declarations an apply carries (a gateway
    /// with inline declarations or none), or its sidecar read again.
    pub(super) async fn apply_declared(
        &self,
        declarations: Option<Declarations>,
    ) -> Result<DeclarationsView, AnnouncementError> {
        let own = self.declaring_mob()?;
        let sidecar = self
            .inner
            .config
            .declarations
            .as_deref()
            .and_then(DeclarationsSource::sidecar);
        let declarations = match (declarations, sidecar) {
            (Some(declarations), None) => {
                validate(&declarations, &own)?;
                declarations
            }
            (None, Some(path)) => read_sidecar(path, &own)?,
            (Some(_), Some(_)) => {
                return Err(invalid(
                    "this gateway's declarations come from its sidecar: an apply carries none",
                ));
            }
            (None, None) => {
                return Err(invalid(
                    "an apply carries the revision and the declarations",
                ));
            }
        };
        {
            let _gate = self.inner.declarations_gate.lock().await;
            self.record_applied(&declarations, &own).await?;
            self.reconcile_applied(&own).await?;
            self.reconcile_roles().await?;
        }
        self.declared_view().await
    }

    /// Reconcile at a trigger: the options' declarations when they are newer
    /// than the applied ones (at compose, the first time), the applied ones,
    /// and every role template's children.
    pub(super) async fn reconcile_declared(&self) -> Result<(), AnnouncementError> {
        let _gate = self.inner.declarations_gate.lock().await;
        if let Some(own) = self.inner.native.own_mob() {
            if let Some(source) = self.inner.config.declarations.as_deref() {
                let declarations = source.declarations();
                match self.inner.store.applied_declarations().await? {
                    Some(applied) if declarations.revision < applied.revision => {}
                    Some(applied) if declarations.revision == applied.revision => {
                        // An apply at runtime reached this revision with
                        // other content: it stands, and the boot says so once.
                        let digest = declarations.digest(&own)?;
                        if digest != applied.digest
                            && !self
                                .inner
                                .options_conflict_warned
                                .swap(true, std::sync::atomic::Ordering::SeqCst)
                        {
                            tracing::warn!(
                                revision = applied.revision,
                                applied_digest = %applied.digest,
                                options_digest = %digest,
                                "mob announcement options declare the applied revision with \
                                 other content; the applied declarations stand"
                            );
                        }
                    }
                    _ => self.record_applied(declarations, &own).await?,
                }
            }
            self.reconcile_applied(&own).await?;
        }
        self.reconcile_roles().await
    }

    /// Check `declarations` against the applied revision and record them: a
    /// lower revision is a rollback, the applied revision with another
    /// digest a conflict, and with the same digest nothing new.
    async fn record_applied(
        &self,
        declarations: &Declarations,
        own: &MobId,
    ) -> Result<(), AnnouncementError> {
        let digest = declarations.digest(own)?;
        if let Some(applied) = self.inner.store.applied_declarations().await? {
            if declarations.revision < applied.revision {
                return Err(AnnouncementStoreError::DeclarationRollback {
                    applied: applied.revision,
                }
                .into());
            }
            if declarations.revision == applied.revision {
                if digest != applied.digest {
                    return Err(AnnouncementStoreError::DeclarationDigestConflict {
                        revision: applied.revision,
                    }
                    .into());
                }
                return Ok(());
            }
        }
        self.inner
            .store
            .record_declarations(AppliedDeclarations {
                revision: declarations.revision,
                digest,
                applied_at_ms: self.now(),
                declarations: declarations.clone(),
            })
            .await?;
        Ok(())
    }

    /// Bring the records the applied declarations manage in line with them:
    /// dropped records first (ended or cancelled), then sources, then
    /// subscriptions and role templates.
    async fn reconcile_applied(&self, own: &MobId) -> Result<(), AnnouncementError> {
        let Some(applied) = self.inner.store.applied_declarations().await? else {
            return Ok(());
        };
        let mut marks: BTreeMap<MarkKey, DeclarationMark> = self
            .inner
            .store
            .declaration_marks()
            .await?
            .into_iter()
            .map(|mark| ((mark.kind, mark.id.clone()), mark))
            .collect();
        let declared: BTreeSet<MarkKey> = applied
            .declarations
            .sources
            .iter()
            .map(|source| (DeclarationRecordKind::Source, source.source_id.to_string()))
            .chain(
                applied
                    .declarations
                    .subscriptions
                    .iter()
                    .map(|subscription| {
                        (
                            DeclarationRecordKind::Subscription,
                            subscription.subscription_id.to_string(),
                        )
                    }),
            )
            .collect();
        let dropped = marks
            .keys()
            .filter(|key| !declared.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        for key in dropped {
            if let Some(mark) = marks.remove(&key) {
                self.drop_declared(mark).await?;
            }
        }
        for source in &applied.declarations.sources {
            let mark = marks.remove(&(DeclarationRecordKind::Source, source.source_id.to_string()));
            self.reconcile_source(source, mark, own, applied.revision)
                .await?;
        }
        for subscription in &applied.declarations.subscriptions {
            let key = (
                DeclarationRecordKind::Subscription,
                subscription.subscription_id.to_string(),
            );
            let mut mark = marks.remove(&key);
            // A subscription declared again as a role template, or the other
            // way round, first loses what the reconciler made of its old form.
            if let Some(before) =
                mark.take_if(|before| before.template != subscription.members.is_some())
            {
                self.drop_declared(before).await?;
            }
            if subscription.members.is_some() {
                self.reconcile_template(subscription, mark, own, applied.revision)
                    .await?;
            } else {
                self.reconcile_subscription(subscription, mark, own, applied.revision)
                    .await?;
            }
        }
        Ok(())
    }

    /// Write the mark of a record at `status`, keeping when it took the
    /// status if it already had it.
    async fn mark_record(
        &self,
        before: Option<&DeclarationMark>,
        base: DeclarationMark,
        status: DeclarationStatus,
        refusal: Option<DeclarationRefusal>,
        managed: bool,
    ) -> Result<(), AnnouncementError> {
        let now = self.now();
        let unchanged = before.is_some_and(|before| {
            before.status == status
                && before.refusal == refusal
                && before.managed == managed
                && before.digest == base.digest
                && before.mob_id == base.mob_id
        });
        if unchanged {
            return Ok(());
        }
        let since_ms = before
            .filter(|before| before.status == status)
            .map_or(now, |before| before.since_ms);
        let mark = DeclarationMark {
            status,
            since_ms,
            refusal,
            managed,
            ..base
        };
        self.inner.store.put_declaration_mark(mark, now).await?;
        Ok(())
    }

    /// Mark `before`'s record pending with the refusal `error` met, unless
    /// the store itself failed.
    async fn mark_record_refused(
        &self,
        before: Option<&DeclarationMark>,
        base: DeclarationMark,
        error: &AnnouncementError,
        managed: bool,
    ) -> Result<(), AnnouncementError> {
        if store_failure(error) {
            return Err(error.clone());
        }
        self.mark_record(
            before,
            base,
            DeclarationStatus::Pending,
            Some(refusal_from(error)),
            managed,
        )
        .await
    }

    /// Mark `before`'s record applied when `put` succeeded, or pending with
    /// the refusal it met.
    async fn mark_put<T>(
        &self,
        before: Option<&DeclarationMark>,
        base: DeclarationMark,
        put: Result<T, AnnouncementError>,
        managed: bool,
    ) -> Result<(), AnnouncementError> {
        match put {
            Ok(_) => {
                self.mark_record(before, base, DeclarationStatus::Applied, None, true)
                    .await
            }
            Err(error) => {
                self.mark_record_refused(before, base, &error, managed)
                    .await
            }
        }
    }

    /// Mark a create as `mark_put` does, except that a refusal because a
    /// record of the other kind holds the id makes the declared one
    /// unmanaged: the reconciler never made that record.
    async fn mark_created<T>(
        &self,
        before: Option<&DeclarationMark>,
        base: DeclarationMark,
        put: Result<T, AnnouncementError>,
        managed: bool,
    ) -> Result<(), AnnouncementError> {
        if matches!(
            &put,
            Err(AnnouncementError::Store(
                AnnouncementStoreError::SubscriptionIdTaken { .. }
            ))
        ) {
            return self.mark_replayed(before, base, put).await;
        }
        self.mark_put(before, base, put, managed).await
    }

    /// The create of a record that a live record made at runtime already
    /// holds: replayed, it meets its receipt when the reconciler made the
    /// record and a crash lost the mark; otherwise the record is unmanaged.
    async fn mark_replayed<T>(
        &self,
        before: Option<&DeclarationMark>,
        base: DeclarationMark,
        put: Result<T, AnnouncementError>,
    ) -> Result<(), AnnouncementError> {
        match put {
            Ok(_) => {
                self.mark_record(before, base, DeclarationStatus::Applied, None, true)
                    .await
            }
            Err(error) if store_failure(&error) => Err(error),
            Err(_) => {
                let refusal = unmanaged_record(base.kind, &base.id);
                self.mark_record(
                    before,
                    base,
                    DeclarationStatus::Pending,
                    Some(refusal),
                    false,
                )
                .await
            }
        }
    }

    /// A declared subscription or template whose id a cancelled or ended
    /// record holds: suppressed while the reconciler made it and its
    /// declaration is unchanged; otherwise pending until retention removes
    /// that record, as it is never made live again.
    async fn mark_terminal(
        &self,
        before: Option<&DeclarationMark>,
        base: DeclarationMark,
        error: AnnouncementStoreError,
    ) -> Result<(), AnnouncementError> {
        let managed = before.is_some_and(|mark| mark.managed);
        if held_at(before, &base.digest) {
            return self
                .mark_record(before, base, DeclarationStatus::Suppressed, None, true)
                .await;
        }
        let state = match &error {
            AnnouncementStoreError::SubscriptionEnded { .. } => "ended",
            _ => "cancelled",
        };
        let refusal = DeclarationRefusal {
            kind: refusal_from(&AnnouncementError::Store(error)).kind,
            message: format!(
                "{} {} is {state} and is never made live again: declare it under a new \
                 subscription_id to take effect now, or it is made again under this id once \
                 retention removes the {state} record (7 days by default)",
                base.kind.as_str(),
                base.id
            ),
        };
        self.mark_record(
            before,
            base,
            DeclarationStatus::Pending,
            Some(refusal),
            managed,
        )
        .await
    }

    async fn reconcile_source(
        &self,
        declared: &DeclaredSource,
        before: Option<DeclarationMark>,
        own: &MobId,
        revision: u64,
    ) -> Result<(), AnnouncementError> {
        let mob_id = declared.mob(own);
        let id = declared.source_id.to_string();
        let digest = declared.digest(own)?;
        let mut before = before;
        // A declaration that moved the source to another mob ends the one it
        // made in the first.
        if let Some(mark) = before.as_ref()
            && mark.mob_id.as_ref() != Some(&mob_id)
        {
            if mark.managed {
                self.end_declared_source(mark).await?;
            }
            before = None;
        }
        let base = DeclarationMark {
            kind: DeclarationRecordKind::Source,
            id: id.clone(),
            mob_id: Some(mob_id.clone()),
            template: false,
            revision,
            digest: digest.clone(),
            status: DeclarationStatus::Pending,
            since_ms: 0,
            refusal: None,
            managed: false,
        };
        let before = before.as_ref();
        let managed = before.is_some_and(|mark| mark.managed);
        let applied = before
            .is_some_and(|mark| mark.digest == digest && mark.status == DeclarationStatus::Applied);
        let found = self
            .inner
            .store
            .source(&mob_id, &declared.source_id)
            .await?;
        let Some(source) = found else {
            let operation = step_operation("create-source", &id, &digest, None)?;
            let put = self.put_source(&operation, declared.put(own, None)).await;
            return self.mark_put(before, base, put, managed).await;
        };
        if source.state != SourceState::Ended {
            if !managed {
                // Made at runtime and live: never touched.
                let operation = step_operation("create-source", &id, &digest, None)?;
                let put = self.put_source(&operation, declared.put(own, None)).await;
                return self.mark_replayed(before, base, put).await;
            }
            if applied {
                // Live and applied as declared: an operator's pause holds.
                return Ok(());
            }
            // Live and changed: replaced in place at its revision.
            let expected = Some(source.revision);
            let operation = step_operation("replace-source", &id, &digest, expected)?;
            let put = self
                .put_source(&operation, declared.put(own, expected))
                .await;
            return self.mark_put(before, base, put, managed).await;
        }
        // Ended by an operator: held while its declaration is unchanged.
        if held_at(before, &digest) {
            return self
                .mark_record(before, base, DeclarationStatus::Suppressed, None, true)
                .await;
        }
        // Ended, and declared anew or changed: its next generation starts.
        let operation = step_operation("restart-source", &id, &digest, Some(source.revision))?;
        let put = self.put_source(&operation, declared.put(own, None)).await;
        self.mark_put(before, base, put, managed).await
    }

    async fn reconcile_subscription(
        &self,
        declared: &DeclaredSubscription,
        before: Option<DeclarationMark>,
        own: &MobId,
        revision: u64,
    ) -> Result<(), AnnouncementError> {
        let id = declared.subscription_id.to_string();
        let digest = declared.digest(own)?;
        let base = DeclarationMark {
            kind: DeclarationRecordKind::Subscription,
            id: id.clone(),
            mob_id: None,
            template: false,
            revision,
            digest: digest.clone(),
            status: DeclarationStatus::Pending,
            since_ms: 0,
            refusal: None,
            managed: false,
        };
        let before = before.as_ref();
        let managed = before.is_some_and(|mark| mark.managed);
        let applied = before
            .is_some_and(|mark| mark.digest == digest && mark.status == DeclarationStatus::Applied);
        let Some(found) = self.subscription(&declared.subscription_id).await? else {
            let Some(request) = declared.put(own, None) else {
                return Ok(());
            };
            let operation = step_operation("create-subscription", &id, &digest, None)?;
            let put = self.put_subscription(&operation, request).await;
            return self.mark_created(before, base, put, managed).await;
        };
        if let Some(terminal) = found.terminal_refusal() {
            return self.mark_terminal(before, base, terminal).await;
        }
        if !managed {
            // Made at runtime and live: never touched.
            let Some(request) = declared.put(own, None) else {
                return Ok(());
            };
            let operation = step_operation("create-subscription", &id, &digest, None)?;
            let put = self.put_subscription(&operation, request).await;
            return self.mark_replayed(before, base, put).await;
        }
        if applied {
            // Live and applied as declared: an operator's pause holds.
            return Ok(());
        }
        // Live and changed: replaced in place at its revision, keeping what
        // it owes.
        let expected = Some(found.revision);
        let Some(request) = declared.put(own, expected) else {
            return Ok(());
        };
        let operation = step_operation("replace-subscription", &id, &digest, expected)?;
        let put = self.put_subscription(&operation, request).await;
        self.mark_put(before, base, put, managed).await
    }

    async fn reconcile_template(
        &self,
        declared: &DeclaredSubscription,
        before: Option<DeclarationMark>,
        own: &MobId,
        revision: u64,
    ) -> Result<(), AnnouncementError> {
        let id = declared.subscription_id.to_string();
        let digest = declared.digest(own)?;
        let base = DeclarationMark {
            kind: DeclarationRecordKind::Subscription,
            id: id.clone(),
            mob_id: None,
            template: true,
            revision,
            digest: digest.clone(),
            status: DeclarationStatus::Pending,
            since_ms: 0,
            refusal: None,
            managed: false,
        };
        let before = before.as_ref();
        let managed = before.is_some_and(|mark| mark.managed);
        let applied = before
            .is_some_and(|mark| mark.digest == digest && mark.status == DeclarationStatus::Applied);
        let found = self
            .inner
            .store
            .role_template(&declared.subscription_id)
            .await?;
        let Some(template) = found else {
            let Some(request) = declared.template(own, None) else {
                return Ok(());
            };
            let operation = step_operation("create-template", &id, &digest, None)?;
            let put = self.put_role_template(&operation, request).await;
            return self.mark_created(before, base, put, managed).await;
        };
        if template.state == SubscriptionState::Cancelled {
            // Cancelled: held while its declaration is unchanged; declared
            // anew or changed, it starts again at its next revision.
            if held_at(before, &digest) {
                return self
                    .mark_record(before, base, DeclarationStatus::Suppressed, None, true)
                    .await;
            }
            let Some(request) = declared.template(own, None) else {
                return Ok(());
            };
            let operation =
                step_operation("restart-template", &id, &digest, Some(template.revision))?;
            let put = self.put_role_template(&operation, request).await;
            return self.mark_put(before, base, put, managed).await;
        }
        if !managed {
            // Made at runtime and live: never touched.
            let Some(request) = declared.template(own, None) else {
                return Ok(());
            };
            let operation = step_operation("create-template", &id, &digest, None)?;
            let put = self.put_role_template(&operation, request).await;
            return self.mark_replayed(before, base, put).await;
        }
        if applied {
            return Ok(());
        }
        let expected = Some(template.revision);
        let Some(request) = declared.template(own, expected) else {
            return Ok(());
        };
        let operation = step_operation("replace-template", &id, &digest, expected)?;
        let put = self.put_role_template(&operation, request).await;
        self.mark_put(before, base, put, managed).await
    }

    /// End the live source a mark says the reconciler made.
    async fn end_declared_source(&self, mark: &DeclarationMark) -> Result<(), AnnouncementError> {
        let Some(mob_id) = mark.mob_id.as_ref() else {
            return Ok(());
        };
        let source_id = crate::mob_announcements::types::SourceId::new(mark.id.clone())?;
        let Some(source) = self.inner.store.source(mob_id, &source_id).await? else {
            return Ok(());
        };
        if source.state == SourceState::Ended {
            return Ok(());
        }
        let operation =
            step_operation("end-source", &mark.id, &mark.digest, Some(source.revision))?;
        self.change_source(
            &operation,
            mob_id,
            &source_id,
            source.revision,
            SourceChange::End,
        )
        .await?;
        Ok(())
    }

    /// A record the declarations no longer name: what the reconciler made
    /// is ended (a source) or cancelled (a subscription or a role template
    /// with its children), and the mark goes. A refusal keeps the mark for
    /// the next trigger.
    async fn drop_declared(&self, mark: DeclarationMark) -> Result<(), AnnouncementError> {
        let dropped = match mark.kind {
            _ if !mark.managed => Ok(()),
            DeclarationRecordKind::Source => self.end_declared_source(&mark).await,
            DeclarationRecordKind::Subscription if mark.template => {
                self.cancel_declared_template(&mark).await
            }
            DeclarationRecordKind::Subscription => self.cancel_declared_subscription(&mark).await,
            DeclarationRecordKind::RoleChild => Ok(()),
        };
        match dropped {
            Ok(()) => {
                self.inner
                    .store
                    .remove_declaration_mark(mark.kind, &mark.id, self.now())
                    .await?;
                Ok(())
            }
            Err(error) if store_failure(&error) => Err(error),
            Err(error) => {
                tracing::warn!(
                    kind = mark.kind.as_str(),
                    id = %mark.id,
                    %error,
                    "a record dropped from the declarations was not ended yet"
                );
                Ok(())
            }
        }
    }

    async fn cancel_declared_subscription(
        &self,
        mark: &DeclarationMark,
    ) -> Result<(), AnnouncementError> {
        let id = crate::mob_announcements::types::SubscriptionId::new(mark.id.clone())?;
        let Some(subscription) = self.subscription(&id).await? else {
            return Ok(());
        };
        if !subscription.state.is_live() {
            return Ok(());
        }
        let operation = step_operation(
            "cancel-subscription",
            &mark.id,
            &mark.digest,
            Some(subscription.revision),
        )?;
        self.change_subscription(
            &operation,
            &id,
            subscription.revision,
            SubscriptionChange::Cancel,
        )
        .await?;
        Ok(())
    }

    async fn cancel_declared_template(
        &self,
        mark: &DeclarationMark,
    ) -> Result<(), AnnouncementError> {
        let id = crate::mob_announcements::types::SubscriptionId::new(mark.id.clone())?;
        let Some(template) = self.inner.store.role_template(&id).await? else {
            return Ok(());
        };
        if template.state == SubscriptionState::Cancelled {
            return Ok(());
        }
        let operation = step_operation(
            "cancel-template",
            &mark.id,
            &mark.digest,
            Some(template.revision),
        )?;
        self.inner
            .store
            .cancel_role_template(&operation, &id, template.revision, self.now())
            .await?;
        self.wake();
        Ok(())
    }

    /// Bring every live role template's children in line with its members
    /// now.
    async fn reconcile_roles(&self) -> Result<(), AnnouncementError> {
        let templates = self.inner.store.role_templates(None).await?;
        if templates.is_empty() {
            return Ok(());
        }
        let (roles, admitted) = self.expand_roles(&templates).await;
        let facts = self.roster_facts(admitted).await;
        self.inner
            .store
            .reconcile_role_children(&roles, &facts, self.now())
            .await?;
        Ok(())
    }

    /// Resolve the members each template takes now, and check every one
    /// without a child as a put of its subscription would: its route, the
    /// runtime's governance, its mob's event cursor and the member itself.
    /// A template whose roster or mobs cannot be read now is left out: its
    /// children stay as they are, and none is made. Returns the expansions
    /// and the members admitted, whose roster facts the store needs.
    pub(super) async fn expand_roles(
        &self,
        templates: &[TemplateChildren],
    ) -> (RoleExpansions, BTreeSet<Member>) {
        let mut roles = RoleExpansions {
            templates: BTreeMap::new(),
            child_limits: self.inner.config.child_limits,
        };
        let mut admitted = BTreeSet::new();
        for TemplateChildren { template, children } in templates {
            let members = match self
                .inner
                .native
                .members_with_role(&template.members.mob_id, &template.members.role)
                .await
            {
                Ok(members) => members,
                Err(error) => {
                    tracing::warn!(
                        template = %template.id,
                        error = %error.0,
                        "a role template's roster could not be read; its children stay as they are"
                    );
                    continue;
                }
            };
            if let Err(error) = self.ensure_hosted(&template.mobs).await {
                tracing::warn!(
                    template = %template.id,
                    %error,
                    "a role template names a mob not hosted now; its children stay as they are"
                );
                continue;
            }
            // A child pending or left captures again only once its member
            // passes the checks again.
            let made = children
                .iter()
                .map(|(child, state)| (&child.member, (child.status, state.is_some())))
                .collect::<BTreeMap<_, _>>();
            let mut expansion = RoleExpansion {
                revision: template.revision,
                members: BTreeMap::new(),
            };
            for member in members {
                let kept = made.get(&member).is_some_and(|(status, recorded)| {
                    !matches!(status, RoleChildStatus::Pending | RoleChildStatus::Left) && *recorded
                });
                let found = if kept {
                    RoleMember::Kept
                } else {
                    match self.admit_role_member(&member).await {
                        Ok((route, member_mob_cursor)) => {
                            admitted.insert(member.clone());
                            RoleMember::Admitted {
                                route,
                                member_mob_cursor,
                            }
                        }
                        Err(error) => RoleMember::Refused(refusal_from(&error)),
                    }
                };
                expansion.members.insert(member, found);
            }
            roles.templates.insert(template.id.clone(), expansion);
        }
        (roles, admitted)
    }

    /// `subscriptions/put` with `members`, and the reconciler's put of a
    /// declared template: on a replace, each live child is admitted again,
    /// the owner checking its member and the store the sources' audiences
    /// and routes.
    pub(super) async fn put_template(
        &self,
        operation: &OperationId,
        request: PutRoleTemplate,
    ) -> Result<RoleTemplate, AnnouncementError> {
        self.ensure_hosted(&request.mobs).await?;
        let (children, facts) = self.readmission(&request.id).await?;
        Ok(self
            .inner
            .store
            .put_role_template(operation, request, &children, &facts, self.now())
            .await?)
    }

    /// The owner's verdict on the member of each live child of the template
    /// `id` that captures (one applied or pending), and the roster facts of
    /// the admitted ones: what a replace admits them again with.
    async fn readmission(
        &self,
        id: &SubscriptionId,
    ) -> Result<(RoleExpansion, RosterFacts), AnnouncementError> {
        let mut expansion = RoleExpansion {
            revision: 0,
            members: BTreeMap::new(),
        };
        let Some(found) = self.inner.store.role_template_children(id).await? else {
            return Ok((expansion, RosterFacts::default()));
        };
        expansion.revision = found.template.revision;
        let mut admitted = BTreeSet::new();
        for (child, state) in &found.children {
            if !matches!(
                child.status,
                RoleChildStatus::Applied | RoleChildStatus::Pending
            ) || !state.is_some_and(SubscriptionState::is_live)
            {
                continue;
            }
            let verdict = match self.admit_role_member(&child.member).await {
                Ok((route, member_mob_cursor)) => {
                    admitted.insert(child.member.clone());
                    RoleMember::Admitted {
                        route,
                        member_mob_cursor,
                    }
                }
                Err(error) => RoleMember::Refused(refusal_from(&error)),
            };
            expansion.members.insert(child.member.clone(), verdict);
        }
        let facts = self.roster_facts(admitted).await;
        Ok((expansion, facts))
    }

    /// A role template by its id, whatever its state: `subscriptions/get`
    /// answers a template's id with it.
    pub async fn role_template(
        &self,
        id: &SubscriptionId,
    ) -> Result<Option<RoleTemplate>, AnnouncementError> {
        Ok(self.inner.store.role_template(id).await?)
    }

    /// `subscriptions/pause`, `resume` and `cancel`: of the subscription with
    /// this id, or, when none has it, of the role template with it and its
    /// children in one step.
    pub async fn change_subscription_or_template(
        &self,
        operation: &OperationId,
        id: &SubscriptionId,
        expected_revision: u64,
        change: SubscriptionChange,
    ) -> Result<SubscriptionVerbOutcome, AnnouncementError> {
        self.ensure_authoritative()?;
        if self.subscription(id).await?.is_none()
            && self.inner.store.role_template(id).await?.is_some()
        {
            let outcome = self
                .inner
                .store
                .change_role_template(operation, id, expected_revision, change, self.now())
                .await?;
            self.wake();
            return Ok(SubscriptionVerbOutcome::Template(outcome));
        }
        Ok(SubscriptionVerbOutcome::Subscription(
            self.change_subscription(operation, id, expected_revision, change)
                .await?,
        ))
    }

    /// The checks a put of `member`'s subscription makes before the store:
    /// the route it is kept on or takes now, the runtime's governance for a
    /// member it delivers to itself, its mob's event cursor for a member of a
    /// mob a member created, and the member itself.
    async fn admit_role_member(
        &self,
        member: &Member,
    ) -> Result<(MemberRouteRecord, Option<u64>), AnnouncementError> {
        let route = self.member_route(member).await?;
        if route.route != MemberRoute::Peer {
            self.ensure_deliverable()?;
        }
        let member_mob_cursor = match route.route {
            MemberRoute::Child => Some(
                self.inner
                    .native
                    .member_mob_cursor(&member.mob_id)
                    .await
                    .map_err(|error| AnnouncementError::NativeUnavailable(error.0))?,
            ),
            MemberRoute::Local | MemberRoute::Peer => None,
        };
        self.inner
            .native
            .check_member(member, route.route)
            .await
            .map_err(|problem| member_problem(member, problem))?;
        Ok((route, member_mob_cursor))
    }

    /// `declarations/get`: the applied revision and digest, every record the
    /// declarations manage or could not make, and the children of the role
    /// templates they made.
    pub(super) async fn declared_view(&self) -> Result<DeclarationsView, AnnouncementError> {
        let own = self.declaring_mob()?;
        let (revision, digest) = match self.inner.store.applied_declarations().await? {
            Some(applied) => (applied.revision, applied.digest),
            None => (
                0,
                Declarations {
                    revision: 0,
                    sources: Vec::new(),
                    subscriptions: Vec::new(),
                }
                .digest(&own)?,
            ),
        };
        let marks = self.inner.store.declaration_marks().await?;
        let deliverable = self.ensure_deliverable().err().map(|error| match error {
            AnnouncementError::GovernedRuntime => "governed_runtime".to_string(),
            other => crate::mob_announcements::rpc::refusal_kind(&other),
        });
        let delivery = |state: Option<SubscriptionState>| match (&deliverable, state) {
            (_, Some(state)) if state != SubscriptionState::Active => None,
            (None, Some(_)) => Some(DeclaredDelivery::Ready),
            (Some(cause), Some(_)) => Some(DeclaredDelivery::Refused {
                cause: cause.clone(),
            }),
            (_, None) => None,
        };
        let mut records = Vec::with_capacity(marks.len());
        let mut templates = BTreeSet::new();
        for mark in &marks {
            let mut record = DeclarationRecordView {
                kind: mark.kind,
                id: mark.id.clone(),
                status: mark.status,
                refusal: mark.refusal.clone(),
                collection: None,
                delivery: None,
            };
            // A template the reconciler made lists its children whatever its
            // own status: they capture on their own.
            if mark.template && mark.managed {
                templates.insert(mark.id.clone());
            }
            if mark.status == DeclarationStatus::Applied {
                match mark.kind {
                    DeclarationRecordKind::Source => {
                        record.collection = self.declared_collection(mark).await?;
                    }
                    DeclarationRecordKind::Subscription if mark.template => {}
                    DeclarationRecordKind::Subscription => {
                        let id =
                            crate::mob_announcements::types::SubscriptionId::new(mark.id.clone())?;
                        let state = self.subscription(&id).await?.map(|found| found.state);
                        record.delivery = delivery(state);
                    }
                    DeclarationRecordKind::RoleChild => {}
                }
            }
            records.push(record);
        }
        for child in self.inner.store.role_children().await? {
            if !templates.contains(child.template_id.as_str()) {
                continue;
            }
            let status = match child.status {
                RoleChildStatus::Applied => DeclarationStatus::Applied,
                RoleChildStatus::Pending => DeclarationStatus::Pending,
                RoleChildStatus::Left => DeclarationStatus::Left,
                RoleChildStatus::Suppressed => DeclarationStatus::Suppressed,
            };
            let delivery = if status == DeclarationStatus::Applied {
                let state = self
                    .subscription(&child.subscription_id)
                    .await?
                    .map(|found| found.state);
                delivery(state)
            } else {
                None
            };
            records.push(DeclarationRecordView {
                kind: DeclarationRecordKind::RoleChild,
                id: child.subscription_id.to_string(),
                status,
                refusal: child.refusal,
                collection: None,
                delivery,
            });
        }
        Ok(DeclarationsView {
            revision,
            digest,
            records,
        })
    }

    /// Retry the pending declared records at a health pass: a member that
    /// registered since may be admitted now. Nothing is reconciled while
    /// nothing is pending, and a failure waits for the next pass.
    pub(super) async fn retry_pending_declared(&self) {
        let pending = match self.inner.store.declaration_marks().await {
            Ok(marks) => marks
                .iter()
                .any(|mark| mark.status == DeclarationStatus::Pending),
            Err(error) => {
                tracing::warn!(%error, "declaration marks not read at the health pass");
                return;
            }
        };
        if pending && let Err(error) = self.reconcile_declared().await {
            tracing::warn!(
                kind = %crate::mob_announcements::rpc::refusal_kind(&error),
                %error,
                "pending declarations not retried at the health pass"
            );
        }
    }

    /// An applied source's collection, as its health stands at the read.
    async fn declared_collection(
        &self,
        mark: &DeclarationMark,
    ) -> Result<Option<DeclaredCollection>, AnnouncementError> {
        let Some(mob_id) = mark.mob_id.as_ref() else {
            return Ok(None);
        };
        let source_id = crate::mob_announcements::types::SourceId::new(mark.id.clone())?;
        let Some(source) = self.inner.store.source(mob_id, &source_id).await? else {
            return Ok(None);
        };
        Ok(match self.health_status(&source) {
            SourceHealthStatus::Baselining => Some(DeclaredCollection::Baselining),
            SourceHealthStatus::Backfilling => Some(DeclaredCollection::Backfilling),
            SourceHealthStatus::Ok => Some(DeclaredCollection::Live),
            SourceHealthStatus::Failing => Some(DeclaredCollection::Failing),
            SourceHealthStatus::Stale => Some(DeclaredCollection::Stale),
            SourceHealthStatus::Paused | SourceHealthStatus::Ended => None,
        })
    }
}
