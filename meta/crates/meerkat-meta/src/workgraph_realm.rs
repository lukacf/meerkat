//! Mob-realm scoping of member-bound WorkGraph attention.
//!
//! meerkat 0.8.41 builds every mob member in the realm `mob.<mob_id>` and
//! rescopes the WorkGraph service a host hands the mob runtime to that realm
//! (`meerkat_mob::mob_scoped_workgraph_service`), so a member resolves its
//! attention bindings in the mob realm and nowhere else. A goal, attention
//! binding or reassignment that names a member (`WorkOwnerKey::mob_agent`,
//! id `mob/<mob_id>/agent/<identity>`) from any other realm is refused typed
//! by meerkat (`WorkGraphError::AttentionTargetRealmMismatch`): storing it
//! elsewhere would put it where the member never looks.
//!
//! Meta's own runtime service is scoped to the mob realm at construction
//! (`workgraph_wiring::scoped_workgraph_service`), so member-bound work for
//! THIS mob created through the console, the RPC surfaces or the agent tool
//! plane already lands in the realm the member reads. Two things remain and
//! live here:
//!
//! 1. [`classify_goal_target`]: the typed owner-key classification the RPC
//!    arms use to refuse a member of ANOTHER mob before the write. The console
//!    and stdin surfaces are scoped to one realm by contract (`realm_id` is
//!    never accepted over the wire), so a binding written into a sibling mob's
//!    realm from here would be unlistable and unmanageable from this surface;
//!    it is refused with meerkat's own typed error instead of being routed or
//!    silently accepted. Such work is created through that mob's own runtime.
//! 2. [`migrate_member_bindings_to_mob_realms`]: bindings that predate the
//!    rescoping. Before meerkat 0.8.41 an agent-spawned child mob shared the
//!    parent runtime's service unscoped, so a console-created goal that
//!    bound a child-mob member (an explicit `owner` key naming the child)
//!    lived in the parent realm and the child member found it there. After the
//!    rescoping the child resolves attention in `mob.<child>` and that binding
//!    is invisible. The migration re-creates every such binding in the mob
//!    realm its owner key names, over the same store, and cancels the parent
//!    realm original (which stops its binding), so a second run finds nothing
//!    to do. It runs at `MobRuntime` bootstrap ([`WorkGraphRealmMigrationMode`]
//!    on the bootstrap spec) and its report is exposed on `meta/capabilities`.

use std::sync::Arc;

use meerkat::{
    AttentionListRequest, AttentionPauseRequest, CloseWorkItemRequest, GoalAttentionTarget,
    GoalCreateRequest, WorkAttentionBinding, WorkAttentionStatus, WorkAttentionTarget,
    WorkGraphError, WorkGraphService, WorkItem, WorkOwnerKey, WorkStatus,
};
use serde::{Deserialize, Serialize};

/// Which WorkGraph realm a goal or attention target must live in, read off
/// the typed owner key alone (no roster, no name matching).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttentionTargetRealm {
    /// The target is not a mob member owner key (a session target, or an
    /// owner key of another kind): it lives in the realm of the service that
    /// writes it.
    Unscoped,
    /// The target names a member of the mob whose realm the writing service
    /// is scoped to.
    ThisMob { mob_id: String },
    /// The target names a member of another mob, whose members resolve
    /// attention only in `required_realm_id`.
    OtherMob {
        mob_id: String,
        required_realm_id: String,
    },
}

/// Classify an owner key against the realm a service is scoped to.
pub fn classify_owner_key(
    owner_key: &WorkOwnerKey,
    service_realm_id: &str,
) -> Result<AttentionTargetRealm, WorkGraphError> {
    let Some(member) = owner_key.as_mob_agent() else {
        return Ok(AttentionTargetRealm::Unscoped);
    };
    let required_realm_id = member.realm_id()?;
    if required_realm_id == service_realm_id {
        Ok(AttentionTargetRealm::ThisMob {
            mob_id: member.mob_id.to_string(),
        })
    } else {
        Ok(AttentionTargetRealm::OtherMob {
            mob_id: member.mob_id.to_string(),
            required_realm_id,
        })
    }
}

/// Classify a goal/attention target against the realm `service` writes into.
pub fn classify_goal_target(
    target: &GoalAttentionTarget,
    service: &WorkGraphService,
) -> Result<AttentionTargetRealm, WorkGraphError> {
    match target {
        GoalAttentionTarget::Session { .. } => Ok(AttentionTargetRealm::Unscoped),
        GoalAttentionTarget::Owner { owner_key } => {
            classify_owner_key(owner_key, service.default_realm_id())
        }
    }
}

/// Refuse a target that names a member of another mob before `service`
/// writes it. Returns meerkat's own typed refusal so every surface reports
/// the same error whether Meta or meerkat caught it first.
pub fn refuse_foreign_mob_target(
    target: &GoalAttentionTarget,
    service: &WorkGraphService,
) -> Result<(), WorkGraphError> {
    match classify_goal_target(target, service)? {
        AttentionTargetRealm::OtherMob {
            mob_id,
            required_realm_id,
        } => {
            let owner_key = match target {
                GoalAttentionTarget::Owner { owner_key } => owner_key.canonical(),
                GoalAttentionTarget::Session { session_id } => format!("session:{session_id}"),
            };
            Err(WorkGraphError::AttentionTargetRealmMismatch {
                owner_key,
                mob_id,
                required_realm_id,
                realm_id: service.default_realm_id().to_string(),
            })
        }
        AttentionTargetRealm::Unscoped | AttentionTargetRealm::ThisMob { .. } => Ok(()),
    }
}

/// Whether the bootstrap migration of pre-rescoping member bindings runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkGraphRealmMigrationMode {
    /// Re-create foreign-mob member bindings in their mob realm and cancel
    /// the originals.
    #[default]
    Apply,
    /// Report what `Apply` would migrate; write nothing.
    DryRun,
    /// Do not scan.
    Off,
}

/// One member binding the migration found in the runtime's realm whose owner
/// key names another mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigratedMemberBinding {
    pub binding_id: String,
    pub item_id: String,
    pub owner_key: String,
    pub mob_id: String,
    pub from_realm_id: String,
    pub to_realm_id: String,
    pub status: WorkAttentionStatus,
    /// The binding created in the mob realm; `None` in dry-run mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_binding_id: Option<String>,
    /// The goal item created in the mob realm; `None` in dry-run mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_item_id: Option<String>,
}

/// How the migration ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkGraphRealmMigrationOutcome {
    Completed,
    /// The scan or a write failed; `migrated` holds what completed before it.
    Failed {
        detail: String,
    },
}

/// Report of one migration run, exposed on `meta/capabilities`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkGraphRealmMigrationReport {
    pub mode: WorkGraphRealmMigrationMode,
    pub realm_id: String,
    pub scanned_bindings: usize,
    /// Foreign-mob member bindings already terminal (stopped or superseded),
    /// left alone.
    pub skipped_terminal: usize,
    pub migrated: Vec<MigratedMemberBinding>,
    pub outcome: WorkGraphRealmMigrationOutcome,
}

impl WorkGraphRealmMigrationReport {
    fn empty(mode: WorkGraphRealmMigrationMode, realm_id: &str) -> Self {
        Self {
            mode,
            realm_id: realm_id.to_string(),
            scanned_bindings: 0,
            skipped_terminal: 0,
            migrated: Vec::new(),
            outcome: WorkGraphRealmMigrationOutcome::Completed,
        }
    }
}

const MIGRATION_TRACE_TARGET: &str = "meta::workgraph_realm_migration";

/// Move every member-bound attention binding in `service`'s realm whose
/// owner key names ANOTHER mob into that mob's realm (same store, same
/// namespace), preserving title, description, mode, completion and join
/// policies, priority, labels, timing fields, external and evidence refs,
/// delegated authority, projection policy and the paused state. The original
/// goal item is closed as `Cancelled`, which stops its binding, so a rerun is
/// a no-op. Edges of the original item are not carried (an attention overlay
/// reads the goal and its binding, not the item's links).
///
/// Never returns an error for a store or write failure: the failure is
/// recorded in the report's outcome with everything completed before it, so
/// bootstrap can proceed and the operator sees the state on `meta/capabilities`.
pub async fn migrate_member_bindings_to_mob_realms(
    service: &WorkGraphService,
    mode: WorkGraphRealmMigrationMode,
) -> WorkGraphRealmMigrationReport {
    let realm_id = service.default_realm_id().to_string();
    let mut report = WorkGraphRealmMigrationReport::empty(mode, &realm_id);
    if mode == WorkGraphRealmMigrationMode::Off {
        return report;
    }
    match run_migration(service, mode, &realm_id, &mut report).await {
        Ok(()) => {}
        Err(error) => {
            tracing::error!(
                target: MIGRATION_TRACE_TARGET,
                realm_id = %realm_id,
                migrated = report.migrated.len(),
                %error,
                "member binding realm migration failed"
            );
            report.outcome = WorkGraphRealmMigrationOutcome::Failed {
                detail: error.to_string(),
            };
        }
    }
    report
}

async fn run_migration(
    service: &WorkGraphService,
    mode: WorkGraphRealmMigrationMode,
    realm_id: &str,
    report: &mut WorkGraphRealmMigrationReport,
) -> Result<(), WorkGraphError> {
    let bindings = service
        .list_attention(AttentionListRequest {
            realm_id: None,
            namespace: None,
            target: None,
            status: None,
        })
        .await?
        .attention;
    report.scanned_bindings = bindings.len();
    for binding in bindings {
        let WorkAttentionTarget::LoweredOwner { owner_key } = &binding.target else {
            continue;
        };
        let AttentionTargetRealm::OtherMob {
            mob_id,
            required_realm_id,
        } = classify_owner_key(owner_key, realm_id)?
        else {
            continue;
        };
        if matches!(
            binding.status,
            WorkAttentionStatus::Stopped | WorkAttentionStatus::Superseded
        ) {
            report.skipped_terminal += 1;
            continue;
        }
        let item = service
            .get(None, None, binding.work_ref.item_id.clone())
            .await?;
        if matches!(
            item.status,
            WorkStatus::Completed | WorkStatus::Cancelled | WorkStatus::Failed
        ) {
            report.skipped_terminal += 1;
            continue;
        }
        let mut record = MigratedMemberBinding {
            binding_id: binding.binding_id.to_string(),
            item_id: item.id.to_string(),
            owner_key: owner_key.canonical(),
            mob_id,
            from_realm_id: realm_id.to_string(),
            to_realm_id: required_realm_id.clone(),
            status: binding.status.clone(),
            new_binding_id: None,
            new_item_id: None,
        };
        if mode == WorkGraphRealmMigrationMode::Apply {
            let (new_item_id, new_binding_id) =
                migrate_one(service, &required_realm_id, &item, &binding, owner_key).await?;
            record.new_item_id = Some(new_item_id);
            record.new_binding_id = Some(new_binding_id);
            tracing::info!(
                target: MIGRATION_TRACE_TARGET,
                binding_id = %record.binding_id,
                item_id = %record.item_id,
                owner_key = %record.owner_key,
                from_realm_id = %record.from_realm_id,
                to_realm_id = %record.to_realm_id,
                new_binding_id = record.new_binding_id.as_deref().unwrap_or_default(),
                "member binding re-created in its mob realm; original cancelled"
            );
        } else {
            tracing::info!(
                target: MIGRATION_TRACE_TARGET,
                binding_id = %record.binding_id,
                owner_key = %record.owner_key,
                from_realm_id = %record.from_realm_id,
                to_realm_id = %record.to_realm_id,
                "member binding would be re-created in its mob realm (dry run)"
            );
        }
        report.migrated.push(record);
    }
    Ok(())
}

/// Re-create `item` + `binding` in `target_realm_id` and cancel the original.
/// Returns the new item and binding ids.
async fn migrate_one(
    service: &WorkGraphService,
    target_realm_id: &str,
    item: &WorkItem,
    binding: &WorkAttentionBinding,
    owner_key: &WorkOwnerKey,
) -> Result<(String, String), WorkGraphError> {
    let sibling = WorkGraphService::with_scope(
        Arc::clone(service.store()),
        target_realm_id,
        service.default_namespace().clone(),
    );
    // A new item may only start Open or Blocked; an in-progress original
    // restarts Open in the mob realm (its claim belonged to the old realm).
    let status = match item.status {
        WorkStatus::Blocked => Some(WorkStatus::Blocked),
        _ => None,
    };
    let created = sibling
        .create_goal(GoalCreateRequest {
            realm_id: None,
            namespace: None,
            title: item.title.clone(),
            description: item.description.clone(),
            target: GoalAttentionTarget::Owner {
                owner_key: owner_key.clone(),
            },
            mode: binding.mode,
            completion_policy: item.completion_policy.clone(),
            failed_child_join_policy: item.failed_child_join_policy,
            cancelled_child_join_policy: item.cancelled_child_join_policy,
            priority: item.priority,
            labels: item.labels.clone(),
            due_at: item.due_at,
            not_before: item.not_before,
            snoozed_until: item.snoozed_until,
            external_refs: item.external_refs.clone(),
            evidence_refs: item.evidence_refs.clone(),
            status,
            delegated_authority: binding.delegated_authority,
            projection_policy: binding.projection_policy.clone(),
        })
        .await?;
    if let WorkAttentionStatus::Paused { until } = &binding.status {
        sibling
            .pause_attention(AttentionPauseRequest {
                binding_id: created.attention.binding_id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: created.attention.machine_state.revision,
                until: *until,
            })
            .await?;
    }
    service
        .close(CloseWorkItemRequest {
            id: item.id.clone(),
            realm_id: None,
            namespace: None,
            expected_revision: item.revision,
            status: WorkStatus::Cancelled,
        })
        .await?;
    Ok((
        created.item.id.to_string(),
        created.attention.binding_id.to_string(),
    ))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat::{
        AttentionDelegatedAuthority, AttentionProjectionPolicy, CreateWorkItemRequest,
        MemoryWorkGraphStore, WorkAttentionBindingId, WorkAttentionMode, WorkCompletionPolicy,
        WorkGraphEvent, WorkGraphEventKind, WorkGraphMachine, WorkGraphStore, WorkItemRef,
        WorkNamespace,
    };

    fn mob_realm(mob_id: &str) -> String {
        meerkat_core::mob_realm_id(mob_id)
            .expect("mob realm")
            .as_str()
            .to_string()
    }

    fn member_key(mob_id: &str, identity: &str) -> WorkOwnerKey {
        WorkOwnerKey::mob_agent(mob_id, identity).expect("member owner key")
    }

    #[test]
    fn owner_keys_classify_by_the_mob_they_name() {
        let alpha = mob_realm("alpha");
        assert_eq!(
            classify_owner_key(&member_key("alpha", "w"), &alpha).unwrap(),
            AttentionTargetRealm::ThisMob {
                mob_id: "alpha".to_string()
            }
        );
        assert_eq!(
            classify_owner_key(&member_key("beta", "w"), &alpha).unwrap(),
            AttentionTargetRealm::OtherMob {
                mob_id: "beta".to_string(),
                required_realm_id: mob_realm("beta"),
            }
        );
        let principal = WorkOwnerKey::principal("operator@example.test").expect("principal");
        assert_eq!(
            classify_owner_key(&principal, &alpha).unwrap(),
            AttentionTargetRealm::Unscoped
        );
        // A plain agent key without the member shape is not a member.
        let plain = WorkOwnerKey::agent("helper").expect("agent key");
        assert_eq!(
            classify_owner_key(&plain, &alpha).unwrap(),
            AttentionTargetRealm::Unscoped
        );
    }

    #[test]
    fn session_targets_are_unscoped_and_foreign_members_are_refused_typed() {
        let service = WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            mob_realm("alpha"),
            WorkNamespace::default(),
        );
        let session = GoalAttentionTarget::Session {
            session_id: meerkat::SessionId::new(),
        };
        assert_eq!(
            classify_goal_target(&session, &service).unwrap(),
            AttentionTargetRealm::Unscoped
        );
        refuse_foreign_mob_target(&session, &service).expect("session targets pass");
        refuse_foreign_mob_target(
            &GoalAttentionTarget::Owner {
                owner_key: member_key("alpha", "w"),
            },
            &service,
        )
        .expect("this mob's member passes");
        let error = refuse_foreign_mob_target(
            &GoalAttentionTarget::Owner {
                owner_key: member_key("beta", "w"),
            },
            &service,
        )
        .expect_err("another mob's member is refused");
        match error {
            WorkGraphError::AttentionTargetRealmMismatch {
                owner_key,
                mob_id,
                required_realm_id,
                realm_id,
            } => {
                assert_eq!(owner_key, member_key("beta", "w").canonical());
                assert_eq!(mob_id, "beta");
                assert_eq!(required_realm_id, mob_realm("beta"));
                assert_eq!(realm_id, mob_realm("alpha"));
            }
            other => panic!("expected the typed realm refusal, got {other:?}"),
        }
    }

    /// Seed a binding the way a pre-0.8.41 runtime left it: a goal in the
    /// parent realm whose attention target names a member of another mob.
    /// The service layer refuses this now, so the rows go in through the
    /// store, exactly as the legacy rows exist on disk.
    async fn seed_legacy_binding(
        service: &WorkGraphService,
        owner_key: WorkOwnerKey,
        title: &str,
        status: WorkAttentionStatus,
    ) -> WorkAttentionBinding {
        let realm_id = service.default_realm_id().to_string();
        let namespace = service.default_namespace().clone();
        let now = service
            .store()
            .get_store_time_utc()
            .await
            .expect("store time");
        let (item, item_event) = WorkGraphMachine::create_item(
            CreateWorkItemRequest {
                realm_id: Some(realm_id.clone()),
                namespace: Some(namespace.clone()),
                title: title.to_string(),
                description: Some("legacy".to_string()),
                completion_policy: WorkCompletionPolicy::SelfAttest,
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: ["legacy".to_string()].into_iter().collect(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            },
            realm_id.clone(),
            namespace.clone(),
            now,
        )
        .expect("create legacy item");
        let attention = WorkAttentionBinding {
            binding_id: WorkAttentionBindingId::generated(),
            work_ref: WorkItemRef {
                realm_id: realm_id.clone(),
                namespace: namespace.clone(),
                item_id: item.id.clone(),
            },
            target: WorkAttentionTarget::LoweredOwner { owner_key },
            mode: WorkAttentionMode::Coordinate,
            status,
            machine_state: Default::default(),
            delegated_authority: AttentionDelegatedAuthority::AddEvidence,
            projection_policy: AttentionProjectionPolicy::default(),
            created_at: now,
            updated_at: now,
        };
        let attention_event = WorkGraphEvent::graph(
            realm_id,
            namespace,
            WorkGraphEventKind::AttentionCreated,
            now,
            serde_json::json!({ "attention": attention }),
        );
        let (_, attention) = service
            .store()
            .insert_goal(item, item_event, attention, attention_event)
            .await
            .expect("seed legacy goal");
        attention
    }

    async fn bindings_in(service: &WorkGraphService) -> Vec<WorkAttentionBinding> {
        service
            .list_attention(AttentionListRequest {
                realm_id: None,
                namespace: None,
                target: None,
                status: None,
            })
            .await
            .expect("list attention")
            .attention
    }

    #[tokio::test]
    async fn migration_moves_foreign_member_bindings_and_is_idempotent() {
        let store: Arc<dyn WorkGraphStore> = Arc::new(MemoryWorkGraphStore::new());
        let parent = WorkGraphService::with_scope(
            Arc::clone(&store),
            mob_realm("parent"),
            WorkNamespace::default(),
        );
        let child = WorkGraphService::with_scope(
            Arc::clone(&store),
            mob_realm("child"),
            WorkNamespace::default(),
        );
        let own = seed_legacy_binding(
            &parent,
            member_key("parent", "own-worker"),
            "stays in the parent realm",
            WorkAttentionStatus::Active,
        )
        .await;
        let active = seed_legacy_binding(
            &parent,
            member_key("child", "reviewer"),
            "child reviewer goal",
            WorkAttentionStatus::Active,
        )
        .await;
        let paused_until = chrono::Utc::now() + chrono::Duration::hours(4);
        let paused = seed_legacy_binding(
            &parent,
            member_key("child", "writer"),
            "child writer goal",
            WorkAttentionStatus::Paused {
                until: Some(paused_until),
            },
        )
        .await;

        // Dry run: reports both foreign bindings, writes nothing.
        let dry =
            migrate_member_bindings_to_mob_realms(&parent, WorkGraphRealmMigrationMode::DryRun)
                .await;
        assert_eq!(dry.outcome, WorkGraphRealmMigrationOutcome::Completed);
        assert_eq!(dry.scanned_bindings, 3);
        assert_eq!(dry.migrated.len(), 2);
        assert!(
            dry.migrated
                .iter()
                .all(|record| record.new_binding_id.is_none())
        );
        assert!(
            bindings_in(&child).await.is_empty(),
            "dry run must not write"
        );
        assert_eq!(bindings_in(&parent).await.len(), 3);

        // Off: no scan at all.
        let off =
            migrate_member_bindings_to_mob_realms(&parent, WorkGraphRealmMigrationMode::Off).await;
        assert_eq!(off.scanned_bindings, 0);
        assert!(off.migrated.is_empty());

        // Apply: the two child bindings move, the parent's own stays.
        let applied =
            migrate_member_bindings_to_mob_realms(&parent, WorkGraphRealmMigrationMode::Apply)
                .await;
        assert_eq!(applied.outcome, WorkGraphRealmMigrationOutcome::Completed);
        assert_eq!(applied.migrated.len(), 2);
        for record in &applied.migrated {
            assert_eq!(record.from_realm_id, mob_realm("parent"));
            assert_eq!(record.to_realm_id, mob_realm("child"));
            assert_eq!(record.mob_id, "child");
            assert!(record.new_binding_id.is_some());
            assert!(record.new_item_id.is_some());
        }

        let child_bindings = bindings_in(&child).await;
        assert_eq!(child_bindings.len(), 2);
        let moved_active = child_bindings
            .iter()
            .find(|binding| binding.target.owner_key().unwrap() == member_key("child", "reviewer"))
            .expect("reviewer binding moved");
        assert_eq!(moved_active.status, WorkAttentionStatus::Active);
        assert_eq!(moved_active.mode, WorkAttentionMode::Coordinate);
        assert_eq!(
            moved_active.delegated_authority,
            AttentionDelegatedAuthority::AddEvidence
        );
        let moved_item = child
            .get(None, None, moved_active.work_ref.item_id.clone())
            .await
            .expect("moved item");
        assert_eq!(moved_item.title, "child reviewer goal");
        assert_eq!(moved_item.description.as_deref(), Some("legacy"));
        assert!(moved_item.labels.contains("legacy"));
        let moved_paused = child_bindings
            .iter()
            .find(|binding| binding.target.owner_key().unwrap() == member_key("child", "writer"))
            .expect("writer binding moved");
        // Store timestamps round to whole seconds; compare at that precision.
        assert!(
            matches!(
                moved_paused.status,
                WorkAttentionStatus::Paused { until: Some(until) }
                    if (until - paused_until).num_seconds().abs() <= 1
            ),
            "paused state must carry over: {:?}",
            moved_paused.status
        );

        // The originals are stopped with their items cancelled; the parent's
        // own member binding is untouched.
        let parent_bindings = bindings_in(&parent).await;
        let by_id = |id: &WorkAttentionBindingId| {
            parent_bindings
                .iter()
                .find(|binding| binding.binding_id == *id)
                .expect("original binding row kept")
                .clone()
        };
        assert_eq!(
            by_id(&active.binding_id).status,
            WorkAttentionStatus::Stopped
        );
        assert_eq!(
            by_id(&paused.binding_id).status,
            WorkAttentionStatus::Stopped
        );
        assert_eq!(by_id(&own.binding_id).status, WorkAttentionStatus::Active);
        let cancelled = parent
            .get(None, None, active.work_ref.item_id.clone())
            .await
            .expect("original item");
        assert_eq!(cancelled.status, WorkStatus::Cancelled);

        // Idempotent: a second apply finds only terminal foreign rows.
        let again =
            migrate_member_bindings_to_mob_realms(&parent, WorkGraphRealmMigrationMode::Apply)
                .await;
        assert_eq!(again.outcome, WorkGraphRealmMigrationOutcome::Completed);
        assert!(again.migrated.is_empty());
        assert_eq!(again.skipped_terminal, 2);
        assert_eq!(bindings_in(&child).await.len(), 2);
    }

    #[tokio::test]
    async fn migration_reports_a_store_failure_instead_of_panicking() {
        let service = WorkGraphService::with_scope(
            Arc::new(meerkat::DisabledWorkGraphStore),
            mob_realm("parent"),
            WorkNamespace::default(),
        );
        let report =
            migrate_member_bindings_to_mob_realms(&service, WorkGraphRealmMigrationMode::Apply)
                .await;
        assert!(matches!(
            report.outcome,
            WorkGraphRealmMigrationOutcome::Failed { .. }
        ));
        assert!(report.migrated.is_empty());
    }
}
