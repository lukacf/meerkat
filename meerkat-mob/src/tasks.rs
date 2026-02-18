use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::MeerkatId;
use crate::event::MobEvent;
use crate::event::MobEventKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TaskStatus {
    Planned,
    Open,
    InProgress,
    Blocked,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MobTask {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: TaskStatus,
    pub owner: Option<MeerkatId>,
    pub blocked_by: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskBoard {
    tasks: BTreeMap<String, MobTask>,
}

impl TaskBoard {
    pub fn project(events: &[MobEvent]) -> Self {
        let mut board = Self::default();
        for event in events {
            match &event.kind {
                MobEventKind::TaskCreated {
                    task_id,
                    subject,
                    description,
                    blocked_by,
                } => {
                    let now = event.timestamp;
                    board.tasks.insert(
                        task_id.clone(),
                        MobTask {
                            id: task_id.clone(),
                            subject: subject.clone(),
                            description: description.clone(),
                            status: TaskStatus::Open,
                            owner: None,
                            blocked_by: blocked_by.clone(),
                            created_at: now,
                            updated_at: now,
                        },
                    );
                }
                MobEventKind::TaskUpdated {
                    task_id,
                    status,
                    owner,
                } => {
                    if let Some(task) = board.tasks.get_mut(task_id) {
                        task.status = status.clone();
                        task.owner = owner.clone();
                        task.updated_at = event.timestamp;
                    }
                }
                _ => {}
            }
        }
        board
    }

    pub fn list(&self) -> Vec<MobTask> {
        self.tasks.values().cloned().collect()
    }

    pub fn get(&self, task_id: &str) -> Option<MobTask> {
        self.tasks.get(task_id).cloned()
    }

    pub fn create(&mut self, task_id: String, subject: String, description: String, blocked_by: Vec<String>) -> &mut MobTask {
        let now = Utc::now();
        let entry = MobTask {
            id: task_id.clone(),
            subject,
            description,
            status: TaskStatus::Open,
            owner: None,
            blocked_by,
            created_at: now,
            updated_at: now,
        };
        self.tasks.entry(task_id).or_insert(entry)
    }

    pub fn update(&mut self, task_id: &str, status: TaskStatus, owner: Option<MeerkatId>) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.status = status;
            task.owner = owner;
            task.updated_at = Utc::now();
        }
    }
}
