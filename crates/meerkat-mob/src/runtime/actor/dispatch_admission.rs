//! The tool dispatch admission of a fork seat, decided by MobMachine.
//!
//! A host's tool dispatch admission is process-local, so a fork child never
//! copies its source's: when the source's lineage carries one, the host's
//! spawn customizer is asked for the child, and the child runs under what it
//! derives or is not seated at all (see `fork_build`). The handle's fork
//! paths drive both steps before anything is forked; a local temporary
//! council branch is resolved where it is seated.

use super::*;
use crate::runtime::fork_build::{
    DerivedDispatchAdmission, SeatDispatchAdmission, customizer_dispatch_admission,
    customizer_own_identity_dispatch_admission, derive_member_dispatch_admission,
    dispatch_admission_verdict, load_resumed_member_view, rebuild_seat_resume_spec,
};

impl MobActor {
    /// MobMachine's verdict on one member build's tool dispatch admission,
    /// from what the shell observed of the build's `lineage` and the host's
    /// `host_answer`.
    pub(super) fn resolve_member_dispatch_admission_verdict(
        &mut self,
        lineage: mob_dsl::MemberDispatchAdmissionLineage,
        host_answer: mob_dsl::MemberDispatchAdmissionHostAnswer,
    ) -> Result<mob_dsl::MemberDispatchAdmissionVerdict, MobError> {
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::ResolveMemberDispatchAdmission {
                lineage,
                host_answer,
            },
            "resolve_member_dispatch_admission",
        )?;
        dispatch_admission_verdict(&effects, lineage, host_answer)
    }

    /// What this mob observes of `source`'s tool dispatch admission lineage
    /// for a fork of it taken from its session `source_session_id`: whether
    /// its build in this process runs under an admission; for a source this
    /// process has not built, its lineage record, or else whether the host's
    /// answer for its next build would carry one, without waking it (an
    /// ordinary source's rebuild request, made as its restore makes it; a
    /// fork-derived source whose spawn row records no lineage asks the host
    /// for its own identity, as its rebuild does: see
    /// `fork_build::rebuild_dispatch_admission`). A source with no session
    /// of this process (`None`: a placed member, whose admission cannot
    /// cross placement) has no build here that could run under one.
    pub(super) async fn observe_fork_source_dispatch_lineage(
        &self,
        source: &AgentIdentity,
        source_session_id: Option<&SessionId>,
    ) -> Result<mob_dsl::MemberDispatchAdmissionLineage, MobError> {
        use mob_dsl::MemberDispatchAdmissionLineage as Lineage;
        if let Some(present) = self
            .per_spawn_external_tools
            .read()
            .await
            .dispatch_admission(source)
        {
            return Ok(if present {
                Lineage::SourceBuiltGated
            } else {
                Lineage::SourceBuiltUngated
            });
        }
        let (entry, seat_build) = {
            let roster = self.roster.read().await;
            let entry = roster
                .get(source)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(source.clone()))?;
            (entry, roster.next_build(source))
        };
        if entry.dispatch_admission_lineage {
            return Ok(Lineage::Recorded);
        }
        let Some(source_session_id) = source_session_id else {
            return Ok(Lineage::SourceRebuildUngated);
        };
        let next_build_admission = if entry.fork_source.is_some() {
            customizer_own_identity_dispatch_admission(
                &self.definition.id,
                self.spawn_member_customizer.as_ref(),
                &entry,
                source_session_id,
            )?
        } else {
            let view = if self.spawn_member_customizer.is_some() {
                load_resumed_member_view(self.session_service.as_ref(), source_session_id).await?
            } else {
                super::super::handle::ResumedMemberView::new(source_session_id.clone(), None)
            };
            rebuild_seat_resume_spec(
                &self.definition.id,
                self.spawn_member_customizer.as_ref(),
                &entry,
                &view,
                seat_build.as_deref(),
            )?
            .tool_dispatch_admission
        };
        Ok(if next_build_admission.is_some() {
            Lineage::SourceRebuildGated
        } else {
            Lineage::SourceRebuildUngated
        })
    }

    /// Whether a prompt-context fork `child` of `source` (a spawn whose
    /// launch mode forks `source`'s history, which the host's spawn
    /// customizer has answered) is seated from a lineage that runs under a
    /// tool dispatch admission, decided by MobMachine from `source`'s
    /// observed lineage and the admission the customizer installed on the
    /// request; a child of such a source the customizer installed none for
    /// is refused ([`MobError::DispatchAdmissionRequired`]).
    pub(super) async fn prompt_fork_dispatch_lineage(
        &mut self,
        source: &AgentIdentity,
        child: &super::super::handle::SpawnMemberSpec,
    ) -> Result<bool, MobError> {
        let source_session = self
            .roster
            .read()
            .await
            .get(source)
            .ok_or_else(|| MobError::MemberNotFound(source.clone()))?
            .member_ref
            .bridge_session_id()
            .cloned();
        let lineage = self
            .observe_fork_source_dispatch_lineage(source, source_session.as_ref())
            .await?;
        let answered = child.tool_dispatch_admission.clone();
        match derive_member_dispatch_admission(
            |lineage, host_answer| {
                self.resolve_member_dispatch_admission_verdict(lineage, host_answer)
            },
            lineage,
            || Ok(answered),
        )? {
            DerivedDispatchAdmission::NoneRequired => Ok(false),
            DerivedDispatchAdmission::Inherit(_) => Ok(true),
            DerivedDispatchAdmission::Refused => Err(MobError::DispatchAdmissionRequired {
                member_id: child.identity.clone(),
            }),
        }
    }

    /// The tool dispatch admission of one fork seat of `child`, from the
    /// observed `lineage` of its source, decided by MobMachine: a lineage
    /// that carries none leaves the child as its request is; one that
    /// carries an admission asks the host's spawn customizer for the child
    /// (asked in `spawn_source`, by `spawner`), and the child is seated under
    /// what it derives, or not at all
    /// ([`MobError::DispatchAdmissionRequired`]).
    pub(super) async fn resolve_seat_dispatch_admission(
        &mut self,
        lineage: mob_dsl::MemberDispatchAdmissionLineage,
        child: &super::super::handle::SpawnMemberSpec,
        spawn_source: super::super::handle::SpawnSource,
        spawner: Option<AgentIdentity>,
    ) -> Result<SeatDispatchAdmission, MobError> {
        let spawner_runtime_id = match spawner.as_ref() {
            Some(spawner) => self
                .roster
                .read()
                .await
                .get(spawner)
                .map(|entry| entry.agent_runtime_id.clone()),
            None => None,
        };
        let customizer = self.spawn_member_customizer.clone();
        let ctx = super::super::handle::SpawnCustomizationContext {
            mob_id: self.definition.id.clone(),
            spawn_source,
            spawner_identity: spawner,
            spawner_runtime_id,
            requested_profile: child.role_name.clone(),
        };
        let derived = derive_member_dispatch_admission(
            |lineage, host_answer| {
                self.resolve_member_dispatch_admission_verdict(lineage, host_answer)
            },
            lineage,
            || customizer_dispatch_admission(customizer.as_ref(), &ctx, child),
        )?;
        match derived {
            DerivedDispatchAdmission::NoneRequired => Ok(SeatDispatchAdmission::default()),
            DerivedDispatchAdmission::Inherit(admission) => Ok(SeatDispatchAdmission {
                admission: Some(admission),
                lineage_required: true,
            }),
            DerivedDispatchAdmission::Refused => Err(MobError::DispatchAdmissionRequired {
                member_id: child.identity.clone(),
            }),
        }
    }
}
