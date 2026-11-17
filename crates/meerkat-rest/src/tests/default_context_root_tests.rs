//! Session creation and the directory the server reads project instructions
//! from, on the REST surface.
//!
//! A server started without `--context-root` looks for project instructions
//! (AGENTS.md) in its own instance directory. That directory is a default,
//! not something the operator named, and a first start finds none: there are
//! then no project instructions, and a session builds as it does for a
//! directory without AGENTS.md. A context root the operator named is a
//! statement: when it does not exist the request is refused, and so is a
//! request whose AGENTS.md exists and cannot be read.
//!
//! Every row keeps the realm state, the user configuration and the
//! credentials under its own temporary directory.

use super::*;
use axum::body::Body;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

/// Bound on one request: it only turns a hang into a failure.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// A server state composed as the binary composes it, with `instance_root`
/// in the place of the per-user instance directory and `context_root` as the
/// operator's `--context-root` (`None`: not passed).
async fn rest_state(
    temp: &TempDir,
    instance_root: PathBuf,
    context_root: Option<PathBuf>,
) -> AppState {
    let mut bootstrap = RuntimeBootstrap::default();
    bootstrap.realm.state_root = Some(temp.path().join("realms"));
    bootstrap.context.context_root = context_root;
    bootstrap.context.user_config_root = Some(temp.path().join("user"));
    let mut state = AppState::load_from_with_bootstrap(
        instance_root,
        bootstrap,
        false,
        test_credential_store(temp.path()),
    )
    .await
    .expect("the server state loads");
    state.llm_client_override = Some(Arc::new(MockLlmClient));
    state
}

/// `POST /sessions` with a prompt and a model, and nothing else.
async fn create_session_with_prompt_and_model(state: AppState) -> (StatusCode, String) {
    let response = tokio::time::timeout(
        DEADLINE,
        router(state).oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"prompt": "Hello", "model": "claude-sonnet-4-6"}).to_string(),
                ))
                .unwrap(),
        ),
    )
    .await
    .expect("the create request answers")
    .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn assert_created(status: StatusCode, body: &str) {
    assert_eq!(status, StatusCode::OK, "the session is created: {body}");
    let payload: Value = serde_json::from_str(body).unwrap();
    assert!(payload["session_id"].is_string(), "{body}");
    assert_eq!(payload["text"], "ok", "{body}");
}

fn assert_refused_for_project_instructions(
    status: StatusCode,
    body: &str,
    named: &std::path::Path,
) {
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(
        body.contains("project AGENTS.md") && body.contains("is not readable"),
        "the refusal names the project instructions: {body}"
    );
    assert!(
        body.contains(&named.display().to_string()),
        "the refusal names {}: {body}",
        named.display()
    );
}

/// No `--context-root`, and the instance directory does not exist (a first
/// start): the session is created, with no project instructions.
#[tokio::test]
async fn create_session_answers_when_the_default_instance_directory_does_not_exist() {
    let temp = TempDir::new().unwrap();
    let instance_root = temp.path().join("data").join("meerkat").join("rest");
    let state = rest_state(&temp, instance_root.clone(), None).await;
    assert!(
        !instance_root.exists(),
        "the row needs a start that leaves the instance directory absent"
    );

    let (status, body) = create_session_with_prompt_and_model(state).await;
    assert_created(status, &body);
}

/// The same request on a server whose operator named a context root that
/// does not exist is refused, and the refusal names that directory.
#[tokio::test]
async fn create_session_refuses_a_named_context_root_that_does_not_exist() {
    let temp = TempDir::new().unwrap();
    let instance_root = temp.path().join("data").join("meerkat").join("rest");
    let named = temp.path().join("no-such-project");
    let state = rest_state(&temp, instance_root, Some(named.clone())).await;

    let (status, body) = create_session_with_prompt_and_model(state).await;
    assert_refused_for_project_instructions(status, &body, &named);
}

/// A directory that exists and holds no AGENTS.md has no project
/// instructions, as the default instance directory and as a named context
/// root alike.
#[tokio::test]
async fn create_session_answers_for_an_existing_directory_without_agents_md() {
    let temp = TempDir::new().unwrap();
    let instance_root = temp.path().join("data").join("meerkat").join("rest");
    std::fs::create_dir_all(&instance_root).unwrap();
    let state = rest_state(&temp, instance_root.clone(), None).await;
    let (status, body) = create_session_with_prompt_and_model(state).await;
    assert_created(status, &body);

    let named_temp = TempDir::new().unwrap();
    let named = named_temp.path().join("project");
    std::fs::create_dir_all(&named).unwrap();
    let state = rest_state(&named_temp, instance_root, Some(named)).await;
    let (status, body) = create_session_with_prompt_and_model(state).await;
    assert_created(status, &body);
}

/// An AGENTS.md that exists in the default instance directory and cannot be
/// read is a fault, not absence: the request is refused.
#[tokio::test]
async fn create_session_refuses_an_unreadable_agents_md_in_the_default_instance_directory() {
    let temp = TempDir::new().unwrap();
    let instance_root = temp.path().join("data").join("meerkat").join("rest");
    std::fs::create_dir_all(&instance_root).unwrap();
    let agents_md = instance_root.join("AGENTS.md");
    // Not UTF-8: the file is there and its content cannot be read as text.
    std::fs::write(&agents_md, [0xff, 0xfe, 0xfd]).unwrap();
    let state = rest_state(&temp, instance_root, None).await;

    let (status, body) = create_session_with_prompt_and_model(state).await;
    assert_refused_for_project_instructions(status, &body, &agents_md);
}
