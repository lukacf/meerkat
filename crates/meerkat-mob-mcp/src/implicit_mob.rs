//! The host's answer for a session's implicit delegation mob
//! (see [`crate::MobMcpState::with_implicit_mob_customizer`]).

use meerkat_core::Provider;
use meerkat_core::types::SessionId;
use meerkat_mob::{MobDefinition, MobError, ProfileName};

/// What an [`ImplicitMobCustomizer`] is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImplicitMobContext {
    /// The bridge session that owns the implicit mob (the delegating
    /// session).
    pub owner_bridge_session_id: SessionId,
    /// The model the mob's `delegate` profile runs on.
    pub model: String,
}

/// A host's answer to the one question the engine asks before it creates a
/// session's implicit delegation mob: which provider runs the `delegate`
/// profile's model. The engine builds the definition
/// ([`MobDefinition::implicit`]) and applies the answer itself; the host
/// never sees or edits the definition.
pub trait ImplicitMobCustomizer: Send + Sync {
    /// The provider of the `delegate` profile for `ctx.model`, when the host
    /// knows one the static model catalog does not. `None` leaves the
    /// definition exactly [`MobDefinition::implicit`].
    ///
    /// # Errors
    ///
    /// An error refuses the create (nothing is created) and is returned to
    /// the caller, for example the agent's `delegate` call.
    fn delegate_provider(&self, ctx: &ImplicitMobContext) -> Result<Option<Provider>, MobError>;
}

/// The implicit mob definition for `owner` on `model`, its `delegate`
/// profile's provider set from `customizer`'s answer when one is installed
/// and answers.
///
/// # Errors
///
/// The customizer's own error, or [`MobError::Internal`] when
/// [`MobDefinition::implicit`] has no inline `delegate` profile to apply a
/// provider to (an engine invariant).
pub(crate) fn implicit_mob_definition(
    bridge_session_id: &str,
    owner: &SessionId,
    model: &str,
    customizer: Option<&dyn ImplicitMobCustomizer>,
) -> Result<MobDefinition, MobError> {
    let mut definition = MobDefinition::implicit(bridge_session_id, model);
    let Some(customizer) = customizer else {
        return Ok(definition);
    };
    let ctx = ImplicitMobContext {
        owner_bridge_session_id: owner.clone(),
        model: model.to_string(),
    };
    let Some(provider) = customizer.delegate_provider(&ctx)? else {
        return Ok(definition);
    };
    let profile = definition
        .profiles
        .get_mut(&ProfileName::from("delegate"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .ok_or_else(|| {
            MobError::Internal(
                "the implicit mob definition has no inline delegate profile".to_string(),
            )
        })?;
    profile.provider = Some(provider);
    Ok(definition)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    struct Answer(Option<Provider>);
    impl ImplicitMobCustomizer for Answer {
        fn delegate_provider(
            &self,
            ctx: &ImplicitMobContext,
        ) -> Result<Option<Provider>, MobError> {
            assert_eq!(ctx.model, "endpoint-only-model");
            Ok(self.0)
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
    fn implicit_definition_with_no_answer_is_unchanged() {
        let owner = SessionId::new();
        let sid = owner.to_string();
        let definition =
            implicit_mob_definition(&sid, &owner, "endpoint-only-model", Some(&Answer(None)))
                .unwrap();
        assert_eq!(
            definition,
            MobDefinition::implicit(&sid, "endpoint-only-model")
        );
    }

    #[test]
    fn implicit_definition_carries_the_answered_delegate_provider_only() {
        let owner = SessionId::new();
        let sid = owner.to_string();
        let definition = implicit_mob_definition(
            &sid,
            &owner,
            "endpoint-only-model",
            Some(&Answer(Some(Provider::OpenAI))),
        )
        .unwrap();
        let mut expected = MobDefinition::implicit(&sid, "endpoint-only-model");
        expected
            .profiles
            .get_mut(&ProfileName::from("delegate"))
            .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
            .unwrap()
            .provider = Some(Provider::OpenAI);
        assert_eq!(definition, expected);
    }
}
