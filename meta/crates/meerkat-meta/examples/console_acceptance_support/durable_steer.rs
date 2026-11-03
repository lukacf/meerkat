//! Fixture-only ingress into the public typed durable Steer API.
//! The runtime owns admission, boundary delivery, history, and completion.

use std::sync::Arc;

use axum::{Json, Router, extract::Query, http::StatusCode, routing::post};
use meerkat_core::lifecycle::{
    ConversationAppend, ConversationAppendRole, CoreRenderable, InputId,
};
use meerkat_core::{HandlingMode, SystemNoticeKind, types::SessionId};
use meerkat_meta::UnifiedRuntime;
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoticeRequest {
    identity: String,
    session_id: SessionId,
    content: String,
    #[serde(default)]
    background_job: Option<BackgroundJobRequest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BackgroundJobRequest {
    job_id: String,
    display_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputQuery {
    session_id: SessionId,
    input_id: InputId,
}

fn response(result: Result<Value, String>) -> (StatusCode, Json<Value>) {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "error": error }))),
    }
}

fn machine(runtime: &UnifiedRuntime) -> Result<Arc<meerkat_runtime::MeerkatMachine>, String> {
    runtime
        .mob_runtime()
        .session_service()
        .ok_or("fixture needs the actual session runtime adapter")?
        .acquire_runtime_adapter(None)
        .map_err(|error| format!("acquire the fixture session runtime adapter: {error}"))?
        .ok_or_else(|| "fixture needs the actual session runtime adapter".into())
}

fn notice_prompt(request: &NoticeRequest) -> Result<meerkat_runtime::PromptInput, String> {
    if request.content.trim().is_empty() {
        return Err("a nonempty notice is required".into());
    }
    if let Some(job) = &request.background_job {
        if job.job_id.trim().is_empty() || job.display_name.trim().is_empty() {
            return Err("a background job needs a nonempty id and display name".into());
        }
        let notice = meerkat_core::SystemNoticeMessage::persisted_background_job(
            &job.display_name,
            &job.job_id,
            meerkat_core::event::BackgroundJobTerminalStatus::Completed,
            request.content.clone(),
        );
        return Ok(meerkat_runtime::PromptInput::detached_job_completed(
            format!("console-acceptance:background-job:{}", job.job_id),
            notice,
        ));
    }
    let mut prompt = meerkat_runtime::PromptInput::new(
        "",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(HandlingMode::Steer),
                ..Default::default()
            },
        ),
    );
    prompt.typed_turn_appends = vec![ConversationAppend {
        runtime_source: None,
        role: ConversationAppendRole::SystemNotice,
        content: CoreRenderable::SystemNotice {
            kind: SystemNoticeKind::Generic,
            body: Some(request.content.clone()),
            blocks: Vec::new(),
        },
        identity: None,
    }];
    Ok(prompt)
}

async fn submit(runtime: &UnifiedRuntime, request: NoticeRequest) -> Result<Value, String> {
    if !["router:main", "domain:delivery"].contains(&request.identity.as_str()) {
        return Err("an existing fixture identity is required".into());
    }
    let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(&request.identity);
    let actual_session = runtime
        .mob_handle()
        .resolve_bridge_session_id(&member)
        .await
        .ok_or("fixture member has no current session")?;
    if actual_session != request.session_id {
        return Err("the requested session is not the current fixture member session".into());
    }
    let input = meerkat_runtime::Input::Prompt(notice_prompt(&request)?);
    let input_id = input.id().clone();
    let (outcome, _completion) = machine(runtime)?
        .accept_input_with_completion(&actual_session, input)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "accepted": outcome.is_accepted(), "input_id": input_id,
        "identity": request.identity, "session_id": actual_session,
    }))
}

async fn inspect(runtime: &UnifiedRuntime, query: InputQuery) -> Result<Value, String> {
    let machine = machine(runtime)?;
    let state = machine
        .input_state(&query.session_id, &query.input_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("runtime input is absent")?;
    let completion = machine
        .input_terminal_completion(&query.session_id, &query.input_id)
        .await
        .map_err(|error| error.to_string())?;
    let snapshot = machine
        .meerkat_machine_archive_snapshot(&query.session_id)
        .await
        .ok_or("runtime snapshot is absent")?;
    Ok(json!({
        "input_id": query.input_id, "session_id": query.session_id,
        "phase": state.seed.phase, "run_id": state.seed.last_run_id,
        "terminal_outcome": state.seed.terminal_outcome, "completion": completion,
        "runtime_phase": snapshot.control.phase,
        "current_run_id": snapshot.control.current_run_id,
        "queue": snapshot.queue, "steer_queue": snapshot.steer_queue,
    }))
}

pub fn router(runtime: Arc<UnifiedRuntime>) -> Router {
    let reader = runtime.clone();
    Router::new().route(
        "/durable-steer",
        post(move |Json(request): Json<NoticeRequest>| {
            let runtime = runtime.clone();
            async move { response(submit(&runtime, request).await) }
        })
        .get(move |Query(query): Query<InputQuery>| {
            let runtime = reader.clone();
            async move { response(inspect(&runtime, query).await) }
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(background_job: Option<Value>) -> NoticeRequest {
        let mut value = json!({
            "identity": "router:main", "session_id": SessionId::new(),
            "content": "Exact result A\u{030a}, \u{00e5} and \u{1f680}.\nSecond paragraph.",
        });
        if let Some(job) = background_job {
            value["background_job"] = job;
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn durable_steer_generic_control_retains_exact_body_without_job_blocks() {
        let request = request(None);
        let prompt = notice_prompt(&request).unwrap();
        assert_eq!(prompt.typed_turn_appends.len(), 1);
        let append = &prompt.typed_turn_appends[0];
        assert_eq!(append.role, ConversationAppendRole::SystemNotice);
        assert!(append.runtime_source.is_none());
        assert!(append.identity.is_none());
        assert!(matches!(
            &append.content,
            CoreRenderable::SystemNotice { kind: SystemNoticeKind::Generic, body: Some(body), blocks }
                if body == &request.content && blocks.is_empty()
        ));
    }

    #[test]
    fn durable_steer_persisted_job_uses_typed_constructor_without_fabricated_origin() {
        let request = request(Some(
            json!({ "job_id": "job-7", "display_name": "release review" }),
        ));
        let prompt = notice_prompt(&request).unwrap();
        assert_eq!(prompt.typed_turn_appends.len(), 1);
        let append = &prompt.typed_turn_appends[0];
        assert_eq!(append.role, ConversationAppendRole::SystemNotice);
        assert!(append.runtime_source.is_none());
        assert!(append.identity.is_none());
        let CoreRenderable::SystemNotice { kind, body, blocks } = &append.content else {
            panic!("the job completion must be a typed notice");
        };
        assert_eq!(*kind, SystemNoticeKind::BackgroundJob);
        assert_eq!(
            body.as_deref(),
            Some("Background release review job job-7 finished (completed):")
        );
        assert_eq!(
            serde_json::to_value(blocks).unwrap(),
            json!([{
                "type": "background_job", "job_id": "job-7", "display_name": "release review",
                "status": "completed", "detail": request.content, "persisted": true,
            }])
        );
        let wire = serde_json::to_value(&prompt).unwrap();
        assert_eq!(wire["header"]["source"]["type"], "system");
        assert_eq!(wire["header"]["durability"], "durable");
        assert_eq!(
            wire["header"]["idempotency_key"],
            "console-acceptance:background-job:job-7"
        );
        assert_eq!(
            prompt.turn_metadata.unwrap().handling_mode,
            Some(HandlingMode::Steer)
        );
    }

    #[test]
    fn durable_steer_rejects_empty_job_identity_and_empty_result() {
        for job in [
            json!({ "job_id": " ", "display_name": "review" }),
            json!({ "job_id": "job-7", "display_name": " " }),
        ] {
            assert!(notice_prompt(&request(Some(job))).is_err());
        }
        let mut request = request(None);
        request.content = " \n ".into();
        assert!(notice_prompt(&request).is_err());
    }

    #[test]
    fn durable_steer_request_cannot_supply_runtime_provenance() {
        let mut value = json!({
            "identity": "router:main", "session_id": SessionId::new(), "content": "result",
            "runtime_origin": { "run_id": "caller-authored" },
        });
        assert!(serde_json::from_value::<NoticeRequest>(value.clone()).is_err());
        value.as_object_mut().unwrap().remove("runtime_origin");
        value["background_job"] = json!({
            "job_id": "job-7", "display_name": "review", "persisted": false,
        });
        assert!(serde_json::from_value::<NoticeRequest>(value).is_err());
    }
}
