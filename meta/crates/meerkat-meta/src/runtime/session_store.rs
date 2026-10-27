//! Session store subsystem — persistence backends and session lifecycle operations.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStoreKind {
    BigQuery,
    JsonFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStoreContract {
    pub store: SessionStoreKind,
    pub latest_row_per_session: bool,
    pub tombstones_supported: bool,
    pub dedup_read_path: bool,
    pub file_locking: bool,
    pub crash_recovery: bool,
    pub bigquery_dataset: Option<String>,
    pub bigquery_table: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPersistenceRow {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonStoreLockRecord {
    pub owner_pid: u32,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonFileSessionStoreError {
    Io(String),
    Serialize(String),
    InvalidStoreData(String),
    LockHeld { lock_path: String },
    StaleLockRecoveryFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonFileSessionStore {
    data_path: PathBuf,
    lock_path: PathBuf,
    stale_lock_threshold: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BigQuerySessionStoreError {
    Io(String),
    Serialize(String),
    Configuration(String),
    Http(String),
    Api(String),
    InvalidQueryResponse(String),
    ProcessFailed { command: String, stderr: String },
}

/// One process-wide HTTP client for the BigQuery adapter. First
/// `reqwest::Client::build()` in a process performs the system-proxy scan
/// (macOS SystemConfiguration — ~700ms observed once the `system-proxy`
/// feature is unified in by the realtime provider chain); sharing the built
/// client pays that exactly once, off the per-RPC path after first use, and
/// per-request timeouts replace per-adapter client rebuilds.
pub(crate) fn shared_bigquery_http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| reqwest::Client::builder().build().unwrap_or_default())
        .clone()
}

/// The start of the error a request gets when BigQuery answers 401, whatever
/// supplied the token.
const ACCESS_TOKEN_REFUSED: &str = "BigQuery API request failed (status 401)";
/// The start of the error when BigQuery refuses a token a source had just
/// refreshed.
const ACCESS_TOKEN_REFUSED_AFTER_REFRESH: &str =
    "BigQuery refused the access token (401) again after a refresh";
/// The start of the error when an access token source fails.
const ACCESS_TOKEN_SOURCE_FAILED: &str = "the BigQuery access token source failed";

impl BigQuerySessionStoreError {
    /// Whether BigQuery refused the access token (HTTP 401) or an access
    /// token source failed: an expired, revoked or unavailable credential
    /// rather than a table or network problem.
    pub fn is_access_token_refusal(&self) -> bool {
        match self {
            Self::Api(message) => {
                message.starts_with(ACCESS_TOKEN_REFUSED)
                    || message.starts_with(ACCESS_TOKEN_REFUSED_AFTER_REFRESH)
            }
            Self::Configuration(message) => message.starts_with(ACCESS_TOKEN_SOURCE_FAILED),
            _ => false,
        }
    }
}

/// A BigQuery access token and when it stops being valid.
#[derive(Clone, PartialEq, Eq)]
pub struct BigQueryAccessToken {
    pub token: String,
    /// When BigQuery stops accepting the token; `None` when unknown, and the
    /// token is then used until BigQuery refuses it.
    pub expires_at: Option<SystemTime>,
}

impl std::fmt::Debug for BigQueryAccessToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BigQueryAccessToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl BigQueryAccessToken {
    /// Whether the token expires within `margin` from now.
    fn expires_within(&self, margin: Duration) -> bool {
        self.expires_at.is_some_and(|expires_at| {
            !expires_at
                .duration_since(SystemTime::now())
                .is_ok_and(|left| left > margin)
        })
    }
}

type FetchBigQueryAccessToken = dyn Fn() -> futures::future::BoxFuture<'static, Result<BigQueryAccessToken, String>>
    + Send
    + Sync;

/// Supplies the access tokens of an adapter's requests, so a long-lived
/// adapter can use short-lived tokens: a Google access token lives about an
/// hour. The adapter keeps the last token behind one lock, and fetches a new
/// one when there is none, when the cached one is within the refresh margin
/// of its expiry, or when BigQuery refuses it with 401; it then sends that
/// one request once more with the new token. Concurrent requests share one
/// fetch, and clones share the cache. See
/// [`BigQuerySessionStoreAdapter::with_access_token_source`].
#[derive(Clone)]
pub struct BigQueryAccessTokenSource {
    fetch: Arc<FetchBigQueryAccessToken>,
    cached: Arc<tokio::sync::Mutex<Option<BigQueryAccessToken>>>,
    refresh_margin: Duration,
}

impl std::fmt::Debug for BigQueryAccessTokenSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BigQueryAccessTokenSource")
            .field("refresh_margin", &self.refresh_margin)
            .finish_non_exhaustive()
    }
}

impl BigQueryAccessTokenSource {
    /// How long before its expiry a cached token is replaced unless
    /// [`Self::with_refresh_margin`] sets another margin.
    pub const DEFAULT_REFRESH_MARGIN: Duration = Duration::from_mins(5);

    /// A source that awaits `fetch()` whenever it needs a new token.
    pub fn new<F, Fut>(fetch: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<BigQueryAccessToken, String>> + Send + 'static,
    {
        Self {
            fetch: Arc::new(move || Box::pin(fetch())),
            cached: Arc::new(tokio::sync::Mutex::new(None)),
            refresh_margin: Self::DEFAULT_REFRESH_MARGIN,
        }
    }

    /// Replace a cached token once it is within `margin` of its expiry.
    pub fn with_refresh_margin(mut self, margin: Duration) -> Self {
        self.refresh_margin = margin;
        self
    }

    /// The cached token, or a new one when there is none, when the cached one
    /// is within the refresh margin of its expiry, or when it is `refused`.
    async fn token(&self, refused: Option<&str>) -> Result<String, BigQuerySessionStoreError> {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref()
            && refused != Some(token.token.as_str())
            && !token.expires_within(self.refresh_margin)
        {
            return Ok(token.token.clone());
        }
        *cached = None;
        let fresh = (self.fetch)().await.map_err(|err| {
            BigQuerySessionStoreError::Configuration(format!("{ACCESS_TOKEN_SOURCE_FAILED}: {err}"))
        })?;
        let token = fresh.token.trim().to_string();
        if token.is_empty() {
            return Err(BigQuerySessionStoreError::Configuration(format!(
                "{ACCESS_TOKEN_SOURCE_FAILED}: it returned an empty token"
            )));
        }
        *cached = Some(BigQueryAccessToken {
            token: token.clone(),
            expires_at: fresh.expires_at,
        });
        Ok(token)
    }
}

#[derive(Debug, Clone)]
pub struct BigQuerySessionStoreAdapter {
    dataset: String,
    table: String,
    project_id: Option<String>,
    api_base_url: String,
    access_token: Option<String>,
    access_token_source: Option<BigQueryAccessTokenSource>,
    http_timeout: Duration,
    client: reqwest::Client,
}

struct JsonFileLockGuard {
    lock_path: PathBuf,
}

impl Drop for JsonFileLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

pub fn session_store_contracts(decisions: &RuntimeDecisionState) -> Vec<SessionStoreContract> {
    vec![
        SessionStoreContract {
            store: SessionStoreKind::BigQuery,
            latest_row_per_session: true,
            tombstones_supported: true,
            dedup_read_path: true,
            file_locking: false,
            crash_recovery: false,
            bigquery_dataset: Some(decisions.bigquery.dataset.clone()),
            bigquery_table: Some(decisions.bigquery.table.clone()),
        },
        SessionStoreContract {
            store: SessionStoreKind::JsonFile,
            latest_row_per_session: true,
            tombstones_supported: true,
            dedup_read_path: true,
            file_locking: true,
            crash_recovery: true,
            bigquery_dataset: None,
            bigquery_table: None,
        },
    ]
}

pub fn materialize_latest_session_rows(
    rows: &[SessionPersistenceRow],
) -> Vec<SessionPersistenceRow> {
    let mut latest_by_session: BTreeMap<String, SessionPersistenceRow> = BTreeMap::new();
    for row in rows {
        let should_replace = match latest_by_session.get(&row.session_id) {
            Some(existing) => row.updated_at_ms >= existing.updated_at_ms,
            None => true,
        };
        if should_replace {
            latest_by_session.insert(row.session_id.clone(), row.clone());
        }
    }
    latest_by_session.into_values().collect()
}

pub fn materialize_live_session_rows(rows: &[SessionPersistenceRow]) -> Vec<SessionPersistenceRow> {
    materialize_latest_session_rows(rows)
        .into_iter()
        .filter(|row| !row.deleted)
        .collect()
}

impl JsonFileSessionStore {
    pub fn new(data_path: impl AsRef<Path>) -> Self {
        let data_path = data_path.as_ref().to_path_buf();
        let lock_path = data_path.with_extension("lock");
        Self {
            data_path,
            lock_path,
            stale_lock_threshold: Duration::from_secs(30),
        }
    }

    pub fn with_lock_path(mut self, lock_path: impl AsRef<Path>) -> Self {
        self.lock_path = lock_path.as_ref().to_path_buf();
        self
    }

    pub fn with_stale_lock_threshold(mut self, threshold: Duration) -> Self {
        self.stale_lock_threshold = threshold;
        self
    }

    pub fn data_path(&self) -> &Path {
        &self.data_path
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub fn append_rows(
        &self,
        rows: &[SessionPersistenceRow],
    ) -> Result<(), JsonFileSessionStoreError> {
        let _guard = self.acquire_lock()?;
        if let Some(parent) = self.data_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        }

        let mut persisted = self.read_rows()?;
        persisted.extend(rows.iter().cloned());

        let tmp_path = self.data_path.with_extension("tmp");
        let json = serde_json::to_vec_pretty(&persisted)
            .map_err(|err| JsonFileSessionStoreError::Serialize(err.to_string()))?;
        fs::write(&tmp_path, json).map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        fs::rename(&tmp_path, &self.data_path)
            .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        Ok(())
    }

    pub fn read_rows(&self) -> Result<Vec<SessionPersistenceRow>, JsonFileSessionStoreError> {
        if !self.data_path.exists() {
            return Ok(vec![]);
        }
        let bytes = fs::read(&self.data_path)
            .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        serde_json::from_slice::<Vec<SessionPersistenceRow>>(&bytes)
            .map_err(|err| JsonFileSessionStoreError::InvalidStoreData(err.to_string()))
    }

    pub fn read_latest_rows(
        &self,
    ) -> Result<Vec<SessionPersistenceRow>, JsonFileSessionStoreError> {
        let rows = self.read_rows()?;
        Ok(materialize_latest_session_rows(&rows))
    }

    pub fn read_live_rows(&self) -> Result<Vec<SessionPersistenceRow>, JsonFileSessionStoreError> {
        let rows = self.read_rows()?;
        Ok(materialize_live_session_rows(&rows))
    }

    fn acquire_lock(&self) -> Result<JsonFileLockGuard, JsonFileSessionStoreError> {
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        }

        let mut attempts = 0_u8;
        loop {
            attempts += 1;
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&self.lock_path)
            {
                Ok(mut file) => {
                    let lock_record = JsonStoreLockRecord {
                        owner_pid: std::process::id(),
                        created_at_ms: current_time_ms(),
                    };
                    let lock_bytes = serde_json::to_vec(&lock_record)
                        .map_err(|err| JsonFileSessionStoreError::Serialize(err.to_string()))?;
                    file.write_all(&lock_bytes)
                        .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
                    return Ok(JsonFileLockGuard {
                        lock_path: self.lock_path.clone(),
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempts >= 2 {
                        return Err(JsonFileSessionStoreError::LockHeld {
                            lock_path: self.lock_path.display().to_string(),
                        });
                    }
                    if self.is_lock_stale()? {
                        fs::remove_file(&self.lock_path).map_err(|remove_err| {
                            JsonFileSessionStoreError::StaleLockRecoveryFailed(
                                remove_err.to_string(),
                            )
                        })?;
                        continue;
                    }
                    return Err(JsonFileSessionStoreError::LockHeld {
                        lock_path: self.lock_path.display().to_string(),
                    });
                }
                Err(err) => return Err(JsonFileSessionStoreError::Io(err.to_string())),
            }
        }
    }

    fn is_lock_stale(&self) -> Result<bool, JsonFileSessionStoreError> {
        let bytes = fs::read(&self.lock_path)
            .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        let stale_threshold_ms = self.stale_lock_threshold.as_millis() as u64;
        if let Ok(record) = serde_json::from_slice::<JsonStoreLockRecord>(&bytes) {
            let age_ms = current_time_ms().saturating_sub(record.created_at_ms);
            if age_ms < stale_threshold_ms {
                return Ok(false);
            }
            return Ok(!is_process_alive(record.owner_pid));
        }

        let modified = fs::metadata(&self.lock_path)
            .and_then(|meta| meta.modified())
            .map_err(|err| JsonFileSessionStoreError::Io(err.to_string()))?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        Ok(age >= self.stale_lock_threshold)
    }
}

fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let status = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(exit_status) => exit_status.success(),
        // If liveness probing is unavailable, avoid evicting potentially active locks.
        Err(_) => true,
    }
}

impl BigQuerySessionStoreAdapter {
    pub const DEFAULT_API_BASE_URL: &'static str = "https://bigquery.googleapis.com/bigquery/v2";

    pub fn new(
        _legacy_bq_binary: impl AsRef<Path>,
        dataset: impl Into<String>,
        table: impl Into<String>,
    ) -> Self {
        Self::new_native(dataset, table)
    }

    pub fn new_native(dataset: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            dataset: dataset.into(),
            table: table.into(),
            project_id: None,
            api_base_url: Self::DEFAULT_API_BASE_URL.to_string(),
            access_token: None,
            access_token_source: None,
            client: shared_bigquery_http_client(),
            http_timeout: Duration::from_secs(30),
        }
    }

    pub fn with_project_id(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    pub fn with_api_base_url(mut self, api_base_url: impl Into<String>) -> Self {
        self.api_base_url = api_base_url.into();
        self
    }

    pub fn with_access_token(mut self, access_token: impl Into<String>) -> Self {
        self.access_token = Some(access_token.into());
        self
    }

    /// Take the access token of every request from `source`, ahead of
    /// [`Self::with_access_token`] and the token environment variables. A
    /// request BigQuery refuses with 401 is sent once more with a token the
    /// source fetches anew; a source that fails, or a second 401, fails the
    /// request ([`BigQuerySessionStoreError::is_access_token_refusal`]). No
    /// other failure is retried.
    pub fn with_access_token_source(mut self, source: BigQueryAccessTokenSource) -> Self {
        self.access_token_source = Some(source);
        self
    }

    pub fn with_http_timeout(mut self, timeout: Duration) -> Self {
        // The timeout applies per REQUEST (see `authorized_request`) over one
        // shared client: reqwest's first `Client::build()` in a process pays
        // the macOS system-proxy scan (~700ms observed) since the realtime
        // feature unification enabled reqwest's `system-proxy`; rebuilding a
        // client per adapter put that scan — and a connector build — inside
        // every RPC's latency window.
        self.http_timeout = timeout;
        self
    }

    pub fn with_bearer_token(self, access_token: impl Into<String>) -> Self {
        self.with_access_token(access_token)
    }

    pub fn table_ref(&self) -> String {
        format!("{}.{}", self.dataset, self.table)
    }

    pub async fn stream_insert_rows(
        &self,
        rows: &[SessionPersistenceRow],
    ) -> Result<(), BigQuerySessionStoreError> {
        if rows.is_empty() {
            return Ok(());
        }

        let mut request_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let payload_json = serde_json::to_string(&row.payload)
                .map_err(|err| BigQuerySessionStoreError::Serialize(err.to_string()))?;
            let mut row_json = serde_json::json!({
                "session_id": row.session_id,
                "updated_at_ms": row.updated_at_ms.to_string(),
                "deleted": row.deleted,
                "payload": payload_json,
            });
            if !row.labels.is_empty() {
                let labels_json = serde_json::to_string(&row.labels)
                    .map_err(|err| BigQuerySessionStoreError::Serialize(err.to_string()))?;
                row_json["labels_json"] = serde_json::Value::String(labels_json);
            }
            request_rows.push(row_json);
        }
        self.insert_json_rows(&self.table, request_rows).await
    }

    /// Streams `rows`, each a JSON object in the schema of `table`, into that
    /// table of this adapter's dataset with one `insertAll` request. Any row
    /// error fails the call.
    pub(crate) async fn insert_json_rows(
        &self,
        table: &str,
        rows: Vec<Value>,
    ) -> Result<(), BigQuerySessionStoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let project_id = self.resolve_project_id()?;
        let endpoint = format!(
            "{}/projects/{project_id}/datasets/{}/tables/{table}/insertAll",
            self.api_base_url(),
            self.dataset
        );
        let request_rows: Vec<Value> = rows
            .into_iter()
            .map(|row| serde_json::json!({ "json": row }))
            .collect();
        let request = serde_json::json!({
            "ignoreUnknownValues": false,
            "skipInvalidRows": false,
            "rows": request_rows,
        });

        let response = self
            .send_authorized(reqwest::Method::POST, &endpoint, Some(&request))
            .await?;
        if let Some(errors) = response.get("insertErrors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            let detail =
                serde_json::to_string(errors).unwrap_or_else(|_| "<serialize_error>".to_string());
            return Err(BigQuerySessionStoreError::Api(format!(
                "BigQuery insertAll returned row errors: {detail}"
            )));
        }

        Ok(())
    }

    /// `table` of this adapter's dataset as standard SQL names it:
    /// `project.dataset.table`.
    pub(crate) fn qualified_table_name(
        &self,
        table: &str,
    ) -> Result<String, BigQuerySessionStoreError> {
        let project_id = self.resolve_project_id()?;
        Ok(format!("{project_id}.{}.{table}", self.dataset))
    }

    /// Runs `query` (standard SQL) and returns the first column of every
    /// result row as text, waiting for the job to complete and following the
    /// result pages. A null cell is skipped.
    pub(crate) async fn query_first_column(
        &self,
        query: &str,
    ) -> Result<Vec<String>, BigQuerySessionStoreError> {
        let project_id = self.resolve_project_id()?;
        let endpoint = format!("{}/projects/{project_id}/queries", self.api_base_url());
        let request = serde_json::json!({
            "query": query,
            "useLegacySql": false,
            "maxResults": 10000,
        });
        let mut response = self
            .send_authorized(reqwest::Method::POST, &endpoint, Some(&request))
            .await?;
        let mut values = Vec::new();
        loop {
            let complete = response
                .get("jobComplete")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            if complete {
                let rows = response.get("rows").and_then(Value::as_array);
                for row in rows.into_iter().flatten() {
                    if let Some(value) = row.pointer("/f/0/v").and_then(Value::as_str) {
                        values.push(value.to_string());
                    }
                }
            }
            let page_token = response.get("pageToken").and_then(Value::as_str);
            if complete && page_token.is_none() {
                return Ok(values);
            }
            let job_id = response
                .pointer("/jobReference/jobId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    BigQuerySessionStoreError::InvalidQueryResponse(
                        "an unfinished or paged query response names no jobReference.jobId"
                            .to_string(),
                    )
                })?;
            let mut params = vec![("maxResults", "10000".to_string())];
            if let Some(location) = response
                .pointer("/jobReference/location")
                .and_then(Value::as_str)
            {
                params.push(("location", location.to_string()));
            }
            if complete && let Some(token) = page_token {
                params.push(("pageToken", token.to_string()));
            }
            let results = reqwest::Url::parse_with_params(&format!("{endpoint}/{job_id}"), &params)
                .map_err(|err| {
                    BigQuerySessionStoreError::Configuration(format!(
                        "BigQuery query results endpoint: {err}"
                    ))
                })?;
            response = self
                .send_authorized(reqwest::Method::GET, results.as_str(), None)
                .await?;
        }
    }

    pub async fn read_rows(&self) -> Result<Vec<SessionPersistenceRow>, BigQuerySessionStoreError> {
        let fq_table = self.fully_qualified_table()?;
        let query = format!(
            "SELECT session_id, updated_at_ms, deleted, payload, labels_json FROM `{fq_table}` ORDER BY updated_at_ms ASC"
        );
        self.execute_query(&query).await
    }

    pub async fn read_latest_rows(
        &self,
    ) -> Result<Vec<SessionPersistenceRow>, BigQuerySessionStoreError> {
        let fq_table = self.fully_qualified_table()?;
        let query = format!(
            "SELECT session_id, updated_at_ms, deleted, payload, labels_json \
             FROM `{fq_table}` \
             QUALIFY ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY updated_at_ms DESC) = 1"
        );
        self.execute_query(&query).await
    }

    pub async fn read_live_rows(
        &self,
    ) -> Result<Vec<SessionPersistenceRow>, BigQuerySessionStoreError> {
        let fq_table = self.fully_qualified_table()?;
        let query = format!(
            "SELECT session_id, updated_at_ms, deleted, payload, labels_json FROM (\
               SELECT session_id, updated_at_ms, deleted, payload, labels_json \
               FROM `{fq_table}` \
               QUALIFY ROW_NUMBER() OVER (PARTITION BY session_id ORDER BY updated_at_ms DESC) = 1\
             ) WHERE deleted = false"
        );
        self.execute_query(&query).await
    }

    fn fully_qualified_table(&self) -> Result<String, BigQuerySessionStoreError> {
        let project_id = self.resolve_project_id()?;
        Ok(format!("{}.{}", project_id, self.table_ref()))
    }

    async fn execute_query(
        &self,
        query: &str,
    ) -> Result<Vec<SessionPersistenceRow>, BigQuerySessionStoreError> {
        let project_id = self.resolve_project_id()?;
        let endpoint = format!("{}/projects/{project_id}/queries", self.api_base_url());
        let request = serde_json::json!({
            "query": query,
            "useLegacySql": false,
            "maxResults": 10000,
        });
        let response = self
            .send_authorized(reqwest::Method::POST, &endpoint, Some(&request))
            .await?;
        parse_bigquery_query_rows(&response)
    }

    pub async fn gc_superseded_rows(&self) -> Result<u64, BigQuerySessionStoreError> {
        let project_id = self.resolve_project_id()?;
        let table_ref = self.table_ref();
        let endpoint = format!("{}/projects/{project_id}/queries", self.api_base_url());
        let query = format!(
            "DELETE FROM `{project_id}.{table_ref}` AS t \
             WHERE STRUCT(t.session_id, t.updated_at_ms) NOT IN ( \
               SELECT AS STRUCT session_id, MAX(updated_at_ms) \
               FROM `{project_id}.{table_ref}` \
               GROUP BY session_id \
             )"
        );
        let request = serde_json::json!({ "query": query, "useLegacySql": false });
        let response = self
            .send_authorized(reqwest::Method::POST, &endpoint, Some(&request))
            .await?;
        let affected = response
            .get("numDmlAffectedRows")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(affected)
    }

    pub async fn truncate_sessions(&self) -> Result<(), BigQuerySessionStoreError> {
        let project_id = self.resolve_project_id()?;
        let table_ref = self.table_ref();
        let endpoint = format!("{}/projects/{project_id}/queries", self.api_base_url());
        let query = format!("TRUNCATE TABLE `{project_id}.{table_ref}`");
        let request = serde_json::json!({ "query": query, "useLegacySql": false });
        self.send_authorized(reqwest::Method::POST, &endpoint, Some(&request))
            .await?;
        Ok(())
    }

    fn api_base_url(&self) -> &str {
        self.api_base_url.trim_end_matches('/')
    }

    fn resolve_project_id(&self) -> Result<String, BigQuerySessionStoreError> {
        if let Some(project_id) = self.project_id.as_deref() {
            let project = project_id.trim();
            if !project.is_empty() {
                return Ok(project.to_string());
            }
        }

        if let Ok(project_id) = std::env::var("BIGQUERY_PROJECT_ID") {
            let project = project_id.trim();
            if !project.is_empty() {
                return Ok(project.to_string());
            }
        }

        Err(BigQuerySessionStoreError::Configuration(
            "missing BigQuery project_id: call with_project_id(...) or set BIGQUERY_PROJECT_ID"
                .to_string(),
        ))
    }

    fn resolve_access_token(&self) -> Result<String, BigQuerySessionStoreError> {
        if let Some(token) = self.access_token.as_deref() {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(token.to_string());
            }
        }

        for key in [
            "BIGQUERY_ACCESS_TOKEN",
            "GOOGLE_OAUTH_ACCESS_TOKEN",
            "GOOGLE_ACCESS_TOKEN",
        ] {
            if let Ok(token) = std::env::var(key) {
                let token = token.trim();
                if !token.is_empty() {
                    return Ok(token.to_string());
                }
            }
        }

        Err(BigQuerySessionStoreError::Configuration(
            "missing BigQuery access token: call with_access_token(...) or set BIGQUERY_ACCESS_TOKEN"
                .to_string(),
        ))
    }

    /// Sends one request with the access token. When a token source supplies
    /// it and BigQuery refuses it with 401, the request was not applied, so it
    /// is sent once more with a token the source fetches anew. No other
    /// failure is retried.
    async fn send_authorized(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: Option<&Value>,
    ) -> Result<Value, BigQuerySessionStoreError> {
        let Some(source) = &self.access_token_source else {
            let access_token = self.resolve_access_token()?;
            return self
                .send_json_request(method, endpoint, &access_token, body)
                .await;
        };
        let access_token = source.token(None).await?;
        let (status, text) = self
            .send_request(method.clone(), endpoint, &access_token, body)
            .await?;
        if status != reqwest::StatusCode::UNAUTHORIZED {
            return json_response(status, &text);
        }
        let access_token = source.token(Some(&access_token)).await?;
        let (status, text) = self
            .send_request(method, endpoint, &access_token, body)
            .await?;
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(BigQuerySessionStoreError::Api(format!(
                "{ACCESS_TOKEN_REFUSED_AFTER_REFRESH}: {text}"
            )));
        }
        json_response(status, &text)
    }

    async fn send_json_request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        access_token: &str,
        body: Option<&Value>,
    ) -> Result<Value, BigQuerySessionStoreError> {
        let (status, text) = self
            .send_request(method, endpoint, access_token, body)
            .await?;
        json_response(status, &text)
    }

    async fn send_request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        access_token: &str,
        body: Option<&Value>,
    ) -> Result<(reqwest::StatusCode, String), BigQuerySessionStoreError> {
        let mut request = self
            .client
            .request(method, endpoint)
            .timeout(self.http_timeout)
            .bearer_auth(access_token)
            .header("accept", "application/json");
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|err| BigQuerySessionStoreError::Http(format!("{err:?}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| BigQuerySessionStoreError::Http(format!("{err:?}")))?;
        Ok((status, text))
    }
}

/// A BigQuery answer's JSON, or the error its status names.
fn json_response(
    status: reqwest::StatusCode,
    text: &str,
) -> Result<Value, BigQuerySessionStoreError> {
    if !status.is_success() {
        return Err(BigQuerySessionStoreError::Api(format!(
            "BigQuery API request failed (status {}): {}",
            status.as_u16(),
            text
        )));
    }

    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }

    serde_json::from_str::<Value>(text)
        .map_err(|err| BigQuerySessionStoreError::InvalidQueryResponse(err.to_string()))
}

#[derive(Debug, Clone)]
pub struct BigQueryGcConfig {
    pub interval: Duration,
}

impl Default for BigQueryGcConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_hours(6),
        }
    }
}

/// Callback for GC error reporting. Receives the error message.
pub type GcErrorCallback = std::sync::Arc<dyn Fn(String) + Send + Sync>;

pub fn run_periodic_gc(
    adapter: BigQuerySessionStoreAdapter,
    config: BigQueryGcConfig,
) -> impl FnOnce() {
    run_periodic_gc_with_error_callback(adapter, config, None)
}

pub fn run_periodic_gc_with_error_callback(
    adapter: BigQuerySessionStoreAdapter,
    config: BigQueryGcConfig,
    on_error: Option<GcErrorCallback>,
) -> impl FnOnce() {
    move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                let msg = format!("failed to create async runtime for BQ GC: {err}");
                eprintln!("[meta-gc] {msg}");
                if let Some(ref cb) = on_error {
                    cb(msg);
                }
                return;
            }
        };
        loop {
            std::thread::sleep(config.interval);
            match rt.block_on(adapter.gc_superseded_rows()) {
                Ok(deleted) => {
                    if deleted > 0 {
                        eprintln!("[meta-gc] deleted {deleted} superseded session rows");
                    }
                }
                Err(err) => {
                    let msg = format!("gc_superseded_rows failed: {err:?}");
                    eprintln!("[meta-gc] {msg}");
                    if let Some(ref cb) = on_error {
                        cb(msg);
                    }
                }
            }
        }
    }
}

fn parse_bigquery_query_rows(
    response: &Value,
) -> Result<Vec<SessionPersistenceRow>, BigQuerySessionStoreError> {
    if response.is_array() {
        return serde_json::from_value::<Vec<SessionPersistenceRow>>(response.clone())
            .map_err(|err| BigQuerySessionStoreError::InvalidQueryResponse(err.to_string()));
    }

    let rows = response
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut parsed = Vec::with_capacity(rows.len());
    for row in rows {
        parsed.push(parse_bigquery_query_row(&row)?);
    }

    Ok(parsed)
}

fn parse_bigquery_query_row(
    row: &Value,
) -> Result<SessionPersistenceRow, BigQuerySessionStoreError> {
    let fields = row.get("f").and_then(Value::as_array).ok_or_else(|| {
        BigQuerySessionStoreError::InvalidQueryResponse(
            "missing row.f cell array in query response".to_string(),
        )
    })?;
    if fields.len() < 4 {
        return Err(BigQuerySessionStoreError::InvalidQueryResponse(
            "query response row has fewer than 4 columns".to_string(),
        ));
    }

    let session_id = parse_bigquery_string_cell(&fields[0], "session_id")?;
    let updated_at_ms = parse_bigquery_u64_cell(&fields[1], "updated_at_ms")?;
    let deleted = parse_bigquery_bool_cell(&fields[2], "deleted")?;
    let payload = parse_bigquery_payload_cell(&fields[3], "payload")?;
    let labels = if fields.len() > 4 {
        parse_bigquery_labels_cell(&fields[4])?
    } else {
        BTreeMap::new()
    };

    Ok(SessionPersistenceRow {
        session_id,
        updated_at_ms,
        deleted,
        payload,
        labels,
    })
}

fn parse_bigquery_string_cell(
    cell: &Value,
    column: &str,
) -> Result<String, BigQuerySessionStoreError> {
    let value = bigquery_cell_value(cell);
    match value {
        Value::String(s) => Ok(s.clone()),
        _ => Err(BigQuerySessionStoreError::InvalidQueryResponse(format!(
            "query column {column} is not a string"
        ))),
    }
}

fn parse_bigquery_u64_cell(cell: &Value, column: &str) -> Result<u64, BigQuerySessionStoreError> {
    let value = bigquery_cell_value(cell);
    match value {
        Value::Number(num) => num.as_u64().ok_or_else(|| {
            BigQuerySessionStoreError::InvalidQueryResponse(format!(
                "query column {column} is not a u64 number"
            ))
        }),
        Value::String(s) => s.parse::<u64>().map_err(|_| {
            BigQuerySessionStoreError::InvalidQueryResponse(format!(
                "query column {column} is not a u64 string"
            ))
        }),
        _ => Err(BigQuerySessionStoreError::InvalidQueryResponse(format!(
            "query column {column} is not a u64 value"
        ))),
    }
}

fn parse_bigquery_bool_cell(cell: &Value, column: &str) -> Result<bool, BigQuerySessionStoreError> {
    let value = bigquery_cell_value(cell);
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::String(s) => match s.as_str() {
            "true" | "TRUE" | "1" => Ok(true),
            "false" | "FALSE" | "0" => Ok(false),
            _ => Err(BigQuerySessionStoreError::InvalidQueryResponse(format!(
                "query column {column} is not a bool string"
            ))),
        },
        _ => Err(BigQuerySessionStoreError::InvalidQueryResponse(format!(
            "query column {column} is not a bool value"
        ))),
    }
}

fn parse_bigquery_payload_cell(
    cell: &Value,
    column: &str,
) -> Result<Value, BigQuerySessionStoreError> {
    let value = bigquery_cell_value(cell);
    match value {
        Value::Null => Ok(serde_json::json!({})),
        Value::String(s) => {
            if s.trim().is_empty() {
                return Ok(serde_json::json!({}));
            }
            serde_json::from_str::<Value>(s).map_err(|_| {
                BigQuerySessionStoreError::InvalidQueryResponse(format!(
                    "query column {column} payload JSON parse failed"
                ))
            })
        }
        _ => Ok(value.clone()),
    }
}

fn parse_bigquery_labels_cell(
    cell: &Value,
) -> Result<BTreeMap<String, String>, BigQuerySessionStoreError> {
    let value = bigquery_cell_value(cell);
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::String(s) if s.trim().is_empty() => Ok(BTreeMap::new()),
        Value::String(s) => serde_json::from_str::<BTreeMap<String, String>>(s).map_err(|_| {
            BigQuerySessionStoreError::InvalidQueryResponse(
                "query column labels_json parse failed".to_string(),
            )
        }),
        _ => Ok(BTreeMap::new()),
    }
}

fn bigquery_cell_value(cell: &Value) -> &Value {
    cell.get("v").unwrap_or(cell)
}
