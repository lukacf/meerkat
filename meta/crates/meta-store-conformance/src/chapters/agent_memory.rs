//! `AgentMemoryProvider` capability-matrix chapter.
//!
//! Every `supports_*()` capability flag is backed by a behavioral test: a
//! provider advertising a capability must pass the corresponding operation,
//! and a provider refusing it must return the typed
//! `AgentMemoryError::Unsupported` — never a silent no-op or an untyped
//! error. The same chapter runs unchanged against the all-capable
//! `SqliteAgentMemoryStore` and the recall/remember/forget-only reference
//! provider.

use meerkat_meta::identity_first::{
    AgentIdentity, AgentMemoryError, AgentMemoryRecallRequest, AgentMemorySelection, NewAgentMemory,
};
use meerkat_meta::memory::{ManifestTier, MemoryAuthor, MemoryScope};
use meerkat_store_conformance::ConformanceFailure;

use crate::factory::AgentMemoryProviderFactory;
use crate::fixtures;
use crate::steps::Steps;

const CHAPTER: &str = "agent_memory";
const REALM: &str = "conformance-realm";

fn recall_request(identity: &AgentIdentity) -> AgentMemoryRecallRequest {
    AgentMemoryRecallRequest {
        identity: identity.clone(),
        realm: REALM.to_string(),
        query_text: None,
        query_terms: Vec::new(),
        selection: AgentMemorySelection::Always,
        max_entries: 64,
    }
}

fn ensure_unsupported(
    steps: &Steps,
    step: &'static str,
    flag: &str,
    result: Result<(), AgentMemoryError>,
) -> Result<(), ConformanceFailure> {
    match result {
        Err(AgentMemoryError::Unsupported(_)) => Ok(()),
        Err(other) => Err(steps.fail(
            step,
            format!(
                "a provider with {flag}() == false must refuse with the typed Unsupported \
                 error, got: {other}"
            ),
        )),
        Ok(()) => Err(steps.fail(
            step,
            format!(
                "a provider with {flag}() == false must refuse the operation — succeeding while \
                 refusing to advertise the capability makes the flag a lie"
            ),
        )),
    }
}

#[allow(clippy::too_many_lines)]
pub async fn agent_memory(
    factory: &dyn AgentMemoryProviderFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let provider = factory.open().await?;
    let identity = steps.wrap("setup", AgentIdentity::parse("conformance:memory"))?;
    let scope = MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: identity.as_str().to_string(),
    };

    // --- recall is the one unconditionally required operation ------------------
    let step = "recall_baseline";
    let initial = steps.wrap(step, provider.recall(recall_request(&identity)).await)?;
    steps.ensure(
        step,
        initial.is_empty(),
        "recall over an empty provider must return no records",
    )?;

    // --- remember ---------------------------------------------------------------
    let step = "remember_capability";
    let mut remembered_id: Option<String> = None;
    if provider.supports_remember() {
        let record = steps.wrap(
            step,
            provider
                .remember(
                    REALM,
                    &identity,
                    NewAgentMemory {
                        title: "Conformance remembered fact".to_string(),
                        body: "The conformance suite wrote this record.".to_string(),
                        tags: vec!["conformance".to_string()],
                    },
                )
                .await,
        )?;
        let recalled = steps.wrap(step, provider.recall(recall_request(&identity)).await)?;
        steps.ensure(
            step,
            recalled.iter().any(|row| row.memory_id == record.memory_id),
            "a provider advertising supports_remember must serve the write through recall",
        )?;
        remembered_id = Some(record.memory_id);
    } else {
        ensure_unsupported(
            &steps,
            step,
            "supports_remember",
            provider
                .remember(
                    REALM,
                    &identity,
                    NewAgentMemory {
                        title: "refused".to_string(),
                        body: "refused".to_string(),
                        tags: Vec::new(),
                    },
                )
                .await
                .map(|_| ()),
        )?;
    }

    // --- forget -------------------------------------------------------------------
    let step = "forget_capability";
    if provider.supports_forget() {
        if provider.supports_remember() {
            let doomed = steps.wrap(
                step,
                provider
                    .remember(
                        REALM,
                        &identity,
                        NewAgentMemory {
                            title: "Conformance doomed fact".to_string(),
                            body: "This record exists to be forgotten.".to_string(),
                            tags: Vec::new(),
                        },
                    )
                    .await,
            )?;
            let outcome = steps.wrap(
                step,
                provider.forget(REALM, &identity, &doomed.memory_id).await,
            )?;
            steps.ensure(
                step,
                outcome.deleted && outcome.memory_id == doomed.memory_id,
                "forget must report the deletion of an existing record",
            )?;
            let recalled = steps.wrap(step, provider.recall(recall_request(&identity)).await)?;
            steps.ensure(
                step,
                !recalled.iter().any(|row| row.memory_id == doomed.memory_id),
                "a forgotten record must no longer surface through recall",
            )?;
        }
    } else {
        ensure_unsupported(
            &steps,
            step,
            "supports_forget",
            provider
                .forget(REALM, &identity, "conformance-missing-id")
                .await
                .map(|_| ()),
        )?;
    }

    // --- manifest -------------------------------------------------------------------
    let step = "manifest_capability";
    if provider.supports_manifest() {
        let manifest = steps.wrap(
            step,
            provider
                .manifest(std::slice::from_ref(&scope), ManifestTier::Full)
                .await,
        )?;
        if let Some(id) = &remembered_id {
            steps.ensure(
                step,
                manifest.iter().any(|meta| &meta.id == id),
                "a provider advertising supports_manifest must index active records in the \
                 Full manifest",
            )?;
        }
    } else {
        ensure_unsupported(
            &steps,
            step,
            "supports_manifest",
            provider
                .manifest(std::slice::from_ref(&scope), ManifestTier::Full)
                .await
                .map(|_| ()),
        )?;
    }

    // --- supersede -------------------------------------------------------------------
    let step = "supersede_capability";
    if provider.supports_supersede() {
        let prior = match &remembered_id {
            Some(id) => id.clone(),
            None => {
                return Err(steps.fail(
                    step,
                    "chapter limitation: supersede requires a prior record, which needs \
                     supports_remember — no known provider advertises supersede without \
                     remember",
                ));
            }
        };
        let replacement = steps.wrap(
            step,
            provider
                .supersede(
                    &scope,
                    &prior,
                    fixtures::new_memory_record(
                        "Conformance superseding fact",
                        "This record supersedes the remembered fact.",
                    ),
                )
                .await,
        )?;
        steps.ensure(
            step,
            replacement != prior,
            "supersede must mint a new record id within the lineage",
        )?;
        let recalled = steps.wrap(step, provider.recall(recall_request(&identity)).await)?;
        steps.ensure(
            step,
            recalled.iter().any(|row| row.memory_id == replacement),
            "the superseding record must be active and recallable",
        )?;
        steps.ensure(
            step,
            !recalled.iter().any(|row| row.memory_id == prior),
            "the superseded prior must no longer surface as active in recall",
        )?;
    } else {
        ensure_unsupported(
            &steps,
            step,
            "supports_supersede",
            provider
                .supersede(
                    &scope,
                    "conformance-missing-id",
                    fixtures::new_memory_record("refused", "refused"),
                )
                .await
                .map(|_| ()),
        )?;
    }

    // --- propose ---------------------------------------------------------------------
    let step = "propose_capability";
    if provider.supports_propose() {
        let proposal = steps.wrap(
            step,
            provider
                .propose(
                    &scope,
                    fixtures::new_memory_record(
                        "Conformance proposed fact",
                        "This record awaits steward review.",
                    ),
                    MemoryAuthor::Application,
                )
                .await,
        )?;
        steps.ensure(
            step,
            !proposal.is_empty(),
            "propose must return a non-empty proposal id",
        )?;
    } else {
        ensure_unsupported(
            &steps,
            step,
            "supports_propose",
            provider
                .propose(
                    &scope,
                    fixtures::new_memory_record("refused", "refused"),
                    MemoryAuthor::Application,
                )
                .await
                .map(|_| ()),
        )?;
    }

    // --- authored writes ---------------------------------------------------------------
    let step = "authored_writes_capability";
    let author = MemoryAuthor::Agent {
        identity: identity.as_str().to_string(),
    };
    if provider.supports_authored_writes() {
        let receipt = steps.wrap(
            step,
            provider
                .remember_authored(
                    &scope,
                    fixtures::new_memory_record(
                        "Conformance authored fact",
                        "An agent principal wrote this record.",
                    ),
                    author.clone(),
                )
                .await,
        )?;
        steps.ensure(
            step,
            !receipt.memory_id.is_empty(),
            "remember_authored must return a receipt naming the landed record",
        )?;
        let outcome = steps.wrap(
            step,
            provider
                .forget_authored(&scope, &receipt.memory_id, author)
                .await,
        )?;
        steps.ensure(
            step,
            outcome.deleted,
            "forget_authored must delete the author's own record",
        )?;
    } else {
        ensure_unsupported(
            &steps,
            step,
            "supports_authored_writes",
            provider
                .remember_authored(
                    &scope,
                    fixtures::new_memory_record("refused", "refused"),
                    author,
                )
                .await
                .map(|_| ()),
        )?;
    }
    Ok(())
}
