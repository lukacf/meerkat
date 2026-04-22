use super::{SourceIdentityRecord, SourceIdentityStatus, SourceTransportKind, SourceUuid};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultSkillSourceKind {
    Project,
    User,
    Builtin,
}

pub fn default_source_fingerprint(kind: DefaultSkillSourceKind) -> &'static str {
    match kind {
        DefaultSkillSourceKind::Project => "default:project-filesystem",
        DefaultSkillSourceKind::User => "default:user-filesystem",
        DefaultSkillSourceKind::Builtin => "default:embedded-builtin",
    }
}

pub fn default_source_uuid(kind: DefaultSkillSourceKind) -> SourceUuid {
    let raw = match kind {
        DefaultSkillSourceKind::Project => "00000000-0000-4000-8000-000000000101",
        DefaultSkillSourceKind::User => "00000000-0000-4000-8000-000000000102",
        DefaultSkillSourceKind::Builtin => "00000000-0000-4000-8000-000000000103",
    };
    match SourceUuid::parse(raw) {
        Ok(source_uuid) => source_uuid,
        Err(_) => unreachable!("default skill source UUID must remain valid"),
    }
}

pub fn default_source_identity_record(kind: DefaultSkillSourceKind) -> SourceIdentityRecord {
    let (display_name, transport_kind) = match kind {
        DefaultSkillSourceKind::Project => ("project", SourceTransportKind::Filesystem),
        DefaultSkillSourceKind::User => ("user", SourceTransportKind::Filesystem),
        DefaultSkillSourceKind::Builtin => ("embedded", SourceTransportKind::Embedded),
    };
    SourceIdentityRecord {
        source_uuid: default_source_uuid(kind),
        display_name: display_name.to_string(),
        transport_kind,
        fingerprint: default_source_fingerprint(kind).to_string(),
        status: SourceIdentityStatus::Active,
    }
}
