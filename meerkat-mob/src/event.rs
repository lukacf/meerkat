use chrono::{DateTime, Utc};

use serde::{Deserialize, Serialize};

use crate::error::Diagnostic;
use crate::ids::{MeerkatId, MobId, ProfileName};
use crate::tasks::TaskStatus;
use meerkat_core::types::SessionId;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MobEvent {
    pub cursor: u64,
    pub timestamp: DateTime<Utc>,
    pub mob_id: MobId,
    pub kind: MobEventKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NewMobEvent {
    pub mob_id: MobId,
    pub kind: MobEventKind,
    pub timestamp: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MobEventKind {
    MobCreated {
        definition: crate::definition::MobDefinition,
    },
    MobCompleted,

    MeerkatSpawned {
        meerkat_id: MeerkatId,
        role: ProfileName,
        session_id: SessionId,
    },
    MeerkatRetired {
        meerkat_id: MeerkatId,
        role: ProfileName,
        session_id: SessionId,
    },

    PeersWired {
        a: MeerkatId,
        b: MeerkatId,
    },
    PeersUnwired {
        a: MeerkatId,
        b: MeerkatId,
    },

    TaskCreated {
        task_id: String,
        subject: String,
        description: String,
        blocked_by: Vec<String>,
    },
    TaskUpdated {
        task_id: String,
        status: Option<TaskStatus>,
        owner: Option<MeerkatId>,
    },
    DefinitionDiagnostic(Diagnostic),
}
