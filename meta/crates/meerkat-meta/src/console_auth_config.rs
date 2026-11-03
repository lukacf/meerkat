//! Shared explicit console authentication configuration for both gateways
//! and library embedders: `provider: "jwt"` (HS256 shared secret) or
//! `provider: "oidc"` (an external issuer's RS256/ES256 JWKS, for example
//! Google).

use base64::Engine as _;
use serde_json::{Value, json};

use crate::{AuthPolicy, AuthProvider, BigQueryNaming, ConsolePolicy, RuntimeDecisionState};

pub fn parse_console_auth_config(value: &Value) -> Result<RuntimeDecisionState, String> {
    let object = value
        .as_object()
        .ok_or("auth_config must be a JSON object")?;
    let provider = object
        .get("provider")
        .and_then(Value::as_str)
        .or_else(|| {
            (object.contains_key("shared_secret") || object.contains_key("sharedSecret"))
                .then_some("jwt")
        })
        .ok_or("auth_config.provider is required")?;
    if provider == "oidc" {
        return parse_oidc_auth_config(object);
    }
    if provider != "jwt" {
        return Err(format!("unsupported auth_config.provider '{provider}'"));
    }
    let secret = object
        .get("shared_secret")
        .or_else(|| object.get("sharedSecret"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("auth_config.shared_secret must be a non-empty string")?;
    let issuer = object
        .get("issuer")
        .and_then(Value::as_str)
        .unwrap_or("http://127.0.0.1/meta-gateway");
    let audience = object
        .get("audience")
        .and_then(Value::as_str)
        .unwrap_or("persistent-gateway");
    let allowlist = object
        .get("email_allowlist")
        .or_else(|| object.get("emailAllowlist"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut decisions = RuntimeDecisionState::local_console(
        ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        Some(BigQueryNaming {
            dataset: "default_dataset".to_string(),
            table: "default_table".to_string(),
        }),
    );
    decisions.auth = AuthPolicy {
        default_provider: AuthProvider::GenericOidc,
        email_allowlist: allowlist,
    };
    decisions.trusted_oidc = crate::TrustedOidcRuntimeConfig {
        discovery_json:
            json!({"issuer":issuer,"jwks_uri":"http://127.0.0.1/meta-gateway/jwks.json"})
                .to_string(),
        jwks_json: json!({"keys":[{"kty":"oct","alg":"HS256",
            "k":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret.as_bytes())}]})
        .to_string(),
        audience: audience.to_string(),
        require_verified_email: false,
    };
    Ok(decisions)
}

/// `provider: "oidc"`: trust an external issuer's published keys.
///
/// Required: `issuer` (exact `iss` value) and `audience` (the OAuth client
/// id your front presents). Exactly one of `jwks_json` (the JWKS document,
/// inline) or `jwks_uri` (fetched once here, with a 10 s bound, failing
/// closed on any error). The snapshot is fixed for the process lifetime:
/// key rotation at the issuer needs a restart or a refreshable cache on the
/// embedder's own front (see docs/guides/authentication.mdx).
fn parse_oidc_auth_config(
    object: &serde_json::Map<String, Value>,
) -> Result<RuntimeDecisionState, String> {
    let issuer = object
        .get("issuer")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("auth_config.issuer must be a non-empty string for provider \"oidc\"")?;
    let audience = object
        .get("audience")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("auth_config.audience must be a non-empty string for provider \"oidc\"")?;
    let (jwks_json, jwks_uri) = match (object.get("jwks_json"), object.get("jwks_uri")) {
        (Some(_), Some(_)) => {
            return Err("auth_config accepts exactly one of jwks_json or jwks_uri".to_string());
        }
        (Some(inline), None) => {
            let document = match inline {
                Value::String(text) => serde_json::from_str::<Value>(text)
                    .map_err(|error| format!("auth_config.jwks_json is not valid JSON: {error}"))?,
                other => other.clone(),
            };
            validate_jwks_document(&document)?;
            (document.to_string(), OIDC_INLINE_JWKS_MARKER.to_string())
        }
        (None, Some(uri)) => {
            let uri = uri
                .as_str()
                .map(str::trim)
                .filter(|value| value.starts_with("https://"))
                .ok_or("auth_config.jwks_uri must be an https URL")?;
            let document = fetch_jwks_snapshot(uri)?;
            validate_jwks_document(&document)?;
            (document.to_string(), uri.to_string())
        }
        (None, None) => {
            return Err(
                "auth_config requires jwks_json or jwks_uri for provider \"oidc\"".to_string(),
            );
        }
    };
    let allowlist: Vec<String> = object
        .get("email_allowlist")
        .or_else(|| object.get("emailAllowlist"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    if allowlist.is_empty() {
        // Console access is allowlist-gated (`decisions::enforce_console_route_access`
        // refuses every email outside it). Trusting a public issuer without an
        // allowlist would configure a console that admits nobody while reading
        // as if it admitted everyone with an account at that issuer.
        return Err(
            "auth_config.email_allowlist must name at least one email for provider \"oidc\""
                .to_string(),
        );
    }
    let mut decisions = RuntimeDecisionState::local_console(
        ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        Some(BigQueryNaming {
            dataset: "default_dataset".to_string(),
            table: "default_table".to_string(),
        }),
    );
    decisions.auth = AuthPolicy {
        default_provider: AuthProvider::GenericOidc,
        email_allowlist: allowlist,
    };
    // The discovery document records the real key source: the fetched
    // `jwks_uri`, or an `inline` marker for `jwks_json`. Console ingress keys
    // its HS256 development gate off the issuer and JWKS hosts, so this must
    // describe the truth: a public issuer's host must never read as a
    // development host, and provider "oidc" refuses symmetric keys anyway.
    decisions.trusted_oidc = crate::TrustedOidcRuntimeConfig {
        discovery_json: json!({"issuer": issuer, "jwks_uri": jwks_uri}).to_string(),
        jwks_json,
        audience: audience.to_string(),
        require_verified_email: true,
    };
    Ok(decisions)
}

/// `jwks_uri` value recorded in the discovery document when the JWKS was
/// supplied inline; there is no URL to fetch and none is invented.
pub const OIDC_INLINE_JWKS_MARKER: &str = "inline";

fn validate_jwks_document(document: &Value) -> Result<(), String> {
    let keys = document
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("auth_config JWKS must be an object with a keys array")?;
    if keys.is_empty() {
        return Err(
            "auth_config JWKS carries no keys; the console would refuse every request".to_string(),
        );
    }
    for key in keys {
        let kty = key.get("kty").and_then(Value::as_str).unwrap_or_default();
        match kty {
            "RSA" | "EC" => {}
            "oct" => {
                return Err("auth_config provider \"oidc\" does not accept symmetric (oct) keys; use provider \"jwt\" for a shared secret".to_string());
            }
            other => {
                return Err(format!(
                    "auth_config JWKS key type '{other}' is not supported"
                ));
            }
        }
    }
    Ok(())
}

/// One bounded synchronous fetch at configuration time. Runs on a scoped
/// thread so it is safe from inside an async runtime.
fn fetch_jwks_snapshot(uri: &str) -> Result<Value, String> {
    let uri = uri.to_string();
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .map_err(|error| format!("auth_config.jwks_uri client failed: {error}"))?;
                let response = client
                    .get(&uri)
                    .send()
                    .map_err(|error| format!("auth_config.jwks_uri fetch failed: {error}"))?
                    .error_for_status()
                    .map_err(|error| format!("auth_config.jwks_uri fetch failed: {error}"))?;
                response
                    .json::<Value>()
                    .map_err(|error| format!("auth_config.jwks_uri returned invalid JSON: {error}"))
            })
            .join()
            .map_err(|_| "auth_config.jwks_uri fetch thread panicked".to_string())?
    })
}
