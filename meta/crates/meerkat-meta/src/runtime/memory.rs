//! Memory subsystem — local-JSON ledger store adapter and query execution.

use super::*;

/// The sibling a ledger write stages through before it is renamed over
/// `state_path`. Predecessor adoption reads the same rule, so the stage an
/// interrupted write leaves behind is recognized wherever it lies.
pub(crate) fn staged_ledger_path(state_path: &Path) -> PathBuf {
    state_path.with_extension("tmp")
}

impl LocalJsonMemoryStoreAdapter {
    pub(super) fn from_config(
        config: &LocalJsonMemoryBackendConfig,
    ) -> Result<Self, LocalJsonMemoryStoreError> {
        let health_check_endpoint = match config.health_check_endpoint.as_deref() {
            None => None,
            Some(raw) => {
                let endpoint = raw.trim();
                if endpoint.is_empty() {
                    return Err(LocalJsonMemoryStoreError::InvalidConfig(
                        "memory backend health_check_endpoint must not be empty when provided"
                            .to_string(),
                    ));
                }
                Some(endpoint.to_string())
            }
        };
        let state_path = config.state_path.trim();
        if state_path.is_empty() {
            return Err(LocalJsonMemoryStoreError::InvalidConfig(
                "memory backend state_path must not be empty".to_string(),
            ));
        }
        Ok(Self {
            health_check_endpoint,
            state_path: PathBuf::from(state_path),
        })
    }

    fn ensure_remote_health(&self) -> Result<(), LocalJsonMemoryStoreError> {
        let Some(endpoint) = self.health_check_endpoint.as_deref() else {
            return Ok(());
        };
        let health_url = format!("{}/v1/health", endpoint.trim_end_matches('/'));
        let parsed = parse_http_url(&health_url)?;
        let authority = format!("{}:{}", parsed.host, parsed.port);
        let mut addrs = authority.to_socket_addrs().map_err(|err| {
            LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                "healthcheck resolve failed for '{health_url}': {err}"
            ))
        })?;
        let addr = addrs.next().ok_or_else(|| {
            LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                "healthcheck resolve failed for '{health_url}': no socket addresses"
            ))
        })?;
        let mut stream = TcpStream::connect_timeout(&addr, MEMORY_LEDGER_HEALTHCHECK_TIMEOUT)
            .map_err(|err| {
                LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                    "healthcheck connect failed for '{health_url}': {err}"
                ))
            })?;
        stream
            .set_read_timeout(Some(MEMORY_LEDGER_HEALTHCHECK_TIMEOUT))
            .map_err(|err| {
                LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                    "healthcheck timeout setup failed for '{health_url}': {err}"
                ))
            })?;
        stream
            .set_write_timeout(Some(MEMORY_LEDGER_HEALTHCHECK_TIMEOUT))
            .map_err(|err| {
                LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                    "healthcheck timeout setup failed for '{health_url}': {err}"
                ))
            })?;
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            parsed.path, parsed.host
        );
        stream.write_all(request.as_bytes()).map_err(|err| {
            LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                "healthcheck write failed for '{health_url}': {err}"
            ))
        })?;
        let mut reader = BufReader::new(stream);
        let mut status_line = String::new();
        let bytes_read = reader.read_line(&mut status_line).map_err(|err| {
            LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                "healthcheck read failed for '{health_url}': {err}"
            ))
        })?;
        if bytes_read == 0 {
            return Err(LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                "healthcheck read failed for '{health_url}': empty response"
            )));
        }
        let status_code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| {
                LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                    "healthcheck parse failed for '{health_url}': invalid status line '{}'",
                    status_line.trim()
                ))
            })?;
        if (200..300).contains(&status_code) {
            Ok(())
        } else {
            Err(LocalJsonMemoryStoreError::ExternalCallFailed(format!(
                "healthcheck status failed for '{health_url}': HTTP {status_code}"
            )))
        }
    }

    pub(super) fn read_state(&self) -> Result<PersistedMemoryState, LocalJsonMemoryStoreError> {
        self.ensure_remote_health()?;
        if !self.state_path.exists() {
            return Ok(PersistedMemoryState::default());
        }
        let bytes = fs::read(&self.state_path)
            .map_err(|err| LocalJsonMemoryStoreError::Io(err.to_string()))?;
        serde_json::from_slice::<PersistedMemoryState>(&bytes)
            .map_err(|err| LocalJsonMemoryStoreError::InvalidStoreData(err.to_string()))
    }

    fn write_state(&self, state: &PersistedMemoryState) -> Result<(), LocalJsonMemoryStoreError> {
        self.ensure_remote_health()?;
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| LocalJsonMemoryStoreError::Io(err.to_string()))?;
        }
        let tmp_path = staged_ledger_path(&self.state_path);
        let json = serde_json::to_vec_pretty(state)
            .map_err(|err| LocalJsonMemoryStoreError::Serialize(err.to_string()))?;
        fs::write(&tmp_path, json).map_err(|err| LocalJsonMemoryStoreError::Io(err.to_string()))?;
        fs::rename(&tmp_path, &self.state_path)
            .map_err(|err| LocalJsonMemoryStoreError::Io(err.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedHttpUrl {
    host: String,
    port: u16,
    path: String,
}

fn parse_http_url(url: &str) -> Result<ParsedHttpUrl, LocalJsonMemoryStoreError> {
    let trimmed = url.trim();
    let without_scheme = trimmed.strip_prefix("http://").ok_or_else(|| {
        LocalJsonMemoryStoreError::InvalidConfig(format!(
            "memory backend endpoint must start with http:// (got '{trimmed}')"
        ))
    })?;
    if without_scheme.is_empty() {
        return Err(LocalJsonMemoryStoreError::InvalidConfig(
            "memory backend endpoint host must not be empty".to_string(),
        ));
    }
    let (authority, path_suffix) = without_scheme
        .split_once('/')
        .map(|(left, right)| (left, format!("/{right}")))
        .unwrap_or((without_scheme, "/".to_string()));
    if authority.is_empty() {
        return Err(LocalJsonMemoryStoreError::InvalidConfig(
            "memory backend endpoint host must not be empty".to_string(),
        ));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, raw_port))
            if !host.is_empty() && raw_port.chars().all(|c| c.is_ascii_digit()) =>
        {
            let parsed = raw_port.parse::<u16>().map_err(|_| {
                LocalJsonMemoryStoreError::InvalidConfig(format!(
                    "memory backend endpoint port is invalid in '{trimmed}'"
                ))
            })?;
            (host.to_string(), parsed)
        }
        _ => (authority.to_string(), 80_u16),
    };
    if host.is_empty() {
        return Err(LocalJsonMemoryStoreError::InvalidConfig(
            "memory backend endpoint host must not be empty".to_string(),
        ));
    }
    Ok(ParsedHttpUrl {
        host,
        port,
        path: path_suffix,
    })
}

impl MetaRuntimeHandle {
    fn next_memory_sequence(&mut self) -> u64 {
        Self::next_sequence(&mut self.memory_sequence)
    }

    pub(super) fn canonical_memory_token(raw: &str) -> Option<String> {
        let token = raw.trim().to_ascii_lowercase();
        if token.is_empty() { None } else { Some(token) }
    }

    pub(super) fn canonical_memory_store(raw: &str) -> Option<String> {
        let store = Self::canonical_memory_token(raw)?;
        if MEMORY_SUPPORTED_STORES.contains(&store.as_str()) {
            Some(store)
        } else {
            None
        }
    }

    fn default_memory_store() -> String {
        "knowledge_graph".to_string()
    }

    pub(super) fn memory_conflict_for_reference(
        &self,
        entity: Option<&str>,
        topic: Option<&str>,
    ) -> Option<MemoryConflictSignal> {
        let canonical_entity = entity.and_then(Self::canonical_memory_token);
        let canonical_topic = topic.and_then(Self::canonical_memory_token);
        match (canonical_entity, canonical_topic) {
            (Some(entity), Some(topic)) => self
                .memory_conflicts
                .values()
                .find(|signal| signal.entity == entity && signal.topic == topic)
                .cloned(),
            (Some(entity), None) => self
                .memory_conflicts
                .values()
                .find(|signal| signal.entity == entity)
                .cloned(),
            (None, Some(topic)) => self
                .memory_conflicts
                .values()
                .find(|signal| signal.topic == topic)
                .cloned(),
            (None, None) => None,
        }
    }
    pub fn memory_stores(&self) -> Vec<MemoryStoreInfo> {
        MEMORY_SUPPORTED_STORES
            .iter()
            .map(|store| MemoryStoreInfo {
                store: (*store).to_string(),
                record_count: self
                    .memory_assertions
                    .iter()
                    .filter(|assertion| assertion.store == *store)
                    .count()
                    + self
                        .memory_conflicts
                        .values()
                        .filter(|signal| signal.store == *store)
                        .count(),
            })
            .collect()
    }

    fn persist_memory_state(&self) -> Result<(), MemoryIndexError> {
        let Some(backend) = self.memory_backend.as_ref() else {
            return Ok(());
        };
        let state = PersistedMemoryState {
            assertions: self.memory_assertions.clone(),
            conflicts: self.memory_conflicts.values().cloned().collect::<Vec<_>>(),
        };
        backend
            .write_state(&state)
            .map_err(MemoryIndexError::BackendPersistFailed)
    }

    pub fn memory_index(
        &mut self,
        request: MemoryIndexRequest,
    ) -> Result<MemoryIndexResult, MemoryIndexError> {
        let entity = Self::canonical_memory_token(&request.entity)
            .ok_or(MemoryIndexError::EntityRequired)?;
        let topic =
            Self::canonical_memory_token(&request.topic).ok_or(MemoryIndexError::TopicRequired)?;
        let store = match request.store.as_deref() {
            None => Self::default_memory_store(),
            Some(raw_store) => Self::canonical_memory_store(raw_store)
                .ok_or_else(|| MemoryIndexError::UnsupportedStore(raw_store.trim().to_string()))?,
        };
        let fact = request
            .fact
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);
        let conflict = request.conflict.unwrap_or(false);
        if fact.is_none() && !conflict {
            return Err(MemoryIndexError::FactRequiredWhenConflictUnset);
        }

        let previous_memory_assertions = self.memory_assertions.clone();
        let previous_memory_conflicts = self.memory_conflicts.clone();
        let previous_memory_sequence = self.memory_sequence;

        let mut assertion_id = None;
        if let Some(fact) = fact {
            let assertion_sequence = self.next_memory_sequence();
            let assertion = MemoryAssertion {
                assertion_id: format!("memory-assert-{assertion_sequence:06}"),
                entity: entity.clone(),
                topic: topic.clone(),
                store: store.clone(),
                fact,
                metadata: request.metadata.clone(),
                indexed_at_ms: current_time_ms(),
            };
            assertion_id = Some(assertion.assertion_id.clone());
            self.memory_assertions.push(assertion);
            while self.memory_assertions.len() > MEMORY_ASSERTIONS_MAX_RETAINED {
                self.memory_assertions.remove(0);
            }
        }

        if conflict {
            let conflict_key = MemoryConflictKey {
                entity: entity.clone(),
                topic: topic.clone(),
                store: store.clone(),
            };
            self.memory_conflicts.insert(
                conflict_key,
                MemoryConflictSignal {
                    entity: entity.clone(),
                    topic: topic.clone(),
                    store: store.clone(),
                    reason: request
                        .conflict_reason
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToString::to_string),
                    updated_at_ms: current_time_ms(),
                },
            );
        }
        if let Err(error) = self.persist_memory_state() {
            self.memory_assertions = previous_memory_assertions;
            self.memory_conflicts = previous_memory_conflicts;
            self.memory_sequence = previous_memory_sequence;
            return Err(error);
        }

        let conflict_active = self
            .memory_conflict_for_reference(Some(entity.as_str()), Some(topic.as_str()))
            .is_some();

        Ok(MemoryIndexResult {
            entity,
            topic,
            store,
            assertion_id,
            conflict_active,
        })
    }
    pub fn memory_query(&self, request: MemoryQueryRequest) -> MemoryQueryResult {
        let entity = request
            .entity
            .as_deref()
            .and_then(Self::canonical_memory_token);
        let topic = request
            .topic
            .as_deref()
            .and_then(Self::canonical_memory_token);
        let store = request
            .store
            .as_deref()
            .and_then(Self::canonical_memory_store);
        let query = request
            .query
            .as_deref()
            .and_then(Self::canonical_memory_token);
        let assertion_matches_query = |assertion: &MemoryAssertion| {
            query.as_ref().is_none_or(|needle| {
                assertion.entity.contains(needle.as_str())
                    || assertion.topic.contains(needle.as_str())
                    || assertion
                        .fact
                        .to_ascii_lowercase()
                        .contains(needle.as_str())
            })
        };
        let conflict_matches_query = |signal: &MemoryConflictSignal| {
            query.as_ref().is_none_or(|needle| {
                signal.entity.contains(needle.as_str())
                    || signal.topic.contains(needle.as_str())
                    || signal
                        .reason
                        .as_deref()
                        .is_some_and(|reason| reason.to_ascii_lowercase().contains(needle.as_str()))
            })
        };
        let assertions = self
            .memory_assertions
            .iter()
            .filter(|assertion| {
                entity
                    .as_ref()
                    .is_none_or(|value| assertion.entity.as_str() == value.as_str())
            })
            .filter(|assertion| {
                topic
                    .as_ref()
                    .is_none_or(|value| assertion.topic.as_str() == value.as_str())
            })
            .filter(|assertion| {
                store
                    .as_ref()
                    .is_none_or(|value| assertion.store.as_str() == value.as_str())
            })
            .filter(|assertion| assertion_matches_query(assertion))
            .cloned()
            .collect::<Vec<_>>();
        let conflicts = self
            .memory_conflicts
            .values()
            .filter(|signal| {
                entity
                    .as_ref()
                    .is_none_or(|value| signal.entity.as_str() == value.as_str())
            })
            .filter(|signal| {
                topic
                    .as_ref()
                    .is_none_or(|value| signal.topic.as_str() == value.as_str())
            })
            .filter(|signal| {
                store
                    .as_ref()
                    .is_none_or(|value| signal.store.as_str() == value.as_str())
            })
            .filter(|signal| conflict_matches_query(signal))
            .cloned()
            .collect::<Vec<_>>();
        MemoryQueryResult {
            assertions,
            conflicts,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn local_json_adapter_roundtrips_state_without_health_endpoint() {
        let temp = tempfile::tempdir().expect("temp dir");
        let state_path = temp.path().join("memory-ledger-state.json");
        let adapter = LocalJsonMemoryStoreAdapter::from_config(&LocalJsonMemoryBackendConfig {
            state_path: state_path.to_string_lossy().to_string(),
            health_check_endpoint: None,
        })
        .expect("adapter from config");

        let state = PersistedMemoryState {
            assertions: vec![MemoryAssertion {
                assertion_id: "memory-assert-000001".to_string(),
                entity: "delivery".to_string(),
                topic: "email_send".to_string(),
                store: "todo".to_string(),
                fact: "double-check recipient consent".to_string(),
                metadata: None,
                indexed_at_ms: 1,
            }],
            conflicts: vec![],
        };
        adapter.write_state(&state).expect("write state");
        let reloaded = adapter.read_state().expect("read state");
        assert_eq!(reloaded, state);
    }

    #[test]
    fn local_json_adapter_rejects_empty_health_endpoint() {
        let error = LocalJsonMemoryStoreAdapter::from_config(&LocalJsonMemoryBackendConfig {
            state_path: "/tmp/state.json".to_string(),
            health_check_endpoint: Some("   ".to_string()),
        })
        .expect_err("empty health endpoint must be rejected");
        assert!(matches!(error, LocalJsonMemoryStoreError::InvalidConfig(_)));
    }

    #[test]
    fn memory_backend_config_refuses_the_retired_kind() {
        // The predecessor's `kind` is refused at decode, never reinterpreted
        // as the local-JSON backend; the refusal names the supported kind.
        let error = serde_json::from_value::<MemoryBackendConfig>(serde_json::json!({
            "kind": "elephant",
            "endpoint": "http://localhost:3000",
            "state_path": "/tmp/state.json"
        }))
        .expect_err("the retired kind is refused");
        let message = error.to_string();
        assert!(
            message.contains("unknown variant") && message.contains("local_json"),
            "{message}"
        );

        let parsed: MemoryBackendConfig = serde_json::from_value(serde_json::json!({
            "kind": "local_json",
            "state_path": "/tmp/state.json"
        }))
        .expect("local_json wire shape deserializes");
        assert_eq!(
            parsed,
            MemoryBackendConfig::LocalJson(LocalJsonMemoryBackendConfig {
                state_path: "/tmp/state.json".to_string(),
                health_check_endpoint: None,
            })
        );
    }
}
