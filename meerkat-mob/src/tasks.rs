//! Shared task board for mob coordination.
//!
//! The `TaskBoard` is a projection built from `TaskCreated` and `TaskUpdated`
//! events. It provides the current view of all tasks in a mob.

use crate::MobError;
use crate::event::NewMobEvent;
use crate::event::{MobEvent, MobEventKind};
use crate::ids::{MeerkatId, MobId, TaskId};
use crate::store::MobEventStore;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Task lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is open and available for claiming.
    Open,
    /// Task is currently being worked on.
    InProgress,
    /// Task has been completed successfully.
    Completed,
    /// Task has been cancelled.
    Cancelled,
}

/// A task on the shared task board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobTask {
    /// Unique task identifier.
    pub id: TaskId,
    /// Short subject line.
    pub subject: String,
    /// Detailed description.
    pub description: String,
    /// Current status.
    pub status: TaskStatus,
    /// Assigned owner (meerkat ID), if any.
    pub owner: Option<MeerkatId>,
    /// Task IDs that block this task.
    pub blocked_by: Vec<TaskId>,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
    /// When the task was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Projected view of all tasks in a mob, built from events.
#[derive(Debug, Clone, Default)]
pub struct TaskBoard {
    tasks: BTreeMap<TaskId, MobTask>,
}

/// Explicit task-board owner that validates updates and maintains the projection.
#[derive(Clone)]
pub struct MobTaskBoardService {
    mob_id: MobId,
    board: Arc<RwLock<TaskBoard>>,
    events: Arc<dyn MobEventStore>,
}

impl MobTaskBoardService {
    pub fn new(
        mob_id: MobId,
        board: Arc<RwLock<TaskBoard>>,
        events: Arc<dyn MobEventStore>,
    ) -> Self {
        Self {
            mob_id,
            board,
            events,
        }
    }

    pub async fn create_task(
        &self,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    ) -> Result<TaskId, MobError> {
        if subject.trim().is_empty() {
            return Err(MobError::Internal(
                "task subject cannot be empty".to_string(),
            ));
        }

        let task_id = TaskId::from(uuid::Uuid::new_v4().to_string());
        let appended = self
            .events
            .append(NewMobEvent {
                mob_id: self.mob_id.clone(),
                timestamp: None,
                kind: MobEventKind::TaskCreated {
                    task_id: task_id.clone(),
                    subject,
                    description,
                    blocked_by,
                },
            })
            .await?;
        self.board.write().await.apply(&appended);
        Ok(task_id)
    }

    pub async fn update_task(
        &self,
        task_id: TaskId,
        status: TaskStatus,
        owner: Option<MeerkatId>,
    ) -> Result<(), MobError> {
        // We treat `owner` as an *optional* claim/mutation field.
        //
        // Contract:
        // - Owner changes are only applied when `status == in_progress`.
        // - For other status transitions (open/completed/cancelled), any provided
        //   `owner` value is ignored and the current owner is preserved.
        //
        // Rationale: some tool-schema layers may erroneously require sending
        // `owner` even when completing/cancelling a task. Ignoring `owner` for
        // non-in_progress transitions avoids spurious failures while still
        // preventing owner changes outside of in_progress.
        let effective_owner = {
            let board = self.board.read().await;
            let task = board
                .get(&task_id)
                .ok_or_else(|| MobError::Internal(format!("task '{task_id}' not found")))?;
            let current_owner = task.owner.clone();

            if matches!(status, TaskStatus::InProgress) {
                if let Some(new_owner) = owner {
                    let blocked = task.blocked_by.iter().any(|dependency| {
                        board.get(dependency).map(|t| t.status) != Some(TaskStatus::Completed)
                    });
                    if blocked {
                        return Err(MobError::Internal(format!(
                            "task '{task_id}' is blocked by incomplete dependencies"
                        )));
                    }
                    Some(new_owner)
                } else {
                    // No owner supplied: preserve current owner (if any).
                    current_owner
                }
            } else {
                // Owner is not mutable for non-in_progress statuses.
                current_owner
            }
        };

        let appended = self
            .events
            .append(NewMobEvent {
                mob_id: self.mob_id.clone(),
                timestamp: None,
                kind: MobEventKind::TaskUpdated {
                    task_id,
                    status,
                    owner: effective_owner,
                },
            })
            .await?;
        self.board.write().await.apply(&appended);
        Ok(())
    }

    pub async fn clear(&self) {
        self.board.write().await.clear();
    }
}

impl TaskBoard {
    /// Build a `TaskBoard` from a sequence of mob events.
    ///
    /// Only `TaskCreated` and `TaskUpdated` events are considered.
    pub fn project(events: &[MobEvent]) -> Self {
        let mut board = Self::default();
        for event in events {
            board.apply(event);
        }
        board
    }

    /// Apply a single event to update the task board state.
    pub fn apply(&mut self, event: &MobEvent) {
        match &event.kind {
            MobEventKind::TaskCreated {
                task_id,
                subject,
                description,
                blocked_by,
            } => {
                self.tasks.insert(
                    task_id.clone(),
                    MobTask {
                        id: task_id.clone(),
                        subject: subject.clone(),
                        description: description.clone(),
                        status: TaskStatus::Open,
                        owner: None,
                        blocked_by: blocked_by.clone(),
                        created_at: event.timestamp,
                        updated_at: event.timestamp,
                    },
                );
            }
            MobEventKind::TaskUpdated {
                task_id,
                status,
                owner,
            } => {
                if let Some(task) = self.tasks.get_mut(task_id) {
                    task.status = *status;
                    task.owner = owner.clone();
                    task.updated_at = event.timestamp;
                } else {
                    tracing::warn!(
                        task_id = %task_id,
                        cursor = event.cursor,
                        "task update ignored for unknown task id"
                    );
                }
            }
            MobEventKind::MobReset => {
                self.tasks.clear();
            }
            _ => {}
        }
    }

    /// Get a task by ID.
    pub fn get(&self, task_id: &TaskId) -> Option<&MobTask> {
        self.tasks.get(task_id)
    }

    /// List all tasks.
    pub fn list(&self) -> impl Iterator<Item = &MobTask> {
        self.tasks.values()
    }

    /// Number of tasks on the board.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the board is empty.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Remove all tasks from the board.
    pub fn clear(&mut self) {
        self.tasks.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MobId;
    use crate::store::InMemoryMobEventStore;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn make_event(cursor: u64, kind: MobEventKind) -> MobEvent {
        MobEvent {
            cursor,
            timestamp: Utc::now(),
            mob_id: MobId::from("test-mob"),
            kind,
        }
    }

    #[test]
    fn test_task_status_serde_roundtrip() {
        for status in [
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::Completed,
            TaskStatus::Cancelled,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let parsed: TaskStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_mob_task_serde_roundtrip() {
        let task = MobTask {
            id: TaskId::from("task-001"),
            subject: "Build widget".to_string(),
            description: "A detailed description".to_string(),
            status: TaskStatus::InProgress,
            owner: Some(MeerkatId::from("agent-1")),
            blocked_by: vec![TaskId::from("task-000")],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&task).unwrap();
        let parsed: MobTask = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, task.id);
        assert_eq!(parsed.status, TaskStatus::InProgress);
        assert_eq!(parsed.owner, Some(MeerkatId::from("agent-1")));
    }

    #[test]
    fn test_task_board_project_empty() {
        let board = TaskBoard::project(&[]);
        assert!(board.is_empty());
        assert_eq!(board.len(), 0);
    }

    #[test]
    fn test_task_board_project_create() {
        let events = vec![make_event(
            1,
            MobEventKind::TaskCreated {
                task_id: TaskId::from("t1"),
                subject: "Task 1".to_string(),
                description: "Do something".to_string(),
                blocked_by: vec![],
            },
        )];
        let board = TaskBoard::project(&events);
        assert_eq!(board.len(), 1);
        let task_id = TaskId::from("t1");
        let task = board.get(&task_id).unwrap();
        assert_eq!(task.subject, "Task 1");
        assert_eq!(task.status, TaskStatus::Open);
        assert!(task.owner.is_none());
    }

    #[test]
    fn test_task_board_project_create_and_update() {
        let events = vec![
            make_event(
                1,
                MobEventKind::TaskCreated {
                    task_id: TaskId::from("t1"),
                    subject: "Task 1".to_string(),
                    description: "Do something".to_string(),
                    blocked_by: vec![TaskId::from("t0")],
                },
            ),
            make_event(
                2,
                MobEventKind::TaskUpdated {
                    task_id: TaskId::from("t1"),
                    status: TaskStatus::InProgress,
                    owner: Some(MeerkatId::from("agent-1")),
                },
            ),
            make_event(
                3,
                MobEventKind::TaskUpdated {
                    task_id: TaskId::from("t1"),
                    status: TaskStatus::Completed,
                    owner: Some(MeerkatId::from("agent-1")),
                },
            ),
        ];
        let board = TaskBoard::project(&events);
        let task_id = TaskId::from("t1");
        let task = board.get(&task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.owner, Some(MeerkatId::from("agent-1")));
        assert_eq!(task.blocked_by, vec![TaskId::from("t0")]);
    }

    #[test]
    fn test_task_board_ignores_non_task_events() {
        let events = vec![
            make_event(1, MobEventKind::MobCompleted),
            make_event(
                2,
                MobEventKind::PeersWired {
                    a: MeerkatId::from("a"),
                    b: MeerkatId::from("b"),
                },
            ),
        ];
        let board = TaskBoard::project(&events);
        assert!(board.is_empty());
    }

    #[test]
    fn test_task_board_update_nonexistent_task_is_noop() {
        let events = vec![make_event(
            1,
            MobEventKind::TaskUpdated {
                task_id: TaskId::from("nonexistent"),
                status: TaskStatus::Completed,
                owner: None,
            },
        )];
        let board = TaskBoard::project(&events);
        assert!(board.is_empty());
    }

    #[test]
    fn test_task_board_multiple_tasks() {
        let events = vec![
            make_event(
                1,
                MobEventKind::TaskCreated {
                    task_id: TaskId::from("t1"),
                    subject: "Task 1".to_string(),
                    description: "First".to_string(),
                    blocked_by: vec![],
                },
            ),
            make_event(
                2,
                MobEventKind::TaskCreated {
                    task_id: TaskId::from("t2"),
                    subject: "Task 2".to_string(),
                    description: "Second".to_string(),
                    blocked_by: vec![TaskId::from("t1")],
                },
            ),
        ];
        let board = TaskBoard::project(&events);
        assert_eq!(board.len(), 2);
        let tasks: Vec<_> = board.list().collect();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_task_board_idempotent_replay() {
        let events = vec![
            make_event(
                1,
                MobEventKind::TaskCreated {
                    task_id: TaskId::from("t1"),
                    subject: "Task 1".to_string(),
                    description: "First".to_string(),
                    blocked_by: vec![],
                },
            ),
            make_event(
                2,
                MobEventKind::TaskUpdated {
                    task_id: TaskId::from("t1"),
                    status: TaskStatus::Completed,
                    owner: None,
                },
            ),
        ];
        let board1 = TaskBoard::project(&events);
        let board2 = TaskBoard::project(&events);
        let task_id = TaskId::from("t1");
        assert_eq!(
            board1.get(&task_id).unwrap().status,
            board2.get(&task_id).unwrap().status
        );
    }

    #[tokio::test]
    async fn task_board_service_validates_dependency_gated_claims() {
        let board = Arc::new(RwLock::new(TaskBoard::default()));
        let service = MobTaskBoardService::new(
            MobId::from("service-mob"),
            board.clone(),
            Arc::new(InMemoryMobEventStore::new()),
        );

        let blocker = service
            .create_task("Blocker".into(), "done first".into(), vec![])
            .await
            .expect("create blocker");
        let blocked = service
            .create_task(
                "Blocked".into(),
                "done second".into(),
                vec![blocker.clone()],
            )
            .await
            .expect("create blocked task");

        let err = service
            .update_task(
                blocked.clone(),
                TaskStatus::InProgress,
                Some(MeerkatId::from("worker-1")),
            )
            .await
            .expect_err("blocked task claim should be rejected");
        assert!(
            err.to_string()
                .contains("blocked by incomplete dependencies")
        );

        service
            .update_task(blocker, TaskStatus::Completed, None)
            .await
            .expect("complete blocker");
        service
            .update_task(
                blocked.clone(),
                TaskStatus::InProgress,
                Some(MeerkatId::from("worker-1")),
            )
            .await
            .expect("claim unblocked task");

        let board = board.read().await;
        assert_eq!(
            board.get(&blocked).expect("blocked task snapshot").owner,
            Some(MeerkatId::from("worker-1"))
        );
    }
}
