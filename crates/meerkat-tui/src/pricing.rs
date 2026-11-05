//! Token prices for the session cost readout.
//!
//! A [`PriceTable`] answers "what does model `X` cost per million tokens"
//! from three sources, in precedence order:
//!
//! 1. `[pricing."<model id>"]` in `tui.toml` (layered like every other key,
//!    settable with `--set 'pricing."<id>".input=15'`); unset fields fall
//!    back to the sources below;
//! 2. LiteLLM's `GET /model/info` prices for the models of a LiteLLM
//!    endpoint (fetched with the model lists);
//! 3. the community price list LiteLLM maintains
//!    ([`COMMUNITY_PRICES_URL`]), fetched in the background at start-up and
//!    cached in `<config dir>/pricing-cache.json` for [`CACHE_TTL`].
//!
//! Lookups normalise ids: the exact id first, then without a provider prefix
//! (`anthropic/`, `openai/`, `gemini/`, `vertex_ai/`), then without a trailing
//! release date (`-YYYYMMDD` or `-YYYY-MM-DD`).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::approval::rules::PricingSection;

/// The community price list (LiteLLM's `model_prices_and_context_window.json`).
pub const COMMUNITY_PRICES_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
/// Timeout of the community price-list fetch.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest community price list accepted (the file is ~3.4 MB in 2026).
pub const MAX_PRICE_LIST_BYTES: usize = 32 * 1024 * 1024;
/// Age after which the cached community list is refreshed (and its prices
/// count as estimates until the refresh succeeds).
pub const CACHE_TTL: Duration = Duration::from_hours(7 * 24);
/// File name of the community price cache inside the global config dir.
pub const CACHE_FILE: &str = "pricing-cache.json";
/// Tokens per price unit.
pub const TOKENS_PER_UNIT: f64 = 1_000_000.0;
/// The community file's schema example (not a model).
const SAMPLE_SPEC_KEY: &str = "sample_spec";
/// Provider prefixes the community list uses in its keys.
const PROVIDER_PREFIXES: [&str; 4] = ["anthropic/", "openai/", "gemini/", "vertex_ai/"];

/// Prices of one model in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    /// Uncached input tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Input tokens read from the prompt cache (`None`: priced as `input`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<f64>,
    /// Input tokens written to the prompt cache (`None`: priced as `input`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl ModelPrice {
    /// Prices given per token (LiteLLM's unit), converted to per million.
    #[must_use]
    pub fn from_per_token(
        input: f64,
        output: f64,
        cache_read: Option<f64>,
        cache_creation: Option<f64>,
    ) -> Self {
        Self {
            input: input * TOKENS_PER_UNIT,
            output: output * TOKENS_PER_UNIT,
            cached_input: cache_read.map(|p| p * TOKENS_PER_UNIT),
            cache_write: cache_creation.map(|p| p * TOKENS_PER_UNIT),
        }
    }

    /// Whether every price is a finite, non-negative number.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let ok = |v: f64| v.is_finite() && v >= 0.0;
        ok(self.input)
            && ok(self.output)
            && self.cached_input.is_none_or(ok)
            && self.cache_write.is_none_or(ok)
    }

    /// `overrides` applied on top of `self` (or on nothing when `base` is
    /// `None`); `None` when input or output stays unknown.
    fn overridden(base: Option<Self>, overrides: &PricingSection) -> Option<Self> {
        Some(Self {
            input: overrides.input.or(base.map(|b| b.input))?,
            output: overrides.output.or(base.map(|b| b.output))?,
            cached_input: overrides.cached_input.or(base.and_then(|b| b.cached_input)),
            cache_write: overrides.cache_write.or(base.and_then(|b| b.cache_write)),
        })
    }
}

/// Where a model's price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceSource {
    /// `[pricing]` in `tui.toml` or `--set pricing…`.
    Config,
    /// A LiteLLM endpoint's `/model/info`.
    Endpoint,
    /// The community price list (fresh, or a cache within its TTL).
    Community,
    /// The community price list from a cache older than [`CACHE_TTL`].
    CommunityStale,
}

impl PriceSource {
    /// Whether prices from this source are only an estimate.
    #[must_use]
    pub fn is_estimate(self) -> bool {
        self == Self::CommunityStale
    }
}

impl fmt::Display for PriceSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Config => "tui.toml",
            Self::Endpoint => "endpoint /model/info",
            Self::Community => "community price list",
            Self::CommunityStale => "community price list (stale cache)",
        })
    }
}

/// Ids to try for `id`, most specific first: exact, without a provider
/// prefix, without a trailing date.
#[must_use]
pub fn id_candidates(id: &str) -> Vec<&str> {
    let unprefixed = strip_provider(id);
    let mut out = Vec::with_capacity(4);
    for candidate in [id, unprefixed, strip_date(id), strip_date(unprefixed)] {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// `id` without one of the [`PROVIDER_PREFIXES`].
fn strip_provider(id: &str) -> &str {
    PROVIDER_PREFIXES
        .iter()
        .find_map(|prefix| id.strip_prefix(prefix))
        .unwrap_or(id)
}

/// `id` without a trailing `-YYYYMMDD` or `-YYYY-MM-DD`.
fn strip_date(id: &str) -> &str {
    let is_date = |s: &str, dashed: bool| {
        let digits: String = s.chars().filter(|c| *c != '-').collect();
        let shape_ok = if dashed {
            s.len() == 10
                && s.as_bytes().get(4) == Some(&b'-')
                && s.as_bytes().get(7) == Some(&b'-')
        } else {
            s.len() == 8
        };
        shape_ok && digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit())
    };
    for (len, dashed) in [(9, false), (11, true)] {
        if id.len() > len
            && let Some((head, tail)) = id.split_at_checked(id.len() - len)
            && let Some(date) = tail.strip_prefix('-')
            && is_date(date, dashed)
        {
            return head;
        }
    }
    id
}

/// The community price list: prices by raw key (file order) plus a lookup
/// index that also holds the normalised aliases.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommunityPrices {
    models: IndexMap<String, ModelPrice>,
    index: HashMap<String, ModelPrice>,
    /// When the list was fetched (seconds since the Unix epoch).
    fetched_at: u64,
    /// Whether the list is older than [`CACHE_TTL`].
    stale: bool,
}

impl CommunityPrices {
    /// A list fetched at `fetched_at`, judged stale against `now` (both
    /// seconds since the Unix epoch).
    #[must_use]
    pub fn new(models: IndexMap<String, ModelPrice>, fetched_at: u64, now: u64) -> Self {
        let mut index: HashMap<String, ModelPrice> = models
            .iter()
            .map(|(key, price)| (key.clone(), *price))
            .collect();
        // Aliases never shadow an exact key; the first key in file order wins.
        for (key, price) in &models {
            for alias in id_candidates(key).into_iter().skip(1) {
                index.entry(alias.to_string()).or_insert(*price);
            }
        }
        Self {
            models,
            index,
            fetched_at,
            stale: now.saturating_sub(fetched_at) > CACHE_TTL.as_secs(),
        }
    }

    /// Whether nothing is known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Number of priced models in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the list is older than [`CACHE_TTL`].
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Whether a fetch should refresh the list (empty or stale).
    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        self.is_empty() || self.stale
    }

    fn get(&self, id: &str) -> Option<ModelPrice> {
        id_candidates(id)
            .into_iter()
            .find_map(|candidate| self.index.get(candidate).copied())
    }

    /// Decode the community file. Entries without numeric input and output
    /// prices (and keys like `sample_spec` that are not models) are skipped.
    ///
    /// # Errors
    ///
    /// [`PricingError::Decode`] when the body is not a JSON object.
    pub fn decode(body: &str) -> Result<IndexMap<String, ModelPrice>, PricingError> {
        let raw: IndexMap<String, CommunityEntry> =
            serde_json::from_str(body).map_err(|err| PricingError::Decode(err.to_string()))?;
        Ok(raw
            .into_iter()
            .filter(|(key, _)| key != SAMPLE_SPEC_KEY)
            .filter_map(|(key, entry)| Some((key, entry.price()?)))
            .collect())
    }

    /// Load the cache at `path`; `None` when missing or unreadable.
    #[must_use]
    pub fn load_cache(path: &Path, now: u64) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str::<CacheFile>(&text) {
            Ok(cache) => Some(Self::new(cache.models, cache.fetched_at, now)),
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "ignoring unreadable price cache");
                None
            }
        }
    }

    /// Write the list to `path` (temp file + rename).
    ///
    /// # Errors
    ///
    /// [`PricingError::Io`] on serialization or I/O failures.
    pub fn save_cache(&self, path: &Path) -> Result<(), PricingError> {
        let text = serde_json::to_string(&CacheFileRef {
            fetched_at: self.fetched_at,
            models: &self.models,
        })
        .map_err(|err| PricingError::Io(err.to_string()))?;
        let io = |err: std::io::Error| PricingError::Io(format!("{}: {err}", path.display()));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(format!(".{}.tmp", std::process::id()));
        let tmp = path.with_file_name(tmp_name);
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(|err| {
            let _ = std::fs::remove_file(&tmp);
            io(err)
        })
    }
}

/// On-disk form of the community price cache.
#[derive(Debug, Deserialize)]
struct CacheFile {
    fetched_at: u64,
    models: IndexMap<String, ModelPrice>,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    fetched_at: u64,
    models: &'a IndexMap<String, ModelPrice>,
}

/// One value of the community file: a model with prices, or anything else.
#[derive(Deserialize)]
#[serde(untagged)]
enum CommunityEntry {
    Model(CommunityModel),
    Other(IgnoredAny),
}

/// The fields of a community entry the cost readout uses; the rest is ignored.
#[derive(Deserialize)]
struct CommunityModel {
    #[serde(default, deserialize_with = "lenient_f64")]
    input_cost_per_token: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    output_cost_per_token: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    cache_read_input_token_cost: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    cache_creation_input_token_cost: Option<f64>,
}

/// A number, or a value of another type (treated as missing).
#[derive(Deserialize)]
#[serde(untagged)]
enum Lenient {
    Number(f64),
    Other(IgnoredAny),
}

/// Deserialize a number; `null` and values of other types become `None`
/// instead of failing the whole document.
pub(crate) fn lenient_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Option::<Lenient>::deserialize(deserializer)? {
        Some(Lenient::Number(n)) => Some(n),
        Some(Lenient::Other(_)) | None => None,
    })
}

impl CommunityEntry {
    fn price(&self) -> Option<ModelPrice> {
        let Self::Model(model) = self else {
            return None;
        };
        let price = ModelPrice::from_per_token(
            model.input_cost_per_token?,
            model.output_cost_per_token?,
            model.cache_read_input_token_cost,
            model.cache_creation_input_token_cost,
        );
        price.is_valid().then_some(price)
    }
}

/// Price lookups for the cost readout: overrides, endpoint prices and the
/// community list (see the module docs for precedence).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PriceTable {
    overrides: BTreeMap<String, PricingSection>,
    endpoint: HashMap<String, ModelPrice>,
    community: CommunityPrices,
}

impl PriceTable {
    /// A table with the `[pricing]` overrides from `tui.toml`.
    #[must_use]
    pub fn new(overrides: BTreeMap<String, PricingSection>) -> Self {
        Self {
            overrides,
            ..Self::default()
        }
    }

    /// Replace the community list.
    pub fn set_community(&mut self, community: CommunityPrices) {
        self.community = community;
    }

    /// The community list.
    #[must_use]
    pub fn community(&self) -> &CommunityPrices {
        &self.community
    }

    /// Replace the endpoint (LiteLLM `/model/info`) prices; the first price
    /// for an id wins.
    pub fn set_endpoint_prices(&mut self, prices: impl IntoIterator<Item = (String, ModelPrice)>) {
        self.endpoint.clear();
        for (id, price) in prices {
            if price.is_valid() {
                self.endpoint.entry(id).or_insert(price);
            }
        }
    }

    /// Whether any source has prices.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty() && self.endpoint.is_empty() && self.community.is_empty()
    }

    /// The price of `model` and its source, `None` when no source knows it.
    #[must_use]
    pub fn lookup(&self, model: &str) -> Option<(ModelPrice, PriceSource)> {
        let base = self.base(model);
        let configured = id_candidates(model)
            .into_iter()
            .find_map(|candidate| self.overrides.get(candidate))
            .and_then(|section| ModelPrice::overridden(base.map(|(p, _)| p), section));
        match configured {
            Some(price) => Some((price, PriceSource::Config)),
            None => base,
        }
    }

    /// Price from the fetched sources (endpoint, then community).
    fn base(&self, model: &str) -> Option<(ModelPrice, PriceSource)> {
        let endpoint = id_candidates(model)
            .into_iter()
            .find_map(|candidate| self.endpoint.get(candidate).copied());
        if let Some(price) = endpoint {
            return Some((price, PriceSource::Endpoint));
        }
        let source = if self.community.is_stale() {
            PriceSource::CommunityStale
        } else {
            PriceSource::Community
        };
        self.community.get(model).map(|price| (price, source))
    }
}

/// Failure to fetch or store the community price list.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PricingError {
    /// Transport failure (the URL is stripped).
    #[error("network error: {0}")]
    Http(String),
    /// Non-success status.
    #[error("HTTP {0}")]
    Status(u16),
    /// The body is not the expected JSON.
    #[error("unexpected price list: {0}")]
    Decode(String),
    /// The body exceeded [`MAX_PRICE_LIST_BYTES`].
    #[error("price list larger than {} MiB", MAX_PRICE_LIST_BYTES / (1024 * 1024))]
    TooLarge,
    /// Reading or writing the cache failed.
    #[error("price cache: {0}")]
    Io(String),
}

/// Seconds since the Unix epoch, now.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Download and decode the community list from `url`.
///
/// # Errors
///
/// Transport, status or decoding failures.
pub async fn fetch_community(
    url: &str,
    timeout: Duration,
) -> Result<IndexMap<String, ModelPrice>, PricingError> {
    let http = |err: reqwest::Error| PricingError::Http(err.without_url().to_string());
    let mut builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout);
    if crate::models::is_loopback_url(url) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(http)?;
    let mut response = client.get(url).send().await.map_err(http)?;
    let status = response.status();
    if !status.is_success() {
        return Err(PricingError::Status(status.as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > MAX_PRICE_LIST_BYTES as u64)
    {
        return Err(PricingError::TooLarge);
    }
    // Read in chunks so a missing or lying Content-Length cannot exhaust memory.
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(http)? {
        if body.len() + chunk.len() > MAX_PRICE_LIST_BYTES {
            return Err(PricingError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(body).map_err(|err| PricingError::Decode(err.to_string()))?;
    CommunityPrices::decode(&body)
}

/// How the integrator gets its prices: the start-up table (overrides and
/// the cached community list), where the cache lives and where to refresh it.
#[derive(Debug, Clone, Default)]
pub struct PricingSetup {
    /// The prices known at start-up.
    pub table: PriceTable,
    /// The community price cache (`None`: no global config dir).
    pub cache_path: Option<PathBuf>,
    /// Where to fetch the community list (`None`: `--no-pricing-fetch`, tests).
    pub fetch_url: Option<String>,
}

impl PricingSetup {
    /// Overrides plus the cache at `cache_path` (read now; cheap). With
    /// `fetch`, the community list is refreshed from [`COMMUNITY_PRICES_URL`]
    /// when the cache is missing or stale.
    #[must_use]
    pub fn load(
        overrides: BTreeMap<String, PricingSection>,
        cache_path: Option<PathBuf>,
        fetch: bool,
    ) -> Self {
        let mut table = PriceTable::new(overrides);
        if let Some(cached) = cache_path
            .as_deref()
            .and_then(|path| CommunityPrices::load_cache(path, unix_now()))
        {
            table.set_community(cached);
        }
        Self {
            table,
            cache_path,
            fetch_url: fetch.then(|| COMMUNITY_PRICES_URL.to_string()),
        }
    }

    /// Where to refresh the community list from, when it needs refreshing.
    #[must_use]
    pub fn refresh_url(&self) -> Option<&str> {
        self.fetch_url
            .as_deref()
            .filter(|_| self.table.community().needs_refresh())
    }
}

/// Fetch the community list from `url` and write it to `cache_path`
/// (a failed write is logged; the fetched list is still returned).
///
/// # Errors
///
/// Fetch failures (see [`fetch_community`]).
pub async fn refresh_community(
    url: &str,
    cache_path: Option<&Path>,
    timeout: Duration,
) -> Result<CommunityPrices, PricingError> {
    let models = fetch_community(url, timeout).await?;
    let now = unix_now();
    let community = CommunityPrices::new(models, now, now);
    if let Some(path) = cache_path
        && let Err(err) = community.save_cache(path)
    {
        tracing::warn!(error = %err, "could not write the price cache");
    }
    Ok(community)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "sample_spec": {
            "max_tokens": "LEGACY parameter. set to max_output_tokens if provider specifies it.",
            "input_cost_per_token": 0.0,
            "output_cost_per_token": 0.0,
            "litellm_provider": "one of https://docs.litellm.ai/docs/providers"
        },
        "claude-opus-4-6-20260115": {
            "max_input_tokens": 200000,
            "input_cost_per_token": 5e-06,
            "output_cost_per_token": 2.5e-05,
            "cache_read_input_token_cost": 5e-07,
            "cache_creation_input_token_cost": 6.25e-06,
            "litellm_provider": "anthropic",
            "supports_vision": true
        },
        "anthropic/claude-sonnet-4-6": {
            "input_cost_per_token": 3e-06,
            "output_cost_per_token": 1.5e-05
        },
        "gpt-5.4": {
            "input_cost_per_token": 1.25e-06,
            "output_cost_per_token": 1e-05,
            "cache_read_input_token_cost": 1.25e-07
        },
        "text-embedding-3-small": {"input_cost_per_token": 2e-08, "mode": "embedding"},
        "weird": {"input_cost_per_token": "free", "output_cost_per_token": 1e-06},
        "nulls": {"input_cost_per_token": null, "output_cost_per_token": null},
        "not-an-object": 42,
        "gemini/gemini-3.1-pro-preview": {
            "input_cost_per_token": 2e-06,
            "output_cost_per_token": 1.2e-05
        },
        "vertex_ai/gemini-3.1-pro-preview": {
            "input_cost_per_token": 9e-06,
            "output_cost_per_token": 9e-06
        }
    }"#;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn community(now_offset: u64) -> CommunityPrices {
        CommunityPrices::new(
            CommunityPrices::decode(FIXTURE).unwrap(),
            1_000,
            1_000 + now_offset,
        )
    }

    #[test]
    fn decodes_fixture_and_skips_unpriced_entries() {
        let models = CommunityPrices::decode(FIXTURE).unwrap();
        let keys: Vec<&str> = models.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "claude-opus-4-6-20260115",
                "anthropic/claude-sonnet-4-6",
                "gpt-5.4",
                "gemini/gemini-3.1-pro-preview",
                "vertex_ai/gemini-3.1-pro-preview",
            ]
        );
        let opus = models["claude-opus-4-6-20260115"];
        assert!(approx(opus.input, 5.0));
        assert!(approx(opus.output, 25.0));
        assert!(approx(opus.cached_input.unwrap(), 0.5));
        assert!(approx(opus.cache_write.unwrap(), 6.25));
        let gpt = models["gpt-5.4"];
        assert!(approx(gpt.cached_input.unwrap(), 0.125));
        assert!(gpt.cache_write.is_none());
        assert!(matches!(
            CommunityPrices::decode("[1, 2]"),
            Err(PricingError::Decode(_))
        ));
    }

    #[test]
    fn id_normalisation_table() {
        let cases: &[(&str, &[&str])] = &[
            ("gpt-5.4", &["gpt-5.4"]),
            (
                "anthropic/claude-opus-4-6-20260115",
                &[
                    "anthropic/claude-opus-4-6-20260115",
                    "claude-opus-4-6-20260115",
                    "anthropic/claude-opus-4-6",
                    "claude-opus-4-6",
                ],
            ),
            (
                "claude-opus-4-6-20260115",
                &["claude-opus-4-6-20260115", "claude-opus-4-6"],
            ),
            ("gpt-4o-2024-08-06", &["gpt-4o-2024-08-06", "gpt-4o"]),
            (
                "vertex_ai/gemini-3-flash",
                &["vertex_ai/gemini-3-flash", "gemini-3-flash"],
            ),
            // Not dates: wrong length or non-digits.
            ("model-1234567", &["model-1234567"]),
            ("model-2024010x", &["model-2024010x"]),
            ("-20250101", &["-20250101"]),
        ];
        for (id, expected) in cases {
            assert_eq!(id_candidates(id), expected.to_vec(), "{id}");
        }
    }

    #[test]
    fn lookups_follow_normalisation_and_file_order() {
        let mut table = PriceTable::default();
        table.set_community(community(0));
        let (opus, source) = table.lookup("claude-opus-4-6").unwrap();
        assert!(approx(opus.input, 5.0));
        assert_eq!(source, PriceSource::Community);
        assert!(table.lookup("anthropic/claude-opus-4-6-20260115").is_some());
        assert!(table.lookup("claude-sonnet-4-6").is_some());
        assert!(table.lookup("claude-sonnet-4-6-20260301").is_some());
        // `gemini/` comes first in the file, so its alias wins.
        let (gemini, _) = table.lookup("gemini-3.1-pro-preview").unwrap();
        assert!(approx(gemini.input, 2.0));
        assert!(table.lookup("text-embedding-3-small").is_none());
        assert!(table.lookup("unknown-model").is_none());
    }

    #[test]
    fn precedence_override_then_endpoint_then_community() {
        let overrides = BTreeMap::from([
            (
                "gpt-5.4".to_string(),
                PricingSection {
                    output: Some(99.0),
                    ..PricingSection::default()
                },
            ),
            (
                "local-only".to_string(),
                PricingSection {
                    input: Some(1.0),
                    ..PricingSection::default()
                },
            ),
        ]);
        let mut table = PriceTable::new(overrides);
        table.set_community(community(0));
        table.set_endpoint_prices([
            (
                "gpt-5.4".to_string(),
                ModelPrice::from_per_token(2e-06, 3e-06, None, None),
            ),
            (
                "claude-sonnet-4-6".to_string(),
                ModelPrice::from_per_token(1e-06, 1e-06, None, None),
            ),
        ]);
        // Override fields over the endpoint price.
        let (gpt, source) = table.lookup("gpt-5.4").unwrap();
        assert_eq!(source, PriceSource::Config);
        assert!(approx(gpt.input, 2.0) && approx(gpt.output, 99.0));
        // Endpoint over community.
        let (sonnet, source) = table.lookup("claude-sonnet-4-6").unwrap();
        assert_eq!(source, PriceSource::Endpoint);
        assert!(approx(sonnet.input, 1.0));
        // Community alone.
        assert_eq!(
            table.lookup("claude-opus-4-6").unwrap().1,
            PriceSource::Community
        );
        // An override without output and nothing underneath is not a price.
        assert!(table.lookup("local-only").is_none());
    }

    #[test]
    fn staleness_marks_estimates() {
        let mut table = PriceTable::default();
        table.set_community(community(CACHE_TTL.as_secs() + 1));
        assert!(table.community().needs_refresh());
        let (_, source) = table.lookup("gpt-5.4").unwrap();
        assert_eq!(source, PriceSource::CommunityStale);
        assert!(source.is_estimate());
        assert!(!community(CACHE_TTL.as_secs()).is_stale());
    }

    #[test]
    fn cache_round_trip_and_ttl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg").join(CACHE_FILE);
        assert!(CommunityPrices::load_cache(&path, 0).is_none());
        let fresh = community(0);
        fresh.save_cache(&path).unwrap();
        let back = CommunityPrices::load_cache(&path, 1_000).unwrap();
        assert_eq!(back, fresh);
        let old = CommunityPrices::load_cache(&path, 1_000 + CACHE_TTL.as_secs() + 1).unwrap();
        assert!(old.is_stale());
        std::fs::write(&path, "not json").unwrap();
        assert!(CommunityPrices::load_cache(&path, 0).is_none());
    }

    #[test]
    fn setup_reads_cache_and_no_fetch_disables_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CACHE_FILE);
        // No cache: a fetch is wanted unless disabled.
        let setup = PricingSetup::load(BTreeMap::new(), Some(path.clone()), true);
        assert_eq!(setup.refresh_url(), Some(COMMUNITY_PRICES_URL));
        let setup = PricingSetup::load(BTreeMap::new(), Some(path.clone()), false);
        assert_eq!(setup.refresh_url(), None);
        // A fresh cache is used right away and needs no fetch.
        let now = unix_now();
        CommunityPrices::new(CommunityPrices::decode(FIXTURE).unwrap(), now, now)
            .save_cache(&path)
            .unwrap();
        let setup = PricingSetup::load(BTreeMap::new(), Some(path.clone()), true);
        assert!(setup.table.lookup("gpt-5.4").is_some());
        assert_eq!(setup.refresh_url(), None);
        // A stale cache is used and refreshed.
        CommunityPrices::new(CommunityPrices::decode(FIXTURE).unwrap(), 0, now)
            .save_cache(&path)
            .unwrap();
        let setup = PricingSetup::load(BTreeMap::new(), Some(path), true);
        assert_eq!(
            setup.table.lookup("gpt-5.4").map(|(_, s)| s),
            Some(PriceSource::CommunityStale)
        );
        assert_eq!(setup.refresh_url(), Some(COMMUNITY_PRICES_URL));
    }

    #[tokio::test]
    async fn refresh_from_a_local_stub_writes_the_cache() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/prices.json", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let _ = socket.read(&mut buf).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{FIXTURE}",
                FIXTURE.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CACHE_FILE);
        let fetched = refresh_community(&url, Some(&path), FETCH_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(fetched.len(), 5);
        assert!(!fetched.is_stale());
        let cached = CommunityPrices::load_cache(&path, unix_now()).unwrap();
        assert_eq!(cached.len(), 5);
    }

    #[tokio::test]
    async fn oversized_price_list_is_refused() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/prices.json", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0_u8; 4096];
            let _ = socket.read(&mut buf).await.unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{{",
                MAX_PRICE_LIST_BYTES + 1
            );
            let _ = socket.write_all(response.as_bytes()).await;
        });
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(CACHE_FILE);
        let err = refresh_community(&url, Some(&path), FETCH_TIMEOUT)
            .await
            .unwrap_err();
        assert_eq!(err, PricingError::TooLarge);
        assert!(!path.exists(), "nothing cached");
    }
}
