//! Which callers the REST server answers, by the `Origin` a request carries.
//!
//! A browser page sends its origin with every cross-origin request and with
//! every request that is not a plain read. The server answers a page only
//! when the realm's `[rest] allowed_origins` names its origin; the answer
//! then carries a cross-origin allowance for that one origin. A request from
//! any other origin is refused before a route runs, its preflight included.
//!
//! A program that is not a browser sends no `Origin` header. It is served as
//! before and never meets the refusal.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

use crate::ErrorResponse;

/// The `code` of the refusal of a request whose origin is not allowed.
pub const ORIGIN_NOT_ALLOWED: &str = "ORIGIN_NOT_ALLOWED";

/// The origins of the browser pages this server answers: the realm's
/// `[rest] allowed_origins`, read at startup. Empty by default.
#[derive(Debug, Clone, Default)]
pub struct AllowedOrigins(Arc<[String]>);

impl AllowedOrigins {
    /// The origins a realm's config lists. `Config::validate` has refused
    /// every entry that is not one origin.
    pub fn new(origins: &[String]) -> Self {
        Self(origins.iter().cloned().collect())
    }

    /// The listed origins, as the config holds them.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Whether the realm lists `origin`. A browser serializes an origin's
    /// scheme and host in lower case; the comparison ignores ASCII case so
    /// an entry written otherwise still names it.
    fn allows(&self, origin: &str) -> bool {
        self.0
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    }

    /// Why a request with these headers is refused, when it is.
    fn refusal(&self, headers: &HeaderMap) -> Option<&'static str> {
        let mut origins = headers.get_all(header::ORIGIN).iter();
        match (origins.next(), origins.next()) {
            // No origin: not a page's request, unless the browser itself
            // marks it as coming from another site (it sends no `Origin` on
            // some reads). Such a request names no origin a realm could
            // have listed.
            (None, _) => headers
                .get("sec-fetch-site")
                .and_then(|site| site.to_str().ok())
                .is_some_and(|site| site.trim().eq_ignore_ascii_case("cross-site"))
                .then_some(
                    "this server does not answer a request a browser sends from another site \
                     without naming its origin",
                ),
            (Some(origin), None) => {
                let allowed = origin.to_str().is_ok_and(|origin| self.allows(origin));
                (!allowed).then_some(
                    "this origin is not allowed to call this server; an operator allows an \
                     origin by listing it in the realm's `[rest] allowed_origins`",
                )
            }
            (Some(_), Some(_)) => Some("a request names at most one origin"),
        }
    }

    /// The cross-origin answer for the listed origins: an answer to a listed
    /// origin names that origin and no other. `None` when nothing is listed:
    /// the server then gives no cross-origin answer at all.
    fn cross_origin_answer(&self) -> Option<CorsLayer> {
        if self.0.is_empty() {
            return None;
        }
        let allowed = self.clone();
        Some(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
                    origin.to_str().is_ok_and(|origin| allowed.allows(origin))
                }))
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::PATCH,
                    Method::DELETE,
                ])
                .allow_headers(AllowHeaders::mirror_request()),
        )
    }
}

/// Refuse a request whose origin the realm does not allow, before any route
/// runs: `403` in the server's error envelope, with no cross-origin
/// allowance.
async fn refuse_origin_not_allowed(
    State(allowed): State<AllowedOrigins>,
    request: Request,
    next: Next,
) -> Response {
    match allowed.refusal(request.headers()) {
        Some(reason) => (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: reason.to_string(),
                code: ORIGIN_NOT_ALLOWED.to_string(),
                details: None,
            }),
        )
            .into_response(),
        None => next.run(request).await,
    }
}

/// `router`, answering callers by the origin they name. The check is a layer
/// over the finished router, so every route passes it, and so do the answers
/// to a path or a method the router does not know (a preflight is one). The
/// cross-origin answer sits inside the check: no request reaches it before
/// the check has passed its origin.
pub(crate) fn answering_allowed_origins_only(router: Router, allowed: AllowedOrigins) -> Router {
    let router = match allowed.cross_origin_answer() {
        Some(cross_origin_answer) => router.layer(cross_origin_answer),
        None => router,
    };
    router.layer(axum::middleware::from_fn_with_state(
        allowed,
        refuse_origin_not_allowed,
    ))
}
