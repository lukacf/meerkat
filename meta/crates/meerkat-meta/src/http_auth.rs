//! HTTP middleware for Bearer token authentication using JWT/JWKS.

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::JwksCache;

/// Axum middleware that validates Bearer tokens using a [`JwksCache`].
///
/// On success the [`crate::auth::ValidatedJwt`] is inserted into request extensions so
/// downstream handlers can extract it via `Extension<ValidatedJwt>`.
///
/// On failure the middleware short-circuits with `401 Unauthorized`.
pub async fn auth_middleware(
    axum::extract::State(cache): axum::extract::State<JwksCache>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_bearer_token(&request).ok_or(StatusCode::UNAUTHORIZED)?;

    let validated_jwt = cache
        .validate_token(token)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    request.extensions_mut().insert(validated_jwt);
    Ok(next.run(request).await)
}

/// Wrap a router with Bearer-token authentication backed by a JWKS cache.
pub fn with_auth_layer(router: Router, jwks_cache: JwksCache) -> Router {
    router.layer(axum::middleware::from_fn_with_state(
        jwks_cache,
        auth_middleware,
    ))
}

fn extract_bearer_token(request: &Request) -> Option<&str> {
    let header_value = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let token = header_value.strip_prefix("Bearer ")?;
    if token.is_empty() {
        return None;
    }
    Some(token)
}
