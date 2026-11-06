//! Host customization of a session's implicit delegation mob definition
//! (see [`crate::MobMcpState::with_implicit_mob_definition_customizer`]).

use meerkat_core::types::SessionId;
use meerkat_mob::{MobDefinition, MobError, ProfileName};

/// What an [`ImplicitMobDefinitionCustomizer`] is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImplicitMobContext {
    /// The bridge session that owns the implicit mob (the delegating
    /// session).
    pub owner_bridge_session_id: SessionId,
    /// The model the mob's `delegate` profile runs on.
    pub model: String,
}

/// A host hook over the definition of every implicit delegation mob a
/// [`crate::MobMcpState`] creates, between
/// [`MobDefinition::implicit`] and the create (and its validation).
///
/// It may add to the definition, for example a `models[<id>]` entry for a
/// model outside the static catalog or a `provider` on the `delegate`
/// profile. It must keep the definition's `id` and the `delegate` profile's
/// `model`: the create is refused when either changed.
pub trait ImplicitMobDefinitionCustomizer: Send + Sync {
    /// Customize `definition` for `ctx`. An error refuses the create and is
    /// returned to the caller (the agent's `delegate` call).
    fn customize_implicit_mob(
        &self,
        ctx: &ImplicitMobContext,
        definition: &mut MobDefinition,
    ) -> Result<(), MobError>;
}

/// The implicit mob definition for `owner` on `model`, customized by
/// `customizer` when one is installed.
///
/// # Errors
///
/// The customizer's own error, or [`MobError::Internal`] when it changed the
/// definition's id or the `delegate` profile's model (or removed the
/// profile).
pub(crate) fn implicit_mob_definition(
    bridge_session_id: &str,
    owner: &SessionId,
    model: &str,
    customizer: Option<&dyn ImplicitMobDefinitionCustomizer>,
) -> Result<MobDefinition, MobError> {
    let mut definition = MobDefinition::implicit(bridge_session_id, model);
    let Some(customizer) = customizer else {
        return Ok(definition);
    };
    let id = definition.id.clone();
    let ctx = ImplicitMobContext {
        owner_bridge_session_id: owner.clone(),
        model: model.to_string(),
    };
    customizer.customize_implicit_mob(&ctx, &mut definition)?;
    if definition.id != id {
        return Err(MobError::Internal(format!(
            "implicit mob definition customizer changed the mob id from '{id}' to '{}'",
            definition.id
        )));
    }
    let delegate = ProfileName::from("delegate");
    match definition
        .resolve_inline_profile(&delegate)
        .map(|profile| profile.model.as_str())
    {
        Some(customized) if customized == model => Ok(definition),
        Some(customized) => Err(MobError::Internal(format!(
            "implicit mob definition customizer changed the delegate profile's model from \
             '{model}' to '{customized}'"
        ))),
        None => Err(MobError::Internal(
            "implicit mob definition customizer removed the inline delegate profile".to_string(),
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    struct Rename;
    impl ImplicitMobDefinitionCustomizer for Rename {
        fn customize_implicit_mob(
            &self,
            _ctx: &ImplicitMobContext,
            definition: &mut MobDefinition,
        ) -> Result<(), MobError> {
            definition.id = meerkat_mob::MobId::from("elsewhere");
            Ok(())
        }
    }

    struct Remodel;
    impl ImplicitMobDefinitionCustomizer for Remodel {
        fn customize_implicit_mob(
            &self,
            _ctx: &ImplicitMobContext,
            definition: &mut MobDefinition,
        ) -> Result<(), MobError> {
            for binding in definition.profiles.values_mut() {
                if let meerkat_mob::ProfileBinding::Inline(profile) = binding {
                    profile.model = "other-model".to_string();
                }
            }
            Ok(())
        }
    }

    #[test]
    fn implicit_definition_without_customizer_is_unchanged() {
        let owner = SessionId::new();
        let sid = owner.to_string();
        let definition = implicit_mob_definition(&sid, &owner, "claude-sonnet-4-5", None).unwrap();
        assert_eq!(
            definition,
            MobDefinition::implicit(&sid, "claude-sonnet-4-5")
        );
    }

    #[test]
    fn implicit_definition_customizer_cannot_change_the_id() {
        let owner = SessionId::new();
        let err = implicit_mob_definition(&owner.to_string(), &owner, "m", Some(&Rename))
            .expect_err("a renamed implicit mob is refused");
        assert!(err.to_string().contains("changed the mob id"), "{err}");
    }

    #[test]
    fn implicit_definition_customizer_cannot_change_the_delegate_model() {
        let owner = SessionId::new();
        let err = implicit_mob_definition(&owner.to_string(), &owner, "m", Some(&Remodel))
            .expect_err("a remodelled delegate profile is refused");
        assert!(
            err.to_string()
                .contains("changed the delegate profile's model from 'm' to 'other-model'"),
            "{err}"
        );
    }
}
