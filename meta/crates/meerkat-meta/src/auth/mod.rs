//! JWT validation, JWKS caching, and OIDC discovery for API authentication.

pub mod peer_keys;
pub mod session_fence;

pub use peer_keys::{
    GatewayPeerKeyError, GatewayPeerKeys, KEY_FILE_NAME as GATEWAY_PEER_KEY_FILE,
    PubkeyDecodeError, decode_pubkey_b64,
};

use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use tokio::sync::RwLock;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwtValidationConfig {
    pub shared_secret: String,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub now_epoch_seconds: u64,
    pub leeway_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwtClaimsValidationConfig {
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub now_epoch_seconds: u64,
    pub leeway_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtVerificationKey {
    Hs256(Vec<u8>),
    Rs256 { modulus: Vec<u8>, exponent: Vec<u8> },
    Es256P256 { public_key: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtValidationError {
    InvalidFormat,
    InvalidBase64,
    InvalidJson,
    UnsupportedAlgorithm(String),
    InvalidSignature,
    Expired,
    NotYetValid,
    IssuerMismatch,
    AudienceMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedJwt {
    pub subject: Option<String>,
    pub email: Option<String>,
    /// The `email_verified` claim when present as a boolean; `None` when
    /// absent or not a boolean.
    pub email_verified: Option<bool>,
    pub provider: Option<String>,
    pub actor_type: Option<String>,
    pub issuer: Option<String>,
    pub audience: Vec<String>,
    pub expires_at_epoch_seconds: Option<u64>,
    pub not_before_epoch_seconds: Option<u64>,
    /// The `iat` claim when present as a whole number of seconds since the
    /// epoch: when the issuer says it minted the token. `None` when absent
    /// or not a whole number.
    pub issued_at_epoch_seconds: Option<u64>,
    /// The `sid` claim when present as a string: an opaque session or login
    /// identifier within the issuer's namespace, kept exactly as signed.
    /// `None` when absent or not a string.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct JwtHeader {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidcDiscoveryDocument {
    pub issuer: String,
    pub jwks_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwksDocument {
    pub keys: Vec<Jwk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwk {
    pub kid: Option<String>,
    pub kty: String,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub k: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OidcContractError {
    InvalidJson,
    MissingIssuer,
    MissingJwksUri,
    MissingKeys,
    NoMatchingKey,
    UnsupportedKeyType(String),
    UnsupportedJwtAlgorithm(String),
    MissingSymmetricKeyMaterial,
    MissingRsaKeyMaterial,
    MissingEcKeyMaterial,
    InvalidKeyEncoding,
    InvalidSymmetricKeyMaterial,
    InvalidRsaKeyMaterial,
    InvalidEcKeyMaterial,
    UnsupportedEllipticCurve(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwtHeaderView {
    pub alg: String,
    pub kid: Option<String>,
}

pub fn validate_jwt_locally(
    token: &str,
    config: &JwtValidationConfig,
) -> Result<ValidatedJwt, JwtValidationError> {
    let claims_config = JwtClaimsValidationConfig {
        issuer: config.issuer.clone(),
        audience: config.audience.clone(),
        now_epoch_seconds: config.now_epoch_seconds,
        leeway_seconds: config.leeway_seconds,
    };
    let key = JwtVerificationKey::Hs256(config.shared_secret.as_bytes().to_vec());
    validate_jwt_with_verification_key(token, &key, &claims_config)
}

pub fn validate_jwt_with_verification_key(
    token: &str,
    verification_key: &JwtVerificationKey,
    config: &JwtClaimsValidationConfig,
) -> Result<ValidatedJwt, JwtValidationError> {
    let parsed = parse_jwt(token)?;
    if !key_supports_algorithm(verification_key, parsed.header.alg.as_str()) {
        return Err(JwtValidationError::UnsupportedAlgorithm(parsed.header.alg));
    }
    verify_signature(
        verification_key,
        parsed.header.alg.as_str(),
        parsed.signing_input.as_bytes(),
        &parsed.signature,
    )?;
    validate_claims(&parsed.claims, config)
}

pub fn build_jwt_verification_key(
    key: &Jwk,
    alg: &str,
) -> Result<JwtVerificationKey, OidcContractError> {
    match alg {
        "HS256" => build_hs256_key(key),
        "RS256" => build_rs256_key(key),
        "ES256" => build_es256_key(key),
        unsupported => Err(OidcContractError::UnsupportedJwtAlgorithm(
            unsupported.to_string(),
        )),
    }
}

fn build_hs256_key(key: &Jwk) -> Result<JwtVerificationKey, OidcContractError> {
    if key.kty != "oct" {
        return Err(OidcContractError::UnsupportedKeyType(key.kty.clone()));
    }
    let encoded = key
        .k
        .as_deref()
        .ok_or(OidcContractError::MissingSymmetricKeyMaterial)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| OidcContractError::InvalidKeyEncoding)?;
    Ok(JwtVerificationKey::Hs256(bytes))
}

fn build_rs256_key(key: &Jwk) -> Result<JwtVerificationKey, OidcContractError> {
    if key.kty != "RSA" {
        return Err(OidcContractError::UnsupportedKeyType(key.kty.clone()));
    }
    let modulus = key
        .n
        .as_deref()
        .ok_or(OidcContractError::MissingRsaKeyMaterial)
        .and_then(decode_key_component)?;
    let exponent = key
        .e
        .as_deref()
        .ok_or(OidcContractError::MissingRsaKeyMaterial)
        .and_then(decode_key_component)?;
    if modulus.is_empty() || exponent.is_empty() {
        return Err(OidcContractError::InvalidRsaKeyMaterial);
    }
    Ok(JwtVerificationKey::Rs256 { modulus, exponent })
}

fn build_es256_key(key: &Jwk) -> Result<JwtVerificationKey, OidcContractError> {
    if key.kty != "EC" {
        return Err(OidcContractError::UnsupportedKeyType(key.kty.clone()));
    }
    let curve = key
        .crv
        .as_deref()
        .ok_or(OidcContractError::MissingEcKeyMaterial)?;
    if curve != "P-256" {
        return Err(OidcContractError::UnsupportedEllipticCurve(
            curve.to_string(),
        ));
    }

    let x = key
        .x
        .as_deref()
        .ok_or(OidcContractError::MissingEcKeyMaterial)
        .and_then(decode_key_component)?;
    let y = key
        .y
        .as_deref()
        .ok_or(OidcContractError::MissingEcKeyMaterial)
        .and_then(decode_key_component)?;
    if x.len() != 32 || y.len() != 32 {
        return Err(OidcContractError::InvalidEcKeyMaterial);
    }

    let mut public_key = Vec::with_capacity(65);
    public_key.push(0x04);
    public_key.extend_from_slice(&x);
    public_key.extend_from_slice(&y);
    Ok(JwtVerificationKey::Es256P256 { public_key })
}

fn decode_key_component(encoded: &str) -> Result<Vec<u8>, OidcContractError> {
    URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| OidcContractError::InvalidKeyEncoding)
}

struct ParsedJwt {
    header: JwtHeader,
    claims: Value,
    signing_input: String,
    signature: Vec<u8>,
}

fn parse_jwt(token: &str) -> Result<ParsedJwt, JwtValidationError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtValidationError::InvalidFormat);
    }

    let header_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|_| JwtValidationError::InvalidBase64)?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|_| JwtValidationError::InvalidBase64)?;
    let signature = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| JwtValidationError::InvalidBase64)?;

    let header: JwtHeader =
        serde_json::from_slice(&header_bytes).map_err(|_| JwtValidationError::InvalidJson)?;
    let claims: Value =
        serde_json::from_slice(&payload_bytes).map_err(|_| JwtValidationError::InvalidJson)?;
    let signing_input = format!("{}.{}", parts[0], parts[1]);

    Ok(ParsedJwt {
        header,
        claims,
        signing_input,
        signature,
    })
}

fn key_supports_algorithm(key: &JwtVerificationKey, alg: &str) -> bool {
    matches!(
        (key, alg),
        (JwtVerificationKey::Hs256(_), "HS256")
            | (JwtVerificationKey::Rs256 { .. }, "RS256")
            | (JwtVerificationKey::Es256P256 { .. }, "ES256")
    )
}

fn verify_signature(
    verification_key: &JwtVerificationKey,
    alg: &str,
    signing_input: &[u8],
    signature: &[u8],
) -> Result<(), JwtValidationError> {
    match (verification_key, alg) {
        (JwtVerificationKey::Hs256(secret), "HS256") => {
            let mut mac = HmacSha256::new_from_slice(secret)
                .map_err(|_| JwtValidationError::InvalidSignature)?;
            mac.update(signing_input);
            // Pre-fix: byte-wise loop short-circuited on first mismatch
            // — a remote timing oracle for HMAC tag forgery one byte at
            // a time. `Mac::verify_slice` performs the comparison in
            // constant time via the underlying HMAC implementation.
            mac.verify_slice(signature)
                .map_err(|_| JwtValidationError::InvalidSignature)?;
            Ok(())
        }
        (JwtVerificationKey::Rs256 { modulus, exponent }, "RS256") => {
            let components = RsaPublicKeyComponents {
                n: modulus.as_slice(),
                e: exponent.as_slice(),
            };
            components
                .verify(
                    &signature::RSA_PKCS1_2048_8192_SHA256,
                    signing_input,
                    signature,
                )
                .map_err(|_| JwtValidationError::InvalidSignature)
        }
        (JwtVerificationKey::Es256P256 { public_key }, "ES256") => {
            UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, public_key.as_slice())
                .verify(signing_input, signature)
                .map_err(|_| JwtValidationError::InvalidSignature)
        }
        (_, unsupported) => Err(JwtValidationError::UnsupportedAlgorithm(
            unsupported.to_string(),
        )),
    }
}

fn validate_claims(
    claims: &Value,
    config: &JwtClaimsValidationConfig,
) -> Result<ValidatedJwt, JwtValidationError> {
    let exp = claims.get("exp").and_then(Value::as_u64);
    let nbf = claims.get("nbf").and_then(Value::as_u64);
    let iss = claims
        .get("iss")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let aud = extract_audiences(claims);

    if let Some(exp) = exp {
        let threshold = config
            .now_epoch_seconds
            .saturating_sub(config.leeway_seconds);
        if exp < threshold {
            return Err(JwtValidationError::Expired);
        }
    }
    if let Some(nbf) = nbf {
        let threshold = config
            .now_epoch_seconds
            .saturating_add(config.leeway_seconds);
        if nbf > threshold {
            return Err(JwtValidationError::NotYetValid);
        }
    }

    if let Some(expected_issuer) = &config.issuer
        && iss.as_deref() != Some(expected_issuer.as_str())
    {
        return Err(JwtValidationError::IssuerMismatch);
    }
    if let Some(expected_audience) = &config.audience
        && !aud.iter().any(|entry| entry == expected_audience)
    {
        return Err(JwtValidationError::AudienceMismatch);
    }

    Ok(ValidatedJwt {
        subject: claims
            .get("sub")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        email: claims
            .get("email")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        email_verified: claims.get("email_verified").and_then(Value::as_bool),
        provider: claims
            .get("provider")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        actor_type: claims
            .get("actor_type")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        issuer: iss,
        audience: aud,
        expires_at_epoch_seconds: exp,
        not_before_epoch_seconds: nbf,
        issued_at_epoch_seconds: claims.get("iat").and_then(Value::as_u64),
        session_id: claims
            .get("sid")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

pub fn inspect_jwt_header(token: &str) -> Result<JwtHeaderView, JwtValidationError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtValidationError::InvalidFormat);
    }
    let header_bytes = URL_SAFE_NO_PAD
        .decode(parts[0])
        .map_err(|_| JwtValidationError::InvalidBase64)?;
    let header: JwtHeader =
        serde_json::from_slice(&header_bytes).map_err(|_| JwtValidationError::InvalidJson)?;
    Ok(JwtHeaderView {
        alg: header.alg,
        kid: header.kid,
    })
}

pub fn parse_oidc_discovery_json(
    json_text: &str,
) -> Result<OidcDiscoveryDocument, OidcContractError> {
    let doc: OidcDiscoveryDocument =
        serde_json::from_str(json_text).map_err(|_| OidcContractError::InvalidJson)?;
    if doc.issuer.trim().is_empty() {
        return Err(OidcContractError::MissingIssuer);
    }
    if doc.jwks_uri.trim().is_empty() {
        return Err(OidcContractError::MissingJwksUri);
    }
    Ok(doc)
}

pub fn parse_jwks_json(json_text: &str) -> Result<JwksDocument, OidcContractError> {
    let doc: JwksDocument =
        serde_json::from_str(json_text).map_err(|_| OidcContractError::InvalidJson)?;
    if doc.keys.is_empty() {
        return Err(OidcContractError::MissingKeys);
    }
    Ok(doc)
}

pub fn select_jwk_for_token<'a>(
    jwks: &'a JwksDocument,
    kid: Option<&str>,
    alg: &str,
) -> Result<&'a Jwk, OidcContractError> {
    if let Some(kid) = kid {
        return jwks
            .keys
            .iter()
            .find(|key| {
                key.kid.as_deref() == Some(kid)
                    && key.alg.as_deref().is_none_or(|key_alg| key_alg == alg)
            })
            .ok_or(OidcContractError::NoMatchingKey);
    }

    jwks.keys
        .iter()
        .find(|key| key.alg.as_deref().is_none_or(|key_alg| key_alg == alg))
        .ok_or(OidcContractError::NoMatchingKey)
}

pub fn extract_hs256_shared_secret(key: &Jwk) -> Result<String, OidcContractError> {
    let bytes = match build_hs256_key(key)? {
        JwtVerificationKey::Hs256(bytes) => bytes,
        _ => return Err(OidcContractError::InvalidSymmetricKeyMaterial),
    };
    String::from_utf8(bytes).map_err(|_| OidcContractError::InvalidSymmetricKeyMaterial)
}

fn extract_audiences(claims: &Value) -> Vec<String> {
    match claims.get("aud") {
        Some(Value::String(aud)) => vec![aud.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// JwksCache — runtime JWKS cache with discovery, rotation, kid-miss refresh
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum JwksCacheError {
    Discovery(OidcContractError),
    Http(String),
    Validation(JwtValidationError),
    NoMatchingKey,
    NotInitialized,
    /// The token's header requested HS256 (symmetric) but the cache is not
    /// configured to allow it. Rejected by default to prevent HS256 algorithm
    /// confusion if the IdP's JWKS publishes any `oct`/alg-less key.
    Hs256NotAllowed,
    /// No audience is configured, so the token's `aud` claim cannot be bound.
    /// Failing closed prevents cross-RP token reuse (any unexpired token signed
    /// by any key in the IdP's JWKS would otherwise be accepted).
    AudienceRequired,
}

impl Display for JwksCacheError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discovery(err) => write!(f, "OIDC discovery error: {err:?}"),
            Self::Http(msg) => write!(f, "HTTP fetch error: {msg}"),
            Self::Validation(err) => write!(f, "JWT validation error: {err:?}"),
            Self::NoMatchingKey => write!(f, "no matching JWK for token"),
            Self::NotInitialized => write!(f, "JWKS cache not initialized"),
            Self::Hs256NotAllowed => {
                write!(f, "HS256 tokens are not accepted by this JWKS cache")
            }
            Self::AudienceRequired => write!(
                f,
                "JWKS cache requires a configured audience to validate tokens"
            ),
        }
    }
}

impl std::error::Error for JwksCacheError {}

#[derive(Debug, Clone)]
pub struct JwksCacheConfig {
    pub discovery_url: String,
    pub refresh_interval: Duration,
    pub http_timeout: Duration,
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub leeway_seconds: u64,
    /// Opt-in escape hatch for HS256 (symmetric) tokens. Defaults to `false`:
    /// this security primitive rejects HS256 by default to avoid algorithm
    /// confusion if the IdP's JWKS publishes an `oct`/alg-less key. Only set to
    /// `true` for a trusted local/dev IdP you control.
    pub allow_hs256: bool,
}

impl JwksCacheConfig {
    pub fn new(discovery_url: String) -> Self {
        Self {
            discovery_url,
            refresh_interval: Duration::from_hours(1),
            http_timeout: Duration::from_secs(10),
            issuer: None,
            audience: None,
            leeway_seconds: 60,
            allow_hs256: false,
        }
    }

    /// Set the expected audience. Required: the cache fails closed when no
    /// audience is configured, preventing cross-RP token reuse.
    #[must_use]
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = Some(audience.into());
        self
    }

    /// Set the expected issuer.
    #[must_use]
    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }

    /// Opt in to accepting HS256 (symmetric) tokens. Only for a trusted local
    /// IdP you control — see [`Self::allow_hs256`].
    #[must_use]
    pub fn allow_hs256(mut self, allow: bool) -> Self {
        self.allow_hs256 = allow;
        self
    }
}

struct JwksCacheInner {
    jwks: Option<JwksDocument>,
    last_refresh: Option<Instant>,
}

#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<JwksCacheInner>>,
    config: Arc<JwksCacheConfig>,
    http_client: reqwest::Client,
}

impl JwksCache {
    pub fn new(config: JwksCacheConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(config.http_timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner: Arc::new(RwLock::new(JwksCacheInner {
                jwks: None,
                last_refresh: None,
            })),
            config: Arc::new(config),
            http_client,
        }
    }

    /// Fetch OIDC discovery document and JWKS keys. Called on first use or periodic refresh.
    pub async fn refresh(&self) -> Result<(), JwksCacheError> {
        let discovery_json = fetch_json(&self.http_client, &self.config.discovery_url).await?;
        let discovery =
            parse_oidc_discovery_json(&discovery_json).map_err(JwksCacheError::Discovery)?;

        let jwks_json = fetch_json(&self.http_client, &discovery.jwks_uri).await?;
        let jwks = parse_jwks_json(&jwks_json).map_err(JwksCacheError::Discovery)?;

        let mut inner = self.inner.write().await;
        inner.jwks = Some(jwks);
        inner.last_refresh = Some(Instant::now());
        Ok(())
    }

    /// Validate a JWT token using cached JWKS. Refreshes on kid miss.
    pub async fn validate_token(&self, token: &str) -> Result<ValidatedJwt, JwksCacheError> {
        self.maybe_refresh().await?;

        let header = inspect_jwt_header(token).map_err(JwksCacheError::Validation)?;

        // First attempt: try to find key in current cache.
        match self.try_validate(token, &header).await {
            Ok(jwt) => return Ok(jwt),
            Err(JwksCacheError::NoMatchingKey) => {
                // Kid miss — force refresh and retry once.
            }
            Err(err) => return Err(err),
        }

        self.refresh().await?;
        self.try_validate(token, &header).await
    }

    async fn try_validate(
        &self,
        token: &str,
        header: &JwtHeaderView,
    ) -> Result<ValidatedJwt, JwksCacheError> {
        // Fail closed BEFORE selecting a key. (1) Reject HS256 by default so a
        // symmetric-key/alg-confusion token cannot be validated against any
        // `oct`/alg-less JWK. (2) Require a configured audience so this token is
        // bound to THIS relying party — without it any unexpired token signed by
        // any key in the IdP's JWKS would be accepted (cross-RP token reuse).
        if header.alg.eq_ignore_ascii_case("HS256") && !self.config.allow_hs256 {
            return Err(JwksCacheError::Hs256NotAllowed);
        }
        if self
            .config
            .audience
            .as_deref()
            .is_none_or(|audience| audience.trim().is_empty())
        {
            return Err(JwksCacheError::AudienceRequired);
        }

        let inner = self.inner.read().await;
        let jwks = inner.jwks.as_ref().ok_or(JwksCacheError::NotInitialized)?;

        let jwk = select_jwk_for_token(jwks, header.kid.as_deref(), &header.alg)
            .map_err(|_| JwksCacheError::NoMatchingKey)?;

        let verification_key =
            build_jwt_verification_key(jwk, &header.alg).map_err(JwksCacheError::Discovery)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let claims_config = JwtClaimsValidationConfig {
            issuer: self.config.issuer.clone(),
            audience: self.config.audience.clone(),
            now_epoch_seconds: now,
            leeway_seconds: self.config.leeway_seconds,
        };

        validate_jwt_with_verification_key(token, &verification_key, &claims_config)
            .map_err(JwksCacheError::Validation)
    }

    /// Check if cache needs periodic refresh and refresh if needed.
    async fn maybe_refresh(&self) -> Result<(), JwksCacheError> {
        let needs_refresh = {
            let inner = self.inner.read().await;
            match inner.last_refresh {
                Some(last) => last.elapsed() >= self.config.refresh_interval,
                None => true,
            }
        };
        if needs_refresh {
            self.refresh().await?;
        }
        Ok(())
    }
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<String, JwksCacheError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| JwksCacheError::Http(format!("{err}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(JwksCacheError::Http(format!("HTTP {status} from {url}")));
    }
    response
        .text()
        .await
        .map_err(|err| JwksCacheError::Http(format!("{err}")))
}
