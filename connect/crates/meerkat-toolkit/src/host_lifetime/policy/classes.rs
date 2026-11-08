//! Finite application-owned member classes, before any concrete admission.
//! Factory names select trusted host registrations; they are not executable
//! paths, code, or permission to instantiate an arbitrary tool or member.
use super::{HostLifetimeError, PreparedNativeMember};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_core::{CompiledMemberToolGrant, CompiledMemberToolGrants, RealmId, ToolDef};
use serde::{Deserialize, Serialize};
use toolkit_package::Id;

/// Exact callback declaration consumed by the selected trusted host factory.
/// Native preparation still compares its actual resulting tool surface.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledClassCallback {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl CompiledClassCallback {
    pub fn validate(&self) -> Result<(), HostLifetimeError> {
        if !canonical_text(&self.name) || !self.input_schema.is_object() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(())
    }

    /// Project declared data only. This does not resolve a factory or execute it.
    pub fn definition(&self) -> Result<ToolDef, HostLifetimeError> {
        self.validate()?;
        Ok(ToolDef::new(
            self.name.clone(),
            self.description.clone(),
            self.input_schema.clone(),
        ))
    }
}

/// A packaged class may have no current instances. Its grant template has no
/// placeholder member identity and cannot name a native parent or session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledMemberClass {
    pub id: Id,
    /// Host-selected construction purpose. Ordinary classes cannot become
    /// workflow leads by supplying a role, route, or model-authored argument.
    #[serde(
        default,
        skip_serializing_if = "CompiledMemberClassPurpose::is_ordinary"
    )]
    pub purpose: CompiledMemberClassPurpose,
    /// Exact native inline profile, also used as the concrete member's role.
    pub profile: String,
    pub factory: Id,
    /// Trusted host declaration shared by these callbacks. Absence is unknown,
    /// never evidence that the callbacks act without an account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callback_account: Option<CompiledClassCallbackAccount>,
    pub callbacks: Vec<CompiledClassCallback>,
    pub grants: Vec<CompiledMemberToolGrant>,
}

/// This selects a construction contract, not native operation authority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompiledMemberClassPurpose {
    #[default]
    Ordinary,
    SharedWorkflow,
}
impl CompiledMemberClassPurpose {
    pub fn is_ordinary(&self) -> bool {
        *self == Self::Ordinary
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompiledClassCallbackAccount {
    DeclaredNone,
    Selected { authority: String, subject: String },
}

impl CompiledClassCallbackAccount {
    pub fn native(&self) -> Result<meerkat_core::OperationAccount, HostLifetimeError> {
        match self {
            Self::DeclaredNone => Ok(meerkat_core::OperationAccount::DeclaredNone),
            Self::Selected { authority, subject } => {
                meerkat_core::QualifiedAccount::new(authority.clone(), subject.clone())
                    .map(meerkat_core::OperationAccount::Selected)
                    .map_err(|_| HostLifetimeError::InvalidSelection)
            }
        }
    }
}

/// The trusted application owner supplies a concrete identity and destination.
/// Native admission must separately authenticate that owner and issue routes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledMemberClassInstance {
    pub class: Id,
    pub member: PreparedNativeMember,
}

impl CompiledMemberClass {
    pub fn validate(&self) -> Result<(), HostLifetimeError> {
        if let Some(account) = &self.callback_account {
            account.native()?;
        }
        if !canonical_text(&self.profile)
            || self
                .callbacks
                .windows(2)
                .any(|rows| rows[0].name >= rows[1].name)
            || self
                .grants
                .windows(2)
                .any(|rows| rows[0].tool_name >= rows[1].tool_name)
            || self
                .grants
                .iter()
                .any(|grant| !canonical_text(&grant.tool_name))
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        for callback in &self.callbacks {
            callback.validate()?;
        }
        Ok(())
    }

    /// Pure instantiation of the existing native grant row. No live policy is
    /// changed and no member is constructed. The canonical policy owner must
    /// validate the full successor and keep it withheld until native finish.
    pub fn materialize(
        &self,
        realm: &RealmId,
        instance: &CompiledMemberClassInstance,
    ) -> Result<CompiledMemberToolGrants, HostLifetimeError> {
        self.validate()?;
        let selected = &instance.member;
        if instance.class != self.id
            || selected.member.role != self.profile
            || selected.member.comms_name().is_err()
            || meerkat_core::mob_realm_id(&selected.member.mob_id)
                .as_ref()
                .ok()
                != Some(realm)
            || !canonical_text(&selected.member.member)
            || selected.logical_owner.validate_qualified().is_err()
            || selected.context_generation == 0
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let audience_owner = match &selected.audience {
            AudienceRef::Principal { principal } => principal,
            AudienceRef::Destination {
                authority,
                destination_id,
            } => {
                if destination_id.is_empty() {
                    return Err(HostLifetimeError::InvalidSelection);
                }
                authority
            }
        };
        audience_owner
            .validate_qualified()
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        Ok(CompiledMemberToolGrants {
            member_identity: selected.member.member.clone(),
            grants: self.grants.clone(),
        })
    }
}

/// Preserve exact declaration order and reject duplicate class names instead
/// of silently replacing one template with another.
pub fn validate_member_classes(classes: &[CompiledMemberClass]) -> Result<(), HostLifetimeError> {
    if classes.windows(2).any(|rows| rows[0].id >= rows[1].id) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    for class in classes {
        class.validate()?;
    }
    Ok(())
}

fn canonical_text(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        CompiledMemberToolAction, CompiledToolConsequence, MobMemberBinding, PrincipalKind,
        PrincipalRef, TrustDomainId,
    };
    use serde_json::json;

    fn class() -> CompiledMemberClass {
        CompiledMemberClass {
            purpose: Default::default(),
            id: Id::new("project-owner").unwrap(),
            profile: "project-owner".into(),
            factory: Id::new("host:project-owner").unwrap(),
            callback_account: None,
            callbacks: vec![CompiledClassCallback {
                name: "project_read".into(),
                description: "Read the selected project".into(),
                input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            }],
            grants: vec![CompiledMemberToolGrant {
                tool_name: "project_read".into(),
                action: CompiledMemberToolAction::Invoke,
                consequence: CompiledToolConsequence::R0,
            }],
        }
    }
    fn instance() -> CompiledMemberClassInstance {
        let owner = PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "application",
            TrustDomainId::new("application").unwrap(),
        )
        .unwrap();
        CompiledMemberClassInstance {
            class: class().id,
            member: PreparedNativeMember {
                member: MobMemberBinding {
                    mob_id: "application".into(),
                    member: "project-one".into(),
                    role: "project-owner".into(),
                },
                logical_owner: owner.clone(),
                context_generation: 1,
                audience: AudienceRef::Destination {
                    authority: owner,
                    destination_id: "project-one".into(),
                },
                setup_capability: None,
            },
        }
    }

    #[test]
    fn callback_account_absence_preserves_serialization_and_never_means_none() {
        let selected = class();
        let value = serde_json::to_value(&selected).unwrap();
        assert!(value.get("callback_account").is_none());
        assert!(
            value.get("purpose").is_none(),
            "legacy canonical bytes omit Ordinary"
        );
        let decoded = serde_json::from_value::<CompiledMemberClass>(value.clone()).unwrap();
        assert_eq!(decoded.purpose, CompiledMemberClassPurpose::Ordinary);
        assert_eq!(
            toolkit_package::canonical_json(&decoded).unwrap(),
            toolkit_package::canonical_json(&selected).unwrap()
        );
        let mut workflow = selected.clone();
        workflow.purpose = CompiledMemberClassPurpose::SharedWorkflow;
        let explicit_workflow = serde_json::to_value(&workflow).unwrap();
        assert_eq!(explicit_workflow["purpose"], "shared_workflow");
        assert_eq!(
            serde_json::from_value::<CompiledMemberClass>(explicit_workflow).unwrap(),
            workflow
        );
        let mut unknown = value;
        unknown["purpose"] = json!("implicit_helper");
        assert!(serde_json::from_value::<CompiledMemberClass>(unknown).is_err());
        let mut explicit = selected;
        explicit.callback_account = Some(CompiledClassCallbackAccount::DeclaredNone);
        explicit.validate().unwrap();
        assert_eq!(
            explicit
                .callback_account
                .as_ref()
                .unwrap()
                .native()
                .unwrap(),
            meerkat_core::OperationAccount::DeclaredNone
        );
        explicit.callback_account = Some(CompiledClassCallbackAccount::Selected {
            authority: "".into(),
            subject: "project-account".into(),
        });
        assert_eq!(
            explicit.validate(),
            Err(HostLifetimeError::InvalidSelection)
        );
        explicit.callback_account = Some(CompiledClassCallbackAccount::Selected {
            authority: "https://project.example".into(),
            subject: "project-account".into(),
        });
        explicit.validate().unwrap();
        let mut value = serde_json::to_value(&explicit).unwrap();
        value["callback_account"]["credential"] = json!("not a declaration field");
        assert!(serde_json::from_value::<CompiledMemberClass>(value).is_err());
    }

    #[test]
    fn class_has_no_placeholder_and_materializes_only_the_concrete_identity() {
        let class = class();
        validate_member_classes(std::slice::from_ref(&class)).unwrap();
        let before = toolkit_package::canonical_json(&class).unwrap();
        assert!(
            serde_json::to_value(&class)
                .unwrap()
                .get("member_identity")
                .is_none()
        );
        let instance = instance();
        let row = class
            .materialize(
                &meerkat_core::mob_realm_id("application").unwrap(),
                &instance,
            )
            .unwrap();
        assert_eq!(row.member_identity, instance.member.member.member);
        assert_eq!(row.grants, class.grants);
        assert_eq!(toolkit_package::canonical_json(&class).unwrap(), before);
    }

    #[test]
    fn foreign_class_role_realm_and_invalid_destination_are_refused() {
        let class = class();
        let realm = meerkat_core::mob_realm_id("application").unwrap();
        let mut foreign = instance();
        foreign.class = Id::new("unknown").unwrap();
        assert!(class.materialize(&realm, &foreign).is_err());
        let mut foreign = instance();
        foreign.member.member.role = "other".into();
        assert!(class.materialize(&realm, &foreign).is_err());
        assert!(
            class
                .materialize(&meerkat_core::mob_realm_id("other").unwrap(), &instance())
                .is_err()
        );
        let mut invalid = instance();
        invalid.member.context_generation = 0;
        assert!(class.materialize(&realm, &invalid).is_err());
        let mut invalid = instance();
        if let AudienceRef::Destination { destination_id, .. } = &mut invalid.member.audience {
            destination_id.clear();
        }
        assert!(class.materialize(&realm, &invalid).is_err());
    }

    #[test]
    fn duplicate_and_unknown_schema_fields_fail_closed() {
        let mut selected = class();
        assert!(validate_member_classes(&[selected.clone(), selected.clone()]).is_err());
        selected.callbacks.push(selected.callbacks[0].clone());
        assert!(selected.validate().is_err());
        let mut value = serde_json::to_value(instance()).unwrap();
        value["source_code"] = json!("untrusted code");
        assert!(serde_json::from_value::<CompiledMemberClassInstance>(value).is_err());
        let mut value = serde_json::to_value(class()).unwrap();
        value["callbacks"][0]["command"] = json!("untrusted command");
        assert!(serde_json::from_value::<CompiledMemberClass>(value).is_err());
        let mut invalid = class();
        invalid.callbacks[0].input_schema = json!([]);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn shared_factory_does_not_combine_class_grant_templates() {
        let first = class();
        let mut second = first.clone();
        second.id = Id::new("project-reader").unwrap();
        second.grants.clear();
        validate_member_classes(&[first.clone(), second.clone()]).unwrap();
        let mut selected = instance();
        selected.class = second.id.clone();
        let grants = second
            .materialize(
                &meerkat_core::mob_realm_id("application").unwrap(),
                &selected,
            )
            .unwrap();
        assert!(grants.grants.is_empty());
        assert!(!first.grants.is_empty());
    }
}
