//! Parameter parsing for session store RPC methods.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BigQuerySessionStoreOperation {
    StreamInsert,
    ReadAll,
    ReadLatest,
    ReadLive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BigQuerySessionStoreRequest {
    operation: BigQuerySessionStoreOperation,
    dataset: String,
    table: String,
    project_id: Option<String>,
    access_token: Option<String>,
    api_base_url: Option<String>,
    timeout_ms: Option<u64>,
    rows: Vec<SessionPersistenceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BigQuerySessionStoreRpcError {
    Params(String),
    Store(BigQuerySessionStoreError),
}

/// The operation `params` names, read as the request parser reads it
/// (trimmed, with its aliases), whatever the other fields hold. A candidate
/// launch refuses a row insert on it.
pub(super) fn parse_bigquery_session_store_operation(
    params: &Value,
) -> Result<BigQuerySessionStoreOperation, BigQuerySessionStoreRpcError> {
    let operation_raw = params
        .get("operation")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BigQuerySessionStoreRpcError::Params(
                "operation must be one of: stream_insert_rows, read_rows, read_latest_rows, read_live_rows"
                    .to_string(),
            )
        })?;
    match operation_raw {
        "stream_insert_rows" | "stream_insert" => Ok(BigQuerySessionStoreOperation::StreamInsert),
        "read_rows" => Ok(BigQuerySessionStoreOperation::ReadAll),
        "read_latest_rows" | "read_latest" => Ok(BigQuerySessionStoreOperation::ReadLatest),
        "read_live_rows" | "read_live" => Ok(BigQuerySessionStoreOperation::ReadLive),
        _ => Err(BigQuerySessionStoreRpcError::Params(format!(
            "unsupported operation '{operation_raw}'"
        ))),
    }
}

pub(super) fn parse_bigquery_session_store_params(
    params: &Value,
) -> Result<BigQuerySessionStoreRequest, BigQuerySessionStoreRpcError> {
    let object = params.as_object().ok_or_else(|| {
        BigQuerySessionStoreRpcError::Params("params must be a JSON object".to_string())
    })?;

    let operation = parse_bigquery_session_store_operation(params)?;

    let dataset = object
        .get("dataset")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BigQuerySessionStoreRpcError::Params("dataset must be a non-empty string".to_string())
        })?
        .to_string();
    let table = object
        .get("table")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            BigQuerySessionStoreRpcError::Params("table must be a non-empty string".to_string())
        })?
        .to_string();

    let project_id = parse_optional_bigquery_string_field(object, "project_id")?;
    let access_token = parse_optional_bigquery_string_field(object, "access_token")?;
    let api_base_url = parse_optional_bigquery_string_field(object, "api_base_url")?;
    if let Some(ref url) = api_base_url {
        validate_bigquery_api_base_url(url)?;
    }
    let timeout_ms = match object.get("timeout_ms") {
        None => None,
        Some(value) => {
            let timeout_ms = value.as_u64().ok_or_else(|| {
                BigQuerySessionStoreRpcError::Params(
                    "timeout_ms must be a positive integer when provided".to_string(),
                )
            })?;
            if timeout_ms == 0 {
                return Err(BigQuerySessionStoreRpcError::Params(
                    "timeout_ms must be greater than 0".to_string(),
                ));
            }
            Some(timeout_ms)
        }
    };

    let rows = match operation {
        BigQuerySessionStoreOperation::StreamInsert => {
            let rows_value = object.get("rows").ok_or_else(|| {
                BigQuerySessionStoreRpcError::Params(
                    "rows must be provided for stream_insert_rows".to_string(),
                )
            })?;
            serde_json::from_value::<Vec<SessionPersistenceRow>>(rows_value.clone()).map_err(
                |_| {
                    BigQuerySessionStoreRpcError::Params(
                        "rows must be an array of session persistence rows".to_string(),
                    )
                },
            )?
        }
        _ => {
            if object.contains_key("rows") {
                return Err(BigQuerySessionStoreRpcError::Params(
                    "rows is only valid for stream_insert_rows".to_string(),
                ));
            }
            Vec::new()
        }
    };

    Ok(BigQuerySessionStoreRequest {
        operation,
        dataset,
        table,
        project_id,
        access_token,
        api_base_url,
        timeout_ms,
        rows,
    })
}

fn parse_optional_bigquery_string_field(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<String>, BigQuerySessionStoreRpcError> {
    match object.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let parsed = value.as_str().ok_or_else(|| {
                BigQuerySessionStoreRpcError::Params(format!(
                    "{field} must be a non-empty string when provided"
                ))
            })?;
            let trimmed = parsed.trim();
            if trimmed.is_empty() {
                return Err(BigQuerySessionStoreRpcError::Params(format!(
                    "{field} must be a non-empty string when provided"
                )));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

fn validate_bigquery_api_base_url(url: &str) -> Result<(), BigQuerySessionStoreRpcError> {
    let normalized = url.trim().trim_end_matches('/');

    let (scheme, after_scheme) = normalized.split_once("://").ok_or_else(|| {
        BigQuerySessionStoreRpcError::Params(
            "api_base_url must include a scheme (https:// or http:// for localhost)".to_string(),
        )
    })?;
    let authority = after_scheme.split('/').next().unwrap_or("");

    // Handle bracketed IPv6 (e.g. [::1]:8080) before splitting on ':'
    let host = if authority.starts_with('[') {
        authority
            .find(']')
            .map(|end| &authority[1..end])
            .unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };

    let is_localhost = host == "localhost" || host == "127.0.0.1" || host == "::1";

    match scheme {
        "https" => {
            if !host.ends_with(".googleapis.com") && !is_localhost {
                return Err(BigQuerySessionStoreRpcError::Params(
                    "api_base_url must point to a *.googleapis.com host".to_string(),
                ));
            }
        }
        "http" if is_localhost => { /* allow for development/testing */ }
        _ => {
            return Err(BigQuerySessionStoreRpcError::Params(
                "api_base_url must use https:// (or http:// for localhost only)".to_string(),
            ));
        }
    }

    Ok(())
}

pub(super) fn run_bigquery_session_store_request(
    request: BigQuerySessionStoreRequest,
) -> Result<Value, BigQuerySessionStoreRpcError> {
    let mut store = BigQuerySessionStoreAdapter::new_native(request.dataset, request.table);
    if let Some(project_id) = request.project_id {
        store = store.with_project_id(project_id);
    }
    if let Some(access_token) = request.access_token {
        store = store.with_access_token(access_token);
    }
    if let Some(api_base_url) = request.api_base_url {
        store = store.with_api_base_url(api_base_url);
    }
    if let Some(timeout_ms) = request.timeout_ms {
        store = store.with_http_timeout(Duration::from_millis(timeout_ms));
    }

    match request.operation {
        BigQuerySessionStoreOperation::StreamInsert => {
            block_on_bq(store.stream_insert_rows(&request.rows))
                .and_then(|inner| inner)
                .map_err(BigQuerySessionStoreRpcError::Store)?;
            Ok(serde_json::json!({
                "operation": "stream_insert_rows",
                "accepted": true,
                "inserted_rows": request.rows.len(),
            }))
        }
        BigQuerySessionStoreOperation::ReadAll => {
            let rows = block_on_bq(store.read_rows())
                .and_then(|inner| inner)
                .map_err(BigQuerySessionStoreRpcError::Store)?;
            Ok(serde_json::json!({
                "operation": "read_rows",
                "rows": rows,
            }))
        }
        BigQuerySessionStoreOperation::ReadLatest => {
            let rows = block_on_bq(store.read_latest_rows())
                .and_then(|inner| inner)
                .map_err(BigQuerySessionStoreRpcError::Store)?;
            Ok(serde_json::json!({
                "operation": "read_latest_rows",
                "rows": rows,
            }))
        }
        BigQuerySessionStoreOperation::ReadLive => {
            let rows = block_on_bq(store.read_live_rows())
                .and_then(|inner| inner)
                .map_err(BigQuerySessionStoreRpcError::Store)?;
            Ok(serde_json::json!({
                "operation": "read_live_rows",
                "rows": rows,
            }))
        }
    }
}

/// Run an async BQ future from a synchronous RPC context.
/// Spawns a dedicated thread with its own tokio runtime to avoid
/// nesting runtimes when the RPC handler runs inside an existing one.
fn block_on_bq<F: std::future::Future + Send>(
    future: F,
) -> Result<F::Output, BigQuerySessionStoreError>
where
    F::Output: Send,
{
    std::thread::scope(|s| {
        s.spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|err| {
                    BigQuerySessionStoreError::Io(format!(
                        "failed to create async runtime for BigQuery: {err}"
                    ))
                })?;
            Ok(rt.block_on(future))
        })
        .join()
        .map_err(|_| {
            BigQuerySessionStoreError::Io("BigQuery async worker thread panicked".to_string())
        })?
    })
}

pub(super) fn format_bigquery_store_error(error: &BigQuerySessionStoreError) -> String {
    match error {
        BigQuerySessionStoreError::Io(reason)
        | BigQuerySessionStoreError::Serialize(reason)
        | BigQuerySessionStoreError::Configuration(reason)
        | BigQuerySessionStoreError::Http(reason)
        | BigQuerySessionStoreError::Api(reason)
        | BigQuerySessionStoreError::InvalidQueryResponse(reason) => reason.clone(),
        BigQuerySessionStoreError::ProcessFailed { command, stderr } => {
            format!("command '{command}' failed: {stderr}")
        }
    }
}
