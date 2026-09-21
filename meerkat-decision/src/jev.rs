//! Jev (TypeSafe) evaluation backend.
//!
//! This adapter owns the `POST /v1/systemone` transport, protocol encoding
//! and decoding, native probability/confidence signals, and the typed status
//! vocabulary of that endpoint. It owns no application policy: no threshold
//! is applied to a `noul` probability, no level is elected from a `score`,
//! and no credential is read from the environment or persisted. The bearer
//! secret is borrowed per call from an owner-issued [`JevCredentialSource`].

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_core::JevBackendConfig;
use meerkat_core::time_compat::Duration;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::{
    BackendResponse, BackendUsage, Deadline, DecisionBackend, RawAnswer, RawDistribution,
    RawGradeDistribution,
};
use crate::contracts::{BackendKind, Question, RouteProvenance};
use crate::error::BackendFailure;
use crate::validate::ValidatedRequest;

/// A bearer secret whose bytes never appear in `Debug` output or logs.
#[derive(Clone)]
pub struct JevBearerSecret(String);

impl JevBearerSecret {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for JevBearerSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("JevBearerSecret([REDACTED])")
    }
}

/// Why a credential could not be produced for this call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JevCredentialError {
    #[error("credential source has no secret: {0}")]
    Missing(String),
    #[error("credential source failed: {0}")]
    ResolutionFailed(String),
}

/// Owner-issued source of the Jev bearer credential.
///
/// The host composes this from its realm credential authority. Resolving per
/// call (rather than caching in the adapter) keeps revocation, rotation, and
/// refresh with their existing owner.
#[async_trait]
pub trait JevCredentialSource: Send + Sync {
    async fn bearer_secret(&self) -> Result<JevBearerSecret, JevCredentialError>;
}

/// Static credential source for hosts that already hold the secret in memory.
pub struct StaticJevCredential(JevBearerSecret);

impl StaticJevCredential {
    pub fn new(secret: JevBearerSecret) -> Arc<Self> {
        Arc::new(Self(secret))
    }
}

#[async_trait]
impl JevCredentialSource for StaticJevCredential {
    async fn bearer_secret(&self) -> Result<JevBearerSecret, JevCredentialError> {
        Ok(self.0.clone())
    }
}

/// Why the adapter could not be constructed from its config.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JevBackendBuildError {
    #[error("invalid Jev endpoint `{endpoint}`: {reason}")]
    InvalidEndpoint { endpoint: String, reason: String },
    #[error("HTTP client construction failed: {0}")]
    HttpClient(String),
}

/// Decision backend over the Jev evaluation endpoint.
pub struct JevBackend {
    http: reqwest::Client,
    endpoint: reqwest::Url,
    model: String,
    credential: Arc<dyn JevCredentialSource>,
}

impl JevBackend {
    pub fn new(
        config: &JevBackendConfig,
        credential: Arc<dyn JevCredentialSource>,
    ) -> Result<Self, JevBackendBuildError> {
        let endpoint = reqwest::Url::parse(&config.endpoint).map_err(|error| {
            JevBackendBuildError::InvalidEndpoint {
                endpoint: config.endpoint.clone(),
                reason: error.to_string(),
            }
        })?;
        let http = reqwest::Client::builder()
            .build()
            .map_err(|error| JevBackendBuildError::HttpClient(error.to_string()))?;
        Ok(Self {
            http,
            endpoint,
            model: config.model.clone(),
            credential,
        })
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

// ---- wire types (private; the endpoint owns this shape) ----

#[derive(Debug, Serialize)]
struct WireRequest<'a> {
    state: Value,
    model: &'a str,
    questions: BTreeMap<&'a str, WireQuestion>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireQuestion {
    Noul {
        instructions: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<WireNoulCriteria>,
    },
    Choice {
        instructions: Value,
        criteria: IndexMap<String, Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

#[derive(Debug, Serialize)]
struct WireNoulCriteria {
    #[serde(rename = "true")]
    yes: Value,
    #[serde(rename = "false")]
    no: Value,
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    model: String,
    answers: BTreeMap<String, WireAnswer>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

/// Build the wire request. The optional task is folded into a structured
/// state envelope so the endpoint sees it as data beside the state.
pub(crate) fn encode_request<'a>(request: &'a ValidatedRequest, model: &'a str) -> Value {
    let inner = request.request();
    let state = match inner.task.as_ref() {
        Some(task) => json!({ "task": task, "state": inner.state.as_value() }),
        None => inner.state.as_value().clone(),
    };
    let questions = request
        .questions()
        .iter()
        .map(|question| {
            let wire = match question {
                Question::Binary {
                    instructions,
                    criteria,
                    ..
                } => WireQuestion::Noul {
                    instructions: instructions.to_value(),
                    criteria: criteria.as_ref().map(|criteria| WireNoulCriteria {
                        yes: criteria.yes.to_value(),
                        no: criteria.no.to_value(),
                    }),
                },
                Question::ChooseOne {
                    instructions,
                    options,
                    ..
                } => WireQuestion::Choice {
                    instructions: instructions.to_value(),
                    criteria: options
                        .iter()
                        .map(|option| (option.id.to_string(), option.description.to_value()))
                        .collect(),
                },
                Question::Grade {
                    instructions,
                    levels,
                    ..
                } => WireQuestion::Score {
                    instructions: instructions.to_value(),
                    criteria: levels
                        .iter()
                        .map(|level| level.description.to_value())
                        .collect(),
                },
            };
            (question.id().as_str(), wire)
        })
        .collect();
    let wire = WireRequest {
        state,
        model,
        questions,
    };
    serde_json::to_value(wire).unwrap_or(Value::Null)
}

fn decode_answer(answer: WireAnswer) -> Result<RawAnswer, BackendFailure> {
    Ok(match answer {
        WireAnswer::Noul { noul } => RawAnswer::BinaryProbability { yes: noul },
        WireAnswer::Choice {
            choice,
            probabilities,
            confidence,
        } => RawAnswer::ChoiceSelected {
            option: choice,
            distribution: Some(RawDistribution {
                probabilities: probabilities.into_iter().collect(),
                confidence,
            }),
        },
        WireAnswer::Score {
            score,
            probabilities,
            confidence,
        } => {
            let probabilities = probabilities
                .into_iter()
                .map(|(level, probability)| {
                    level
                        .parse::<u32>()
                        .map(|level| (level, probability))
                        .map_err(|_| BackendFailure::InvalidResponse {
                            message: format!("score level key `{level}` is not an index"),
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            RawAnswer::GradeWeighted {
                position: score,
                distribution: Some(RawGradeDistribution {
                    probabilities,
                    confidence,
                }),
            }
        }
    })
}

fn backoff_for(attempt: u32) -> Duration {
    Duration::from_millis(250u64.saturating_mul(1u64 << attempt.min(6)))
}

#[async_trait]
impl DecisionBackend for JevBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Jev
    }

    async fn evaluate(
        &self,
        request: &ValidatedRequest,
        deadline: Deadline,
        max_attempts: u32,
    ) -> Result<BackendResponse, BackendFailure> {
        let body = encode_request(request, &self.model);
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            if deadline.is_expired() {
                return Err(BackendFailure::Timeout);
            }
            let secret = self.credential.bearer_secret().await.map_err(|error| {
                BackendFailure::CredentialUnavailable {
                    message: error.to_string(),
                }
            })?;
            let response = self
                .http
                .post(self.endpoint.clone())
                .bearer_auth(secret.expose())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(deadline.remaining())
                .json(&body)
                .send()
                .await
                .map_err(|error| {
                    if error.is_timeout() {
                        BackendFailure::Timeout
                    } else {
                        BackendFailure::Transport {
                            message: error.without_url().to_string(),
                        }
                    }
                })?;
            let status = response.status();
            let text = response
                .text()
                .await
                .map_err(|error| BackendFailure::Transport {
                    message: error.without_url().to_string(),
                })?;
            let failure = match status.as_u16() {
                200 => {
                    let decoded: WireResponse = serde_json::from_str(&text).map_err(|error| {
                        BackendFailure::InvalidResponse {
                            message: error.to_string(),
                        }
                    })?;
                    let answers = decoded
                        .answers
                        .into_iter()
                        .map(|(id, answer)| decode_answer(answer).map(|raw| (id, raw)))
                        .collect::<Result<Vec<_>, _>>()?;
                    let usage = match decoded.usage {
                        Some(usage) => BackendUsage::Reported {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                        },
                        None => BackendUsage::Unmeasured,
                    };
                    return Ok(BackendResponse {
                        answers,
                        route: RouteProvenance::Jev {
                            endpoint: self.endpoint.to_string(),
                            requested_model: self.model.clone(),
                            served_model: decoded.model,
                        },
                        usage,
                        attempts,
                    });
                }
                401 => BackendFailure::Unauthorized,
                422 => BackendFailure::InvalidRequestRejected { message: text },
                429 => BackendFailure::RateLimited,
                529 => BackendFailure::Overloaded,
                code => BackendFailure::ServiceError {
                    status: code,
                    message: text,
                },
            };
            if !failure.is_transient() || attempts >= max_attempts {
                return Err(failure);
            }
            let wait = backoff_for(attempts).min(deadline.remaining());
            if wait.is_zero() {
                return Err(BackendFailure::Timeout);
            }
            tracing::debug!(
                attempt = attempts,
                ?failure,
                wait_ms = wait.as_millis() as u64,
                "Jev backend transient failure; backing off within the deadline"
            );
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;

    use super::*;
    use crate::validate::tests::validated;

    #[derive(Clone)]
    struct MockState {
        calls: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<(HeaderMap, Value)>>>,
        script: Arc<Mutex<Vec<(u16, String)>>>,
    }

    async fn serve(script: Vec<(u16, String)>) -> (SocketAddr, MockState) {
        let state = MockState {
            calls: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(Mutex::new(Vec::new())),
            script: Arc::new(Mutex::new(script)),
        };
        let app = Router::new()
            .route(
                "/v1/systemone",
                post(
                    |State(state): State<MockState>, headers: HeaderMap, body: String| async move {
                        state.calls.fetch_add(1, Ordering::SeqCst);
                        let value: Value = serde_json::from_str(&body).unwrap();
                        state.seen.lock().unwrap().push((headers, value));
                        let (status, body) = state.script.lock().unwrap().remove(0);
                        (StatusCode::from_u16(status).unwrap(), body)
                    },
                ),
            )
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state)
    }

    fn backend_for(addr: SocketAddr) -> JevBackend {
        let config = JevBackendConfig {
            endpoint: format!("http://{addr}/v1/systemone"),
            model: "jev-latest".into(),
            ..JevBackendConfig::default()
        };
        JevBackend::new(
            &config,
            StaticJevCredential::new(JevBearerSecret::new("test-secret")),
        )
        .unwrap()
    }

    const SUCCESS: &str = r#"{
        "model": "jev-1.13.0",
        "answers": {
            "is_urgent": {"type": "noul", "noul": 0.95},
            "department": {"type": "choice", "choice": "billing", "probabilities": {"billing": 0.88, "technical": 0.12}, "confidence": 0.81},
            "frustration": {"type": "score", "score": 1.05, "legend": {"0": "Calm", "1": "Frustrated", "2": "Very angry"}, "probabilities": {"0": 0.0, "1": 0.95, "2": 0.05}, "confidence": 0.92}
        },
        "usage": {"input_tokens": 296, "output_tokens": 20}
    }"#;

    #[tokio::test]
    async fn encodes_the_documented_wire_shape_and_decodes_native_signals() {
        let (addr, state) = serve(vec![(200, SUCCESS.to_string())]).await;
        let backend = backend_for(addr);
        let request = validated();

        let response = backend
            .evaluate(&request, Deadline::after(Duration::from_secs(5)), 2)
            .await
            .unwrap();

        assert_eq!(response.attempts, 1);
        assert_eq!(
            response.route,
            RouteProvenance::Jev {
                endpoint: format!("http://{addr}/v1/systemone"),
                requested_model: "jev-latest".into(),
                served_model: "jev-1.13.0".into(),
            }
        );
        assert_eq!(
            response.usage,
            BackendUsage::Reported {
                input_tokens: 296,
                output_tokens: 20
            }
        );
        let answers: BTreeMap<_, _> = response.answers.into_iter().collect();
        assert!(matches!(
            answers["is_urgent"],
            RawAnswer::BinaryProbability { yes } if (yes - 0.95).abs() < 1e-9
        ));
        assert!(matches!(
            &answers["frustration"],
            RawAnswer::GradeWeighted { position, distribution: Some(dist) }
                if (position - 1.05).abs() < 1e-9 && dist.probabilities.len() == 3
        ));

        let seen = state.seen.lock().unwrap();
        let (headers, body) = &seen[0];
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer test-secret"
        );
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"]["task"], "Triage a support message");
        assert_eq!(body["questions"]["is_urgent"]["type"], "noul");
        assert_eq!(body["questions"]["department"]["type"], "choice");
        assert_eq!(
            body["questions"]["department"]["criteria"]["billing"],
            "Payments, invoicing, refunds"
        );
        assert_eq!(body["questions"]["frustration"]["type"], "score");
        assert_eq!(
            body["questions"]["frustration"]["criteria"],
            json!(["Calm", "Frustrated", "Very angry"])
        );
    }

    #[tokio::test]
    async fn unauthorized_is_typed_and_never_retried() {
        let (addr, state) = serve(vec![(401, "{}".into())]).await;
        let backend = backend_for(addr);
        let failure = backend
            .evaluate(&validated(), Deadline::after(Duration::from_secs(5)), 3)
            .await
            .unwrap_err();
        assert_eq!(failure, BackendFailure::Unauthorized);
        assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn validation_rejection_carries_the_endpoint_body() {
        let (addr, _) = serve(vec![(422, r#"{"error":"bad question"}"#.into())]).await;
        let backend = backend_for(addr);
        let failure = backend
            .evaluate(&validated(), Deadline::after(Duration::from_secs(5)), 3)
            .await
            .unwrap_err();
        assert!(matches!(
            failure,
            BackendFailure::InvalidRequestRejected { ref message } if message.contains("bad question")
        ));
    }

    #[tokio::test]
    async fn transient_statuses_back_off_within_attempts_then_succeed() {
        let (addr, state) = serve(vec![
            (429, String::new()),
            (529, String::new()),
            (200, SUCCESS.to_string()),
        ])
        .await;
        let backend = backend_for(addr);
        let response = backend
            .evaluate(&validated(), Deadline::after(Duration::from_secs(10)), 3)
            .await
            .unwrap();
        assert_eq!(response.attempts, 3);
        assert_eq!(state.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn transient_statuses_fail_typed_when_attempts_are_exhausted() {
        let (addr, _) = serve(vec![(429, String::new()), (429, String::new())]).await;
        let backend = backend_for(addr);
        let failure = backend
            .evaluate(&validated(), Deadline::after(Duration::from_secs(10)), 2)
            .await
            .unwrap_err();
        assert_eq!(failure, BackendFailure::RateLimited);
    }

    #[tokio::test]
    async fn missing_credential_fails_before_any_request() {
        struct NoSecret;
        #[async_trait]
        impl JevCredentialSource for NoSecret {
            async fn bearer_secret(&self) -> Result<JevBearerSecret, JevCredentialError> {
                Err(JevCredentialError::Missing("JEV_API_KEY unset".into()))
            }
        }
        let (addr, state) = serve(vec![(200, SUCCESS.to_string())]).await;
        let config = JevBackendConfig {
            endpoint: format!("http://{addr}/v1/systemone"),
            ..JevBackendConfig::default()
        };
        let backend = JevBackend::new(&config, Arc::new(NoSecret)).unwrap();
        let failure = backend
            .evaluate(&validated(), Deadline::after(Duration::from_secs(5)), 1)
            .await
            .unwrap_err();
        assert!(matches!(
            failure,
            BackendFailure::CredentialUnavailable { .. }
        ));
        assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let secret = JevBearerSecret::new("very-secret");
        assert!(!format!("{secret:?}").contains("very-secret"));
    }
}
